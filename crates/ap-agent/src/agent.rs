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

/// An approved `confirm`: it allows exactly one call, the next one.
struct Approval {
    next: ToolCall,
    /// The owner answered (not the automatic approval of confirm_irreversible=false).
    owner: bool,
}

#[derive(Default)]
struct RunState {
    messages: Vec<Message>,
    /// Set by an approved `confirm`; consumed (or cleared) by the next tool call.
    approval: Option<Approval>,
    /// The owner denied a `confirm`: only home/back run until the owner
    /// answers `ask_user` or approves a new `confirm`.
    denied: bool,
    /// The screenshot after the last action failed: the next action needs a `look` first.
    need_look: bool,
    /// The last screen text suggested a lock screen or a passcode field.
    lock_screen: bool,
    /// `unlock` was called and did not end with "unlocked" / "not locked".
    lock_unresolved: bool,
    /// Digits typed in a row (type_text digits only, press_key digits).
    digit_run: u32,
    /// The last action typed text (a tap now may press Send).
    typed_recently: bool,
    step: u32,
    usage: UsageLog,
}

impl RunState {
    fn lock_likely(&self) -> bool {
        self.lock_screen || self.lock_unresolved
    }
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

    /// Run `fut` unless the run is cancelled first. On a cancel the device
    /// is also told to stop the calls it already received, because dropping
    /// the future does not stop work a backend worker thread has started
    /// (for example an iPhone long press or text entry).
    async fn guard<T>(&self, fut: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                self.device.cancel();
                None
            }
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
        let mut st = RunState::default();
        if self.cancel.is_cancelled() {
            return self.finish(Outcome::Cancelled, st);
        }
        // A phone with no touch input (an iPhone below iOS 27) cannot do a
        // task: stop before the first LLM call instead of looping on taps
        // that fail.
        if let Some(why) = ap_core::touch_unavailable(info, self.device.capabilities()) {
            return self.finish(
                Outcome::Failed {
                    error: format!("screen only, no touch input: {why}"),
                },
                st,
            );
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
        st.lock_screen = tools::lock_screen_hint(&first.text);
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
                None => {
                    // The provider may still bill this call; the summary says so.
                    st.usage.cancelled_calls += 1;
                    return self.finish(Outcome::Cancelled, st);
                }
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
            // Append-only history: the assistant turn goes back exactly as received,
            // except an empty turn, which the APIs reject: it gets a placeholder so
            // the roles still alternate.
            let usable = resp.content.iter().any(|b| match b {
                ContentBlock::Text { text } => !text.is_empty(),
                ContentBlock::ToolUse { .. }
                | ContentBlock::Thinking { .. }
                | ContentBlock::Opaque { .. } => true,
                _ => false,
            });
            st.messages.push(Message::assistant(if usable {
                resp.content.clone()
            } else {
                vec![ContentBlock::text("(no response)")]
            }));

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
                    input: tools::redact_input(&name, &input),
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
            return self.execute_action(call, observe, st).await;
        }
        // An approval covers only the very next call; any other tool clears it.
        if !matches!(call, ToolCall::Confirm { .. }) {
            st.approval = None;
        }
        match call {
            ToolCall::Look => self.with_screen(String::new(), true, st).await,
            ToolCall::Wait { seconds } => {
                self.guard(tokio::time::sleep(Duration::from_secs_f32(seconds)))
                    .await?;
                self.with_screen(format!("waited {seconds:.1} s"), false, st)
                    .await
            }
            ToolCall::Confirm {
                action,
                details,
                next,
            } => {
                let next = *next;
                let planned = tools::describe(&next);
                // Digit entry (a possible passcode) always needs the owner, even
                // with confirmations turned off.
                if !self.options.confirm_irreversible && tools::digit_entry(&next).is_none() {
                    st.approval = Some(Approval { next, owner: false });
                    return Some(ToolOutput::text(format!(
                        "approved (confirmations are turned off in the config). Do exactly this next: {planned}."
                    )));
                }
                let details = Some(match details {
                    Some(d) => format!("{d}\nNext step: {planned}"),
                    None => format!("Next step: {planned}"),
                });
                let req = ConfirmRequest {
                    kind: ConfirmKind::Irreversible,
                    action,
                    details,
                };
                if self.guard(self.host.confirm(&req)).await? {
                    st.approval = Some(Approval { next, owner: true });
                    st.denied = false;
                    Some(ToolOutput::text(format!(
                        "approved by the owner. Do exactly this next, with irreversible=true: {planned}. Any other call cancels the approval."
                    )))
                } else {
                    st.approval = None;
                    st.denied = true;
                    Some(ToolOutput::text(
                        "denied by the owner. Do not do it. Until the owner answers an `ask_user` question or approves a new `confirm`, only `home` and `back` run. Ask what to do instead, or call `done`.",
                    ))
                }
            }
            ToolCall::AskUser { question } => {
                let answer = self.guard(self.host.ask_user(&question)).await?;
                Some(ToolOutput::text(if answer.trim().is_empty() {
                    "The owner did not answer.".to_string()
                } else {
                    // A new instruction from the owner ends a denial.
                    st.denied = false;
                    format!("The owner answered: {answer}")
                }))
            }
            ToolCall::Unlock => self.unlock(st).await,
            // Actions and Done are handled above / by the caller.
            _ => Some(ToolOutput::error("error: tool not available here")),
        }
    }

    /// Why this action needs an approved `confirm`, if it does. Host-side
    /// rules on top of the model's own `irreversible` flag. `None` from the
    /// outer option = cancelled.
    async fn needs_confirm(&self, call: &ToolCall, st: &RunState) -> Option<Option<String>> {
        if call.irreversible() {
            return Some(Some("this action is marked irreversible".into()));
        }
        match call {
            ToolCall::TypeText { text, .. } if text.contains(['\n', '\r']) => {
                return Some(Some(
                    "this text contains a newline, which presses Enter and can send it".into(),
                ));
            }
            ToolCall::PressKey { key, .. } if matches!(key.parts().1, Key::Enter) => {
                return Some(Some("Enter can send or submit what was typed".into()));
            }
            ToolCall::Tap { .. } | ToolCall::DoubleTap { .. } | ToolCall::LongPress { .. }
                if st.typed_recently =>
            {
                let app = self
                    .guard(self.device.foreground_app())
                    .await?
                    .ok()
                    .flatten();
                return Some(match app {
                    Some(id) if !tools::sensitive_app(&id) => None,
                    Some(id) => Some(format!(
                        "a tap right after typing in {id} can send a message or a payment"
                    )),
                    None => Some(
                        "a tap right after typing can send what was typed, and the app in front is unknown"
                            .into(),
                    ),
                });
            }
            _ => {}
        }
        Some(None)
    }

    /// The safety gates, the action, then the screen. `None` = cancelled.
    async fn execute_action(
        &self,
        call: ToolCall,
        observe: bool,
        st: &mut RunState,
    ) -> Option<ToolOutput> {
        let approval = st.approval.take();
        if st.denied && !matches!(call, ToolCall::Home | ToolCall::Back) {
            return Some(ToolOutput::error(
                "refused: the owner denied the last confirmation. Only `home` and `back` run until the owner answers an `ask_user` question or approves a new `confirm`.",
            ));
        }
        if st.need_look {
            return Some(ToolOutput::error(
                "refused: the screenshot after the last action failed, so its result is unknown. Call `look` first.",
            ));
        }
        let approved = match approval {
            None => None,
            Some(a) if tools::same_action(&a.next, &call) => Some(a),
            Some(a) => {
                return Some(ToolOutput::error(format!(
                    "refused: this is not the approved action ({}). The approval is cancelled; call `confirm` again for the action you want.",
                    tools::describe(&a.next)
                )));
            }
        };
        let input = matches!(call, ToolCall::TypeText { .. } | ToolCall::PressKey { .. });
        if input && st.lock_likely() {
            return Some(ToolOutput::error(
                "refused: the phone looks locked, or a passcode or password field is shown. No typing or keys here: call `unlock`, or call `done` with success=false and ask the owner to unlock the phone.",
            ));
        }
        let digits = tools::digit_entry(&call);
        if let Some(n) = digits {
            let run = st.digit_run + n;
            if run >= 4 && !approved.as_ref().is_some_and(|a| a.owner) {
                return Some(ToolOutput::error(format!(
                    "refused: this would make {run} digits in a row, which could be a passcode or PIN. Never type passcodes: on the lock screen call `unlock`. If the owner explicitly asked you to type this number, call `confirm` with this exact call first; the owner must approve it."
                )));
            }
        }
        if self.options.confirm_irreversible
            && approved.is_none()
            && let Some(why) = self.needs_confirm(&call, st).await?
        {
            return Some(ToolOutput::error(format!(
                "refused: {why}, and no approval is pending. Call `confirm` with this exact call (tool and input) first, and do it only if the owner approves."
            )));
        }

        let result = self.guard(self.act(&call)).await?;
        // State for the next gates. A failed action may have done part of its work.
        match &call {
            ToolCall::TypeText { .. } => st.typed_recently = true,
            ToolCall::PressKey { .. } => {}
            _ => st.typed_recently = false,
        }
        match (&call, digits) {
            (_, Some(n)) => st.digit_run += n,
            (
                ToolCall::TypeText { .. }
                | ToolCall::OpenApp { .. }
                | ToolCall::Home
                | ToolCall::Back
                | ToolCall::Button { .. },
                None,
            ) => st.digit_run = 0,
            // Taps, swipes and other keys do not end a run: a passcode can be
            // split around them.
            _ => {}
        }
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                let mut t = format!("error: {e}");
                if let Some(h) = e.hint() {
                    t.push_str(&format!(" (fix: {h})"));
                }
                return Some(ToolOutput::error(t));
            }
        };
        if observe {
            self.with_screen(msg, false, st).await
        } else {
            Some(ToolOutput::text(msg))
        }
    }

    async fn unlock(&self, st: &mut RunState) -> Option<ToolOutput> {
        // The model thinks the phone is locked: block typing until it is not.
        st.lock_unresolved = true;
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
        let tried = Instant::now();
        let r = self.guard(self.host.unlock(&*self.device)).await?;
        // A host that cannot unlock (for example the CLI) did not try, so it
        // does not use up the hourly limit.
        if !matches!(r, UnlockResult::Unavailable(_)) {
            self.unlock_attempts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(tried);
        }
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
            st.lock_unresolved = false;
            self.with_screen(msg, true, st).await
        }
    }

    /// Text plus a screen observation. A failed screenshot is an error result,
    /// and the next action is refused until a `look` succeeds.
    async fn with_screen(
        &self,
        msg: String,
        settled: bool,
        st: &mut RunState,
    ) -> Option<ToolOutput> {
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
        let is_error = match obs {
            Ok(o) => {
                st.need_look = false;
                st.lock_screen = tools::lock_screen_hint(&o.text);
                content.extend(o.to_blocks());
                false
            }
            Err(e) => {
                st.need_look = true;
                content.push(ContentBlock::text(format!(
                    "error: no screenshot ({e}). You cannot see the result, so the screen may not be what you expect. Call `look` before the next action."
                )));
                true
            }
        };
        Some(ToolOutput { content, is_error })
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
                // Check the take-over flag between parts, so that a part that
                // was cut short is never followed by Enter.
                let stop = || {
                    if self.cancel.is_cancelled() {
                        Err(ap_core::Error::Other("cancelled by the owner".into()))
                    } else {
                        Ok(())
                    }
                };
                for (i, part) in text.split('\n').enumerate() {
                    if i > 0 {
                        stop()?;
                        d.key(&Key::Enter).await?;
                    }
                    if !part.is_empty() {
                        stop()?;
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
