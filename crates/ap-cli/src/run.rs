//! `agentphone run "<task>"`: the agent with a terminal host.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use ap_agent::{
    Agent, AgentEvent, AgentHost, AgentOptions, CancelToken, ConfirmKind, ConfirmRequest, Outcome,
    RunReport, UnlockResult,
};
use ap_core::{Config, Device};
use ap_llm::Provider;
use console::style;

use crate::cli::RunArgs;

/// What the CLI host answers to `unlock`. It matches the system prompt:
/// when `unlock` does not answer "unlocked", the agent stops with `done`.
pub const UNLOCK_BY_HAND: &str = "agentphone run does not enter passcodes. Call `done` with success=false and ask the owner to unlock the phone by hand, then run the task again.";

/// Answers the agent's questions. Prompts run on a blocking thread.
pub trait Prompter: Send + Sync + 'static {
    /// y/N question. Default and any error = no.
    fn confirm(&self, question: &str) -> bool;
    /// Free-text answer; empty = no answer.
    fn ask(&self, question: &str) -> String;
}

/// Real terminal prompts (dialoguer).
pub struct TerminalPrompter;

impl Prompter for TerminalPrompter {
    fn confirm(&self, question: &str) -> bool {
        dialoguer::Confirm::new()
            .with_prompt(question)
            .default(false)
            .wait_for_newline(true)
            .interact()
            .unwrap_or(false)
    }

    fn ask(&self, question: &str) -> String {
        dialoguer::Input::<String>::new()
            .with_prompt(question)
            .allow_empty(true)
            .interact_text()
            .unwrap_or_default()
    }
}

/// Prints the live step log and asks the owner.
pub struct TerminalHost<P: Prompter> {
    prompter: Arc<P>,
    json: bool,
    /// Lines are written here (tests capture them).
    log: Box<dyn Fn(&str) + Send + Sync>,
    /// Set while the run is cancelled, so late events are not printed.
    quiet: AtomicBool,
}

impl<P: Prompter> TerminalHost<P> {
    pub fn new(prompter: P, json: bool) -> Self {
        Self {
            prompter: Arc::new(prompter),
            json,
            log: Box::new(|l| eprintln!("{l}")),
            quiet: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub fn with_log(mut self, log: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.log = Box::new(log);
        self
    }

    fn line(&self, s: String) {
        if !self.quiet.load(Ordering::Relaxed) {
            (self.log)(&s);
        }
    }
}

/// One line for an event, or `None` to print nothing.
pub fn event_line(e: &AgentEvent) -> Option<String> {
    Some(match e {
        AgentEvent::Started {
            task,
            device,
            model,
        } => format!(
            "{} {task}\n  phone: {device}\n  model: {model}\n  Press Ctrl+C to stop and take over.",
            style("Task:").bold()
        ),
        AgentEvent::ModelText { step, text } => {
            let t = text.trim();
            let short: String = t.chars().take(400).collect();
            let more = if t.chars().count() > 400 { "…" } else { "" };
            format!(
                "{} {}{more}",
                style(format!("[{step}]")).dim(),
                style(short).italic()
            )
        }
        AgentEvent::ToolCall { step, name, input } => {
            let args = if input.as_object().is_some_and(|o| o.is_empty()) || input.is_null() {
                String::new()
            } else {
                let s = input.to_string();
                let s: String = s.chars().take(160).collect();
                format!(" {s}")
            };
            format!(
                "{} {} {}{}",
                style(format!("[{step}]")).dim(),
                style("→").cyan(),
                style(name).cyan().bold(),
                style(args).dim()
            )
        }
        AgentEvent::ToolResult {
            step,
            name,
            ok,
            summary,
        } => {
            if *ok && summary.trim().is_empty() {
                return None;
            }
            let s: String = summary.chars().take(200).collect();
            let mark = if *ok {
                style("✔").green()
            } else {
                style("✘").red()
            };
            format!("{} {mark} {name}: {s}", style(format!("[{step}]")).dim())
        }
        AgentEvent::Usage { .. } => return None,
        AgentEvent::Finished { .. } => return None,
    })
}

#[async_trait::async_trait]
impl<P: Prompter> AgentHost for TerminalHost<P> {
    fn on_step(&self, e: &AgentEvent) {
        if self.json {
            if let Ok(s) = serde_json::to_string(e) {
                println!("{s}");
            }
            return;
        }
        if let Some(l) = event_line(e) {
            self.line(l);
        }
    }

    async fn confirm(&self, r: &ConfirmRequest) -> bool {
        if r.kind == ConfirmKind::Unlock {
            // This host never enters a passcode (see `unlock` below), so there is
            // nothing to approve. Let the agent reach `unlock`, which tells it to
            // ask the owner to unlock the phone by hand.
            return true;
        }
        let what = "The agent wants to do something that cannot be undone:";
        let mut msg = format!("{} {what}\n  {}", style("?").yellow().bold(), r.action);
        if let Some(d) = &r.details {
            msg.push_str(&format!("\n  ({d})"));
        }
        self.line(msg);
        let p = self.prompter.clone();
        let ok = tokio::task::spawn_blocking(move || p.confirm("Allow it?"))
            .await
            .unwrap_or(false);
        self.line(if ok {
            format!("  {}", style("allowed").green())
        } else {
            format!("  {}", style("denied").red())
        });
        ok
    }

    /// The CLI does not store or type passcodes. The agent must ask the owner.
    async fn unlock(&self, _device: &dyn Device) -> UnlockResult {
        UnlockResult::Unavailable(UNLOCK_BY_HAND.into())
    }

    async fn ask_user(&self, q: &str) -> String {
        self.line(format!(
            "{} The agent asks: {q}",
            style("?").yellow().bold()
        ));
        let p = self.prompter.clone();
        tokio::task::spawn_blocking(move || p.ask("Your answer (Enter = no answer)"))
            .await
            .unwrap_or_default()
    }
}

/// Agent options from config plus command-line overrides.
pub fn options(cfg: &Config, args: &RunArgs) -> AgentOptions {
    let mut o = AgentOptions::from_config(cfg);
    if let Some(m) = &args.model {
        o.model = Some(m.clone());
    }
    if let Some(n) = args.max_steps {
        o.max_steps = n.max(1);
    }
    o
}

/// The LLM config with `--provider` / `--model` applied.
///
/// A `--provider` that changes the provider family (Anthropic or
/// OpenAI-compatible) needs `--model` too: the configured model id belongs to
/// the other family (for example `claude-opus-5-5` sent to OpenAI).
pub fn llm_config(cfg: &Config, args: &RunArgs) -> Result<ap_core::LlmConfig> {
    let mut l = cfg.llm.clone();
    if let Some(p) = &args.provider {
        l.provider = p.clone();
        if args.model.is_none() && l.is_anthropic() != cfg.llm.is_anthropic() {
            return Err(ap_core::Error::setup(
                format!(
                    "--provider {p} needs --model: the configured model \"{}\" is for provider \"{}\"",
                    cfg.llm.model, cfg.llm.provider
                ),
                format!("add `--model <model id>` for {p}, or run `agentphone setup llm`"),
            )
            .into());
        }
    }
    if let Some(m) = &args.model {
        l.model = m.clone();
    }
    Ok(l)
}

/// The final `run --json` line. Every stdout line in JSON mode is one JSON
/// object (JSONL): the events have an `event` field, and this last line has
/// `"event": "report"`.
pub fn report_line(report: &RunReport) -> Result<String> {
    Ok(serde_json::to_string(&serde_json::json!({
        "event": "report",
        "outcome": report.outcome,
        "steps": report.steps,
        "usage": report.usage,
    }))?)
}

/// Final summary text.
pub fn summary(r: &RunReport) -> String {
    let (head, body) = match &r.outcome {
        Outcome::Done { summary, success } => (
            if *success {
                style("Done.").green().bold().to_string()
            } else {
                style("Finished without success.")
                    .yellow()
                    .bold()
                    .to_string()
            },
            summary.clone(),
        ),
        Outcome::StepLimit { steps } => (
            style("Stopped.").yellow().bold().to_string(),
            format!("The step limit ({steps}) was reached. Use --max-steps to allow more."),
        ),
        Outcome::Cancelled => (
            style("Stopped.").yellow().bold().to_string(),
            "You have control of the phone.".to_string(),
        ),
        Outcome::Refused { details } => (
            style("The model declined this task.")
                .red()
                .bold()
                .to_string(),
            details.clone().unwrap_or_default(),
        ),
        Outcome::NoAction { text } => (
            style("The agent stopped acting.")
                .yellow()
                .bold()
                .to_string(),
            text.clone(),
        ),
        Outcome::Failed { error } => (style("Failed.").red().bold().to_string(), error.clone()),
    };
    let cost = r
        .usage
        .total_cost_usd
        .map(|c| format!(", about ${c:.4}"))
        .unwrap_or_default();
    let t = &r.usage.total;
    format!(
        "{head} {body}\n  {} AI steps, {} input + {} output tokens{cost}",
        r.steps,
        t.total_input(),
        t.output_tokens
    )
}

/// Process exit code for an outcome.
pub fn exit_code(o: &Outcome) -> u8 {
    match o {
        Outcome::Done { success: true, .. } => 0,
        Outcome::Cancelled => 130,
        _ => 1,
    }
}

/// Run the agent until it finishes or the user presses Ctrl+C.
pub async fn run_agent(
    provider: Arc<dyn Provider>,
    device: Box<dyn Device>,
    host: Arc<dyn AgentHost>,
    options: AgentOptions,
    observer: Option<Box<dyn ap_agent::Observer>>,
    task: &str,
    cancel_on: impl std::future::Future<Output = ()> + Send,
) -> RunReport {
    let mut agent = Agent::new(provider, device, host, options);
    if let Some(o) = observer {
        agent = agent.with_observer(o);
    }
    let token: CancelToken = agent.cancel_token();
    let watcher = async {
        cancel_on.await;
        token.cancel();
        std::future::pending::<()>().await;
    };
    tokio::select! {
        r = agent.run(task) => r,
        _ = watcher => unreachable!("the watcher never completes"),
    }
}

/// Ctrl+C handling: the first press cancels the run, a second one exits.
pub async fn ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
    eprintln!(
        "\n{} stopping the agent (press Ctrl+C again to quit now)…",
        style("Ctrl+C:").yellow().bold()
    );
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            std::process::exit(130);
        }
    });
}

/// `agentphone run`.
pub async fn command(cfg: &Config, args: &RunArgs, wanted: Option<&str>, json: bool) -> Result<u8> {
    let llm = llm_config(cfg, args)?;
    let provider = ap_llm::provider_from_config(&llm)?;

    // Ask for Jev consent before the phone connection is opened.
    #[allow(unused_mut)]
    let mut observer: Option<Box<dyn ap_agent::Observer>> = None;
    #[cfg(feature = "jev")]
    if args.jev {
        observer = Some(crate::jev::observer()?);
    }

    let device = crate::devices::connect(cfg, wanted).await?;
    let opts = options(cfg, args);
    let host: Arc<dyn AgentHost> = Arc::new(TerminalHost::new(TerminalPrompter, json));

    let report = run_agent(provider, device, host, opts, observer, &args.task, ctrl_c()).await;
    if json {
        println!("{}", report_line(&report)?);
    } else {
        eprintln!("\n{}", summary(&report));
    }
    if report.outcome == Outcome::Cancelled {
        // A prompt may still wait on stdin in a blocking thread; the runtime
        // would wait for it on shutdown. Exit now.
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::process::exit(130);
    }
    Ok(exit_code(&report.outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use ap_core::mock::MockDevice;
    use ap_core::{Gesture, Point, Size};
    use ap_llm::{ContentBlock, Request, Response, StopReason, Usage};
    use serde_json::{Value, json};

    struct Scripted(Mutex<VecDeque<Response>>);

    #[async_trait::async_trait]
    impl Provider for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }
        fn default_model(&self) -> &str {
            "claude-opus-5-5"
        }
        async fn complete(&self, _r: Request) -> ap_llm::Result<Response> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| call("done", json!({"summary": "end", "success": false}))))
        }
    }

    fn call(name: &str, input: Value) -> Response {
        Response {
            id: "m".into(),
            model: "claude-opus-5-5".into(),
            content: vec![ContentBlock::ToolUse {
                id: format!("toolu_{name}"),
                name: name.into(),
                input,
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 10,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            stop_details: None,
        }
    }

    struct Answers {
        yes: bool,
    }

    impl Prompter for Answers {
        fn confirm(&self, _q: &str) -> bool {
            self.yes
        }
        fn ask(&self, _q: &str) -> String {
            "blue".into()
        }
    }

    fn mock() -> MockDevice {
        let m = MockDevice::new();
        let mut info = ap_core::Device::info(&m).clone();
        info.screen_pixels = Size::new(117.0, 253.0);
        m.with_info(info)
    }

    fn quick_options() -> AgentOptions {
        AgentOptions {
            max_steps: 10,
            ..AgentOptions::default()
        }
    }

    async fn run_script(script: Vec<Response>, yes: bool) -> (RunReport, MockDevice, Vec<String>) {
        let m = mock();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let l2 = lines.clone();
        let host = TerminalHost::new(Answers { yes }, false).with_log(move |s| {
            l2.lock()
                .unwrap()
                .push(console::strip_ansi_codes(s).to_string())
        });
        let report = run_agent(
            Arc::new(Scripted(Mutex::new(script.into()))),
            Box::new(m.clone()),
            Arc::new(host),
            quick_options(),
            Some(Box::new(ap_agent::ScreenObserver::instant())),
            "test task",
            std::future::pending(),
        )
        .await;
        let l = lines.lock().unwrap().clone();
        (report, m, l)
    }

    #[tokio::test]
    async fn run_with_mock_device() {
        let (r, m, lines) = run_script(
            vec![
                call("tap", json!({"x": 0.5, "y": 0.5})),
                call("ask_user", json!({"question": "Which colour?"})),
                call("done", json!({"summary": "tapped it", "success": true})),
            ],
            true,
        )
        .await;
        assert_eq!(
            r.outcome,
            Outcome::Done {
                summary: "tapped it".into(),
                success: true
            }
        );
        assert_eq!(m.calls(), vec![Gesture::Tap { at: Point::CENTER }]);
        assert!(
            lines.iter().any(|l| l.contains("Task: test task")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("→ tap")), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("Which colour?")),
            "{lines:?}"
        );
        let s = console::strip_ansi_codes(&summary(&r)).to_string();
        assert!(s.starts_with("Done. tapped it"), "{s}");
        assert!(s.contains("3 AI steps") && s.contains("$"), "{s}");
        assert_eq!(exit_code(&r.outcome), 0);
    }

    #[tokio::test]
    async fn unlock_asks_owner_by_hand() {
        let host = TerminalHost::new(Answers { yes: false }, false).with_log(|_| {});
        let req = ConfirmRequest {
            kind: ConfirmKind::Unlock,
            action: "unlock".into(),
            details: None,
        };
        assert!(host.confirm(&req).await);
        let m = mock();
        match host.unlock(&m).await {
            UnlockResult::Unavailable(why) => {
                // Same rule as the system prompt: stop with `done`, no `ask_user`.
                assert!(why.contains("by hand") && why.contains("`done`"));
                assert!(!why.contains("ask_user"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn denied_confirmation_blocks_action() {
        let (r, m, lines) = run_script(
            vec![
                call(
                    "confirm",
                    json!({"action": "Send the message", "tool": "tap", "input": {"x": 0.9, "y": 0.9}}),
                ),
                call("tap", json!({"x": 0.9, "y": 0.9, "irreversible": true})),
                call("done", json!({"summary": "not sent", "success": false})),
            ],
            false,
        )
        .await;
        assert!(m.calls().is_empty(), "{:?}", m.calls());
        assert!(lines.iter().any(|l| l.contains("cannot be undone")));
        assert!(lines.iter().any(|l| l.contains("denied")));
        assert_eq!(exit_code(&r.outcome), 1);
    }

    #[tokio::test]
    async fn ctrl_c_cancels() {
        let m = mock();
        let host = TerminalHost::new(Answers { yes: true }, false).with_log(|_| {});
        let r = run_agent(
            Arc::new(Scripted(Mutex::new(
                vec![call("wait", json!({"seconds": 5}))].into(),
            ))),
            Box::new(m),
            Arc::new(host),
            quick_options(),
            Some(Box::new(ap_agent::ScreenObserver::instant())),
            "wait",
            tokio::time::sleep(std::time::Duration::from_millis(200)),
        )
        .await;
        assert_eq!(r.outcome, Outcome::Cancelled);
        assert_eq!(exit_code(&r.outcome), 130);
    }

    #[test]
    fn overrides() {
        let cfg = Config::default();
        let args = RunArgs {
            task: "x".into(),
            max_steps: Some(0),
            model: Some("claude-haiku-5-5".into()),
            provider: Some("openai".into()),
            #[cfg(feature = "jev")]
            jev: false,
        };
        let o = options(&cfg, &args);
        assert_eq!(o.max_steps, 1);
        assert_eq!(o.model.as_deref(), Some("claude-haiku-5-5"));
        assert!(o.confirm_irreversible);
        let l = llm_config(&cfg, &args).unwrap();
        assert_eq!(
            (l.provider.as_str(), l.model.as_str()),
            ("openai", "claude-haiku-5-5")
        );
    }

    fn run_args(provider: Option<&str>, model: Option<&str>) -> RunArgs {
        RunArgs {
            task: "x".into(),
            max_steps: None,
            model: model.map(Into::into),
            provider: provider.map(Into::into),
            #[cfg(feature = "jev")]
            jev: false,
        }
    }

    #[test]
    fn provider_switch_needs_a_model() {
        // Default config: anthropic / claude-opus-5-5.
        let cfg = Config::default();
        let e = llm_config(&cfg, &run_args(Some("openai"), None)).unwrap_err();
        let (problem, fix) = crate::ui::explain(&e);
        assert!(problem.contains("--model"), "{problem}");
        assert!(fix.unwrap().contains("--model"));
        // Same family: no model needed.
        let l = llm_config(&cfg, &run_args(Some("claude"), None)).unwrap();
        assert_eq!(l.model, "claude-opus-5-5");
        // OpenAI-compatible config, one run with Anthropic.
        let mut o = Config::default();
        o.llm.provider = "ollama".into();
        o.llm.model = "llava".into();
        assert!(llm_config(&o, &run_args(Some("anthropic"), None)).is_err());
        assert!(llm_config(&o, &run_args(Some("openrouter"), None)).is_ok());
        let l = llm_config(&o, &run_args(Some("anthropic"), Some("claude-haiku-5-5"))).unwrap();
        assert_eq!(
            (l.provider.as_str(), l.model.as_str()),
            ("anthropic", "claude-haiku-5-5")
        );
    }

    #[test]
    fn json_report_is_one_line() {
        let r = RunReport {
            outcome: Outcome::Done {
                summary: "ok".into(),
                success: true,
            },
            steps: 2,
            usage: Default::default(),
            transcript: Vec::new(),
        };
        let line = report_line(&r).unwrap();
        assert!(!line.contains('\n'), "{line}");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["event"], "report");
        assert_eq!(v["steps"], 2);
    }
}
