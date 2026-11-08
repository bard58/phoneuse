//! The observe -> decide -> act loop.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ap_core::{Config, Device, Key, Platform};
use ap_llm::{ContentBlock, Effort, Message, Provider, Request, StopReason};
use serde::Serialize;

use crate::cancel::CancelToken;
use crate::host::{AgentEvent, AgentHost, ConfirmKind, ConfirmRequest, UnlockResult};
use crate::observe::{Observer, ScreenObserver};
use crate::prompt;
use crate::tools::{self, ToolCall};
use crate::usage::UsageLog;

/// Settings for one agent.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOptions {
    /// `None` = the provider's default model.
    pub model: Option<String>,
    /// Maximum LLM calls per run.
    pub max_steps: u32,
    /// Output limit per LLM call. Thinking counts toward it on Claude 5.5 models.
    pub max_tokens: u32,
    /// Claude effort. Claude Opus 5.5 defaults to medium; it is sent explicitly.
    pub effort: Option<Effort>,
    /// Require an approved `confirm` before actions marked irreversible.
    pub confirm_irreversible: bool,
    /// Allow the `unlock` tool at all (the owner must still approve each time).
    pub allow_unlock: bool,
    /// Unlock attempts allowed per hour (upstream: 3).
    pub max_unlocks_per_hour: u32,
    /// Times the model may end a turn without a tool call before the run stops.
    pub max_nudges: u32,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            model: None,
            max_steps: 50,
            max_tokens: 16_000,
            effort: Some(Effort::Medium),
            confirm_irreversible: true,
            allow_unlock: false,
            max_unlocks_per_hour: 3,
            max_nudges: 2,
        }
    }
}

impl AgentOptions {
    /// Model, token limit and safety rules from the user config.
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            model: Some(cfg.llm.model.clone()),
            max_steps: cfg.safety.max_steps.max(1),
            max_tokens: cfg.llm.max_tokens.max(1024),
            confirm_irreversible: cfg.safety.confirm_irreversible,
            allow_unlock: cfg.safety.allow_unlock,
            ..Self::default()
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// The model called `done`.
    Done { summary: String, success: bool },
    /// `max_steps` LLM calls without `done`.
    StepLimit { steps: u32 },
    /// The host cancelled (take-over).
    Cancelled,
    /// The model's safety classifier declined.
    Refused { details: Option<String> },
    /// The model stopped calling tools.
    NoAction { text: String },
    /// LLM or setup error.
    Failed { error: String },
}

/// Result of [`Agent::run`].
#[derive(Debug, Clone)]
pub struct RunReport {
    pub outcome: Outcome,
    /// LLM calls made.
    pub steps: u32,
    pub usage: UsageLog,
    /// Full conversation (images included), for debugging and replay.
    pub transcript: Vec<Message>,
}

struct RunState {
    messages: Vec<Message>,
    /// Set by an approved `confirm`; consumed by the next action.
    approval: Option<String>,
    step: u32,
    usage: UsageLog,
}

struct ToolOutput {
    content: Vec<ContentBlock>,
    is_error: bool,
}

impl ToolOutput {
    fn text(t: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(t)],
            is_error: false,
        }
    }

    fn error(t: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(t)],
            is_error: true,
        }
    }

    fn summary(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The phone agent.
pub struct Agent {
    provider: Arc<dyn Provider>,
    device: Box<dyn Device>,
    host: Arc<dyn AgentHost>,
    observer: Box<dyn Observer>,
    options: AgentOptions,
    cancel: CancelToken,
    unlock_attempts: Mutex<Vec<Instant>>,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        device: Box<dyn Device>,
        host: Arc<dyn AgentHost>,
        options: AgentOptions,
    ) -> Self {
        Self {
            provider,
            device,
            host,
            observer: Box::new(ScreenObserver::default()),
            options,
            cancel: CancelToken::new(),
            unlock_attempts: Mutex::new(Vec::new()),
        }
    }

    /// Replace the screen observer (e.g. one that adds OCR text).
    pub fn with_observer(mut self, observer: Box<dyn Observer>) -> Self {
        self.observer = observer;
        self
    }

    /// Token the host keeps to stop the run (take-over).
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    pub fn device(&self) -> &dyn Device {
        &*self.device
    }

    pub fn options(&self) -> &AgentOptions {
        &self.options
    }

    pub fn into_device(self) -> Box<dyn Device> {
        self.device
    }

    fn emit(&self, e: AgentEvent) {
        self.host.on_step(&e);
    }

    /// Run `fut` unless the run is cancelled first.
    async fn guard<T>(&self, fut: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => None,
            v = fut => Some(v),
        }
    }

    fn finish(&self, outcome: Outcome, st: RunState) -> RunReport {
        tracing::info!(?outcome, steps = st.step, usage = %st.usage.summary(), "agent run finished");
        self.emit(AgentEvent::Finished {
            outcome: outcome.clone(),
        });
        RunReport {
            outcome,
            steps: st.step,
            usage: st.usage,
            transcript: st.messages,
        }
    }

    /// Do `task` on the phone. Always returns a report; errors are an outcome.
    pub async fn run(&self, task: &str) -> RunReport {
        let model = self
            .options
            .model
            .clone()
            .unwrap_or_else(|| self.provider.default_model().to_string());
        let info = self.device.info();
        self.emit(AgentEvent::Started {
            task: task.to_string(),
            device: format!("{} ({}, {})", info.name, info.platform, info.backend),
            model: model.clone(),
        });
        let mut st = RunState {
            messages: Vec::new(),
            approval: None,
            step: 0,
            usage: UsageLog::default(),
        };
        if self.cancel.is_cancelled() {
            return self.finish(Outcome::Cancelled, st);
        }

        let first = match self.guard(self.observer.look(&*self.device)).await {
            None => return self.finish(Outcome::Cancelled, st),
            Some(Err(e)) => {
                return self.finish(
                    Outcome::Failed {
                        error: format!("cannot see the screen: {e}"),
                    },
                    st,
                );
            }
            Some(Ok(o)) => o,
        };
        let mut content = vec![ContentBlock::text(prompt::task_message(task, info))];
        content.extend(first.to_blocks());
        st.messages.push(Message::user(content));

        let tools = tools::definitions();
        let mut nudges = 0;
        while st.step < self.options.max_steps {
            if self.cancel.is_cancelled() {
                return self.finish(Outcome::Cancelled, st);
            }
            st.step += 1;
            let step = st.step;
            let req = Request {
                model: Some(model.clone()),
                system: prompt::SYSTEM_PROMPT.to_string(),
                messages: st.messages.clone(),
                tools: tools.clone(),
                max_tokens: self.options.max_tokens,
                effort: self.options.effort,
                parallel_tool_calls: false,
                cache: true,
            };
            let resp = match self.guard(self.provider.complete(req)).await {
                None => return self.finish(Outcome::Cancelled, st),
                Some(Err(e)) => {
                    return self.finish(
                        Outcome::Failed {
                            error: e.to_string(),
                        },
                        st,
                    );
                }
                Some(Ok(r)) => r,
            };
            let used_model = if resp.model.is_empty() {
                model.clone()
            } else {
                resp.model.clone()
            };
            let su = st.usage.record(step, &used_model, resp.usage).clone();
            self.emit(AgentEvent::Usage {
                step,
                model: used_model,
                usage: su.usage,
                step_cost_usd: su.cost_usd,
                total: st.usage.total,
                total_cost_usd: st.usage.total_cost_usd,
            });
            let text = resp.text();
            if !text.trim().is_empty() {
                self.emit(AgentEvent::ModelText {
                    step,
                    text: text.clone(),
                });
            }
            // Append-only history: the assistant turn goes back exactly as received.
            st.messages.push(Message::assistant(resp.content.clone()));

            if resp.stop_reason == StopReason::Refusal {
                return self.finish(
                    Outcome::Refused {
                        details: resp.stop_details.clone(),
                    },
                    st,
                );
            }
            let calls: Vec<(String, String, serde_json::Value)> = resp
                .tool_uses()
                .into_iter()
                .map(|(i, n, v)| (i.to_string(), n.to_string(), v.clone()))
                .collect();

            if resp.stop_reason == StopReason::MaxTokens && !calls.is_empty() {
                // A cut-off call may have truncated input: never run it.
                let results = calls
                    .iter()
                    .map(|(id, _, _)| {
                        ContentBlock::tool_result(
                            id.clone(),
                            vec![ContentBlock::text(
                                "not run: your response hit the output limit, so this call may be cut off. Send it again, with less text before it.",
                            )],
                            true,
                        )
                    })
                    .collect();
                st.messages.push(Message::user(results));
                continue;
            }

            if calls.is_empty() {
                nudges += 1;
                if nudges > self.options.max_nudges {
                    return self.finish(Outcome::NoAction { text }, st);
                }
                st.messages.push(Message::user_text(
                    "You ended your turn without a tool call. Continue the task with a tool, or call `done` with a summary.",
                ));
                continue;
            }
            nudges = 0;

            let mut results = Vec::with_capacity(calls.len());
            let last = calls.len() - 1;
            for (i, (id, name, input)) in calls.into_iter().enumerate() {
                if self.cancel.is_cancelled() {
                    return self.finish(Outcome::Cancelled, st);
                }
                self.emit(AgentEvent::ToolCall {
                    step,
                    name: name.clone(),
                    input: input.clone(),
                });
                let out = match tools::parse(&name, &input) {
                    Err(msg) => ToolOutput::error(format!("error: {msg}")),
                    Ok(ToolCall::Done { summary, success }) => {
                        return self.finish(Outcome::Done { summary, success }, st);
                    }
                    Ok(call) => match self.execute(call, i == last, &mut st).await {
                        None => return self.finish(Outcome::Cancelled, st),
                        Some(o) => o,
                    },
                };
                self.emit(AgentEvent::ToolResult {
                    step,
                    name,
                    ok: !out.is_error,
                    summary: out.summary(),
                });
                results.push(ContentBlock::tool_result(id, out.content, out.is_error));
            }
            st.messages.push(Message::user(results));
        }
        let steps = st.step;
        self.finish(Outcome::StepLimit { steps }, st)
    }

    /// Run one tool. `None` = cancelled.
    async fn execute(
        &self,
        call: ToolCall,
        observe: bool,
        st: &mut RunState,
    ) -> Option<ToolOutput> {
        if call.is_action() {
            let approved = st.approval.take();
            if call.irreversible() && self.options.confirm_irreversible && approved.is_none() {
                return Some(ToolOutput::error(
                    "refused: this action is marked irreversible, but no approval is pending. Call `confirm` first, and do the action only if the owner approves.",
                ));
            }
            if let ToolCall::TypeText { text, .. } = &call
                && tools::looks_like_passcode(text)
                && approved.is_none()
            {
                return Some(ToolOutput::error(
                    "refused: this looks like a passcode or PIN. Never type passcodes: on the lock screen call `unlock`. If the owner explicitly asked you to type this number, call `confirm` first.",
                ));
            }
            let msg = match self.act(&call).await {
                Ok(m) => m,
                Err(e) => {
                    let mut t = format!("error: {e}");
                    if let Some(h) = e.hint() {
                        t.push_str(&format!(" (fix: {h})"));
                    }
                    return Some(ToolOutput::error(t));
                }
            };
            return if observe {
                self.with_screen(msg, false).await
            } else {
                Some(ToolOutput::text(msg))
            };
        }
        match call {
            ToolCall::Look => self.with_screen(String::new(), true).await,
            ToolCall::Wait { seconds } => {
                self.guard(tokio::time::sleep(Duration::from_secs_f32(seconds)))
                    .await?;
                self.with_screen(format!("waited {seconds:.1} s"), false)
                    .await
            }
            ToolCall::Confirm { action, details } => {
                if !self.options.confirm_irreversible {
                    st.approval = Some(action);
                    return Some(ToolOutput::text(
                        "approved (confirmations are turned off in the config). Do that action next, with irreversible=true.",
                    ));
                }
                let req = ConfirmRequest {
                    kind: ConfirmKind::Irreversible,
                    action: action.clone(),
                    details,
                };
                if self.guard(self.host.confirm(&req)).await? {
                    st.approval = Some(action);
                    Some(ToolOutput::text(
                        "approved by the owner. Do exactly that action next, with irreversible=true.",
                    ))
                } else {
                    st.approval = None;
                    Some(ToolOutput::text(
                        "denied by the owner. Do not do it. Ask what to do instead, or call `done`.",
                    ))
                }
            }
            ToolCall::AskUser { question } => {
                let answer = self.guard(self.host.ask_user(&question)).await?;
                Some(ToolOutput::text(if answer.trim().is_empty() {
                    "The owner did not answer.".to_string()
                } else {
                    format!("The owner answered: {answer}")
                }))
            }
            ToolCall::Unlock => self.unlock().await,
            // Actions and Done are handled above / by the caller.
            _ => Some(ToolOutput::error("error: tool not available here")),
        }
    }

    async fn unlock(&self) -> Option<ToolOutput> {
        if !self.options.allow_unlock {
            return Some(ToolOutput::error(
                "refused: unlocking is turned off in the config (safety.allow_unlock). Call `done` with success=false and ask the owner to unlock the phone.",
            ));
        }
        {
            let mut a = self
                .unlock_attempts
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let now = Instant::now();
            a.retain(|t| now.duration_since(*t) < Duration::from_secs(3600));
            if a.len() as u32 >= self.options.max_unlocks_per_hour {
                return Some(ToolOutput::error(format!(
                    "refused: {} unlock attempts in the last hour. Ask the owner to unlock the phone.",
                    a.len()
                )));
            }
        }
        let req = ConfirmRequest {
            kind: ConfirmKind::Unlock,
            action: "Unlock the phone with the saved passcode".into(),
            details: None,
        };
        if !self.guard(self.host.confirm(&req)).await? {
            return Some(ToolOutput::error(
                "the owner did not approve the unlock. Call `done` with success=false.",
            ));
        }
        self.unlock_attempts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Instant::now());
        let r = self.guard(self.host.unlock(&*self.device)).await?;
        let (msg, err) = match r {
            UnlockResult::Unlocked => ("unlocked".to_string(), false),
            UnlockResult::NotLocked => ("not locked".to_string(), false),
            UnlockResult::StillLocked => (
                "still locked: the passcode did not work; not retrying. Ask the owner to unlock the phone.".to_string(),
                true,
            ),
            UnlockResult::Unavailable(why) => (format!("cannot unlock: {why}"), true),
        };
        if err {
            Some(ToolOutput::error(msg))
        } else {
            self.with_screen(msg, true).await
        }
    }

    /// Text plus a screen observation. A failed screenshot becomes a note.
    async fn with_screen(&self, msg: String, settled: bool) -> Option<ToolOutput> {
        let obs = if settled {
            self.guard(self.observer.look(&*self.device)).await?
        } else {
            self.guard(self.observer.after_action(&*self.device))
                .await?
        };
        let mut content = Vec::new();
        if !msg.is_empty() {
            content.push(ContentBlock::text(msg));
        }
        match obs {
            Ok(o) => content.extend(o.to_blocks()),
            Err(e) => content.push(ContentBlock::text(format!("(no screenshot: {e})"))),
        }
        Some(ToolOutput {
            content,
            is_error: false,
        })
    }

    /// Perform a device action. Returns a short note for the model.
    async fn act(&self, call: &ToolCall) -> ap_core::Result<String> {
        let d = &*self.device;
        match call {
            ToolCall::Tap { at, .. } => {
                d.tap(*at).await?;
                Ok(format!("tapped ({:.3}, {:.3})", at.x, at.y))
            }
            ToolCall::DoubleTap { at, .. } => {
                d.double_tap(*at).await?;
                Ok(format!("double-tapped ({:.3}, {:.3})", at.x, at.y))
            }
            ToolCall::LongPress { at, duration, .. } => {
                d.long_press(*at, *duration).await?;
                Ok(format!("long-pressed ({:.3}, {:.3})", at.x, at.y))
            }
            ToolCall::Swipe {
                from, to, duration, ..
            } => {
                d.swipe(*from, *to, *duration).await?;
                Ok("swiped".into())
            }
            ToolCall::Scroll {
                at,
                direction,
                amount,
            } => {
                // The tool names where the content goes; the device wants the finger direction.
                d.scroll(*at, direction.opposite(), *amount).await?;
                Ok("scrolled".into())
            }
            ToolCall::TypeText { text, .. } => {
                for (i, part) in text.split('\n').enumerate() {
                    if i > 0 {
                        d.key(&Key::Enter).await?;
                    }
                    if !part.is_empty() {
                        d.type_text(part).await?;
                    }
                }
                Ok(format!("typed {} characters", text.chars().count()))
            }
            ToolCall::PressKey { key, .. } => {
                d.key(key).await?;
                Ok("key pressed".into())
            }
            ToolCall::Button { button } => {
                d.button(*button).await?;
                Ok(format!("pressed {button:?}"))
            }
            ToolCall::OpenApp { app } => {
                let id = self.resolve_app(app).await?;
                d.launch_app(&id).await?;
                Ok(format!("opened {id}"))
            }
            ToolCall::Home => {
                d.home().await?;
                Ok("home screen".into())
            }
            ToolCall::Back => {
                // Android: KEYCODE_BACK (works with 3-button and gesture navigation).
                // iPhone: the backend slides a finger in from the very left edge.
                d.button(ap_core::Button::Back).await?;
                Ok("back".into())
            }
            _ => Err(ap_core::Error::InvalidArgument("not an action".into())),
        }
    }

    async fn resolve_app(&self, app: &str) -> ap_core::Result<String> {
        if app.contains('.') && !app.contains(char::is_whitespace) {
            return Ok(app.to_string());
        }
        let ios = self.device.info().platform == Platform::Ios;
        if let Some(id) = tools::app_alias(app, ios) {
            return Ok(id.to_string());
        }
        let want: String = app
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        if let Ok(apps) = self.device.list_apps().await {
            let hit = apps.iter().find(|id| {
                id.rsplit('.')
                    .next()
                    .is_some_and(|last| last.to_lowercase() == want)
            });
            if let Some(id) = hit {
                return Ok(id.clone());
            }
        }
        Err(ap_core::Error::InvalidArgument(format!(
            "no app id known for \"{app}\": give the bundle id (iOS) or package name (Android)"
        )))
    }
}
