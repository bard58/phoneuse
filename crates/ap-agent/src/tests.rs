//! Loop tests: ap-core MockDevice + a scripted fake Provider. No network.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ap_core::mock::MockDevice;
use ap_core::{Gesture, Key, Point, SwipeDirection};
use ap_llm::{ContentBlock, Message, Provider, Request, Response, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::*;

/// Returns scripted responses in order; records every request.
struct Scripted {
    replies: Mutex<VecDeque<Response>>,
    requests: Mutex<Vec<Request>>,
    /// Delay before answering (for cancel tests).
    delay: Duration,
}

impl Scripted {
    fn new(replies: Vec<Response>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
            delay: Duration::ZERO,
        })
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    fn default_model(&self) -> &str {
        "claude-opus-5-5"
    }

    async fn complete(&self, request: Request) -> ap_llm::Result<Response> {
        self.requests.lock().unwrap().push(request);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| call("done", json!({"summary": "script ended", "success": false}))))
    }
}

fn usage() -> Usage {
    Usage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 1000,
    }
}

fn reply(content: Vec<ContentBlock>, stop: StopReason) -> Response {
    Response {
        id: "msg".into(),
        model: "claude-opus-5-5".into(),
        content,
        stop_reason: stop,
        usage: usage(),
        stop_details: None,
    }
}

fn call(name: &str, input: Value) -> Response {
    calls(&[(name, input)])
}

fn calls(list: &[(&str, Value)]) -> Response {
    let content = list
        .iter()
        .enumerate()
        .map(|(i, (n, v))| ContentBlock::ToolUse {
            id: format!("toolu_{n}_{i}"),
            name: n.to_string(),
            input: v.clone(),
        })
        .collect();
    reply(content, StopReason::ToolUse)
}

#[derive(Default)]
struct Host {
    approve: bool,
    answer: String,
    events: Mutex<Vec<AgentEvent>>,
    confirms: Mutex<Vec<ConfirmRequest>>,
    unlocks: Mutex<u32>,
    cancel_on_confirm: Mutex<Option<CancelToken>>,
}

#[async_trait::async_trait]
impl AgentHost for Host {
    fn on_step(&self, e: &AgentEvent) {
        self.events.lock().unwrap().push(e.clone());
    }

    async fn confirm(&self, r: &ConfirmRequest) -> bool {
        self.confirms.lock().unwrap().push(r.clone());
        let token = self.cancel_on_confirm.lock().unwrap().clone();
        if let Some(t) = token {
            t.cancel();
            std::future::pending::<()>().await;
        }
        self.approve
    }

    async fn ask_user(&self, _q: &str) -> String {
        self.answer.clone()
    }

    async fn unlock(&self, _d: &dyn ap_core::Device) -> UnlockResult {
        *self.unlocks.lock().unwrap() += 1;
        UnlockResult::Unlocked
    }
}

/// A mock iPhone with a small screenshot (big debug-mode resizes make tests slow).
fn mock() -> MockDevice {
    let base = MockDevice::new();
    let mut info = ap_core::Device::info(&base).clone();
    info.screen_pixels = ap_core::Size::new(117.0, 253.0);
    base.with_info(info)
}

fn agent(p: Arc<Scripted>, dev: &MockDevice, host: Arc<Host>, opts: AgentOptions) -> Agent {
    Agent::new(p, Box::new(dev.clone()), host, opts)
        .with_observer(Box::new(ScreenObserver::instant()))
}

/// Text of the tool_result blocks in the last user message of a request.
fn last_results(r: &Request) -> Vec<(String, bool)> {
    let m = r.messages.last().unwrap();
    assert_eq!(m.role, Role::User);
    m.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((
                content
                    .iter()
                    .filter_map(|c| match c {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" | "),
                *is_error,
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn tap_then_done() {
    let p = Scripted::new(vec![
        call("tap", json!({"x": 0.5, "y": 0.25})),
        call("done", json!({"summary": "Opened it"})),
    ]);
    let dev = mock();
    let host = Arc::new(Host::default());
    let a = agent(p.clone(), &dev, host.clone(), AgentOptions::default());
    let r = a.run("open settings").await;
    assert_eq!(
        r.outcome,
        Outcome::Done {
            summary: "Opened it".into(),
            success: true
        }
    );
    assert_eq!(r.steps, 2);
    assert_eq!(
        dev.calls(),
        vec![Gesture::Tap {
            at: Point::new(0.5, 0.25)
        }]
    );

    let reqs = p.requests();
    // First request: system prompt, tools, task text + screenshot image.
    let r0 = &reqs[0];
    assert_eq!(r0.system, prompt::SYSTEM_PROMPT);
    assert_eq!(r0.tools, tools::definitions());
    assert!(!r0.parallel_tool_calls && r0.cache);
    assert_eq!(r0.model.as_deref(), Some("claude-opus-5-5"));
    let first = &r0.messages[0].content;
    assert!(matches!(&first[0], ContentBlock::Text { text } if text.contains("open settings")));
    assert!(
        first
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. }))
    );
    // Second request: history is append-only and ends with the tap result + screen.
    let r1 = &reqs[1];
    assert_eq!(r1.messages[..1], r0.messages[..]);
    let Some(ContentBlock::ToolResult { content, .. }) = r1.messages[2].content.first() else {
        panic!("no tool result");
    };
    assert!(
        content
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. }))
    );
    assert!(last_results(r1)[0].0.contains("tapped (0.500, 0.250)"));

    // Usage log and events.
    assert_eq!(r.usage.steps.len(), 2);
    assert_eq!(r.usage.total.cache_read_input_tokens, 2000);
    assert!(r.usage.total_cost_usd.unwrap() > 0.0);
    let ev = host.events.lock().unwrap();
    assert!(matches!(ev.first(), Some(AgentEvent::Started { .. })));
    assert!(matches!(ev.last(), Some(AgentEvent::Finished { .. })));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage { .. })));
}

#[tokio::test]
async fn irreversible_needs_confirm() {
    let p = Scripted::new(vec![
        call("tap", json!({"x": 0.9, "y": 0.9, "irreversible": true})),
        call("confirm", json!({"action": "Send the message to Alex"})),
        call("tap", json!({"x": 0.9, "y": 0.9, "irreversible": true})),
        // Approval is used up: a second irreversible tap is refused again.
        call("tap", json!({"x": 0.9, "y": 0.9, "irreversible": true})),
        call("done", json!({"summary": "sent"})),
    ]);
    let dev = mock();
    let host = Arc::new(Host {
        approve: true,
        ..Host::default()
    });
    let r = agent(p.clone(), &dev, host.clone(), AgentOptions::default())
        .run("send hi")
        .await;
    assert!(matches!(r.outcome, Outcome::Done { .. }));
    assert_eq!(dev.calls().len(), 1, "only the approved tap ran");
    let reqs = p.requests();
    let (t, err) = &last_results(&reqs[1])[0];
    assert!(*err && t.contains("refused"));
    assert!(last_results(&reqs[2])[0].0.contains("approved"));
    assert!(!last_results(&reqs[3])[0].1);
    assert!(last_results(&reqs[4])[0].1);
    let c = host.confirms.lock().unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ConfirmKind::Irreversible);
    assert_eq!(c[0].action, "Send the message to Alex");
}

#[tokio::test]
async fn denied_confirm_blocks_action() {
    let p = Scripted::new(vec![
        call("confirm", json!({"action": "Delete photo"})),
        call("tap", json!({"x": 0.5, "y": 0.5, "irreversible": true})),
    ]);
    let dev = mock();
    let host = Arc::new(Host::default()); // approve = false
    let r = agent(p.clone(), &dev, host, AgentOptions::default())
        .run("delete")
        .await;
    assert!(dev.calls().is_empty());
    assert!(last_results(&p.requests()[1])[0].0.contains("denied"));
    assert!(matches!(r.outcome, Outcome::Done { success: false, .. }));
}

#[tokio::test]
async fn passcode_like_text_is_refused() {
    let p = Scripted::new(vec![call("type_text", json!({"text": "123456"}))]);
    let dev = mock();
    agent(
        p.clone(),
        &dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("x")
    .await;
    assert!(dev.calls().is_empty());
    assert!(last_results(&p.requests()[1])[0].0.contains("passcode"));
}

#[tokio::test]
async fn unlock_rules() {
    // Off in config: refused without asking the host.
    let p = Scripted::new(vec![call("unlock", json!({}))]);
    let host = Arc::new(Host {
        approve: true,
        ..Host::default()
    });
    agent(p.clone(), &mock(), host.clone(), AgentOptions::default())
        .run("x")
        .await;
    assert!(last_results(&p.requests()[1])[0].0.contains("allow_unlock"));
    assert!(host.confirms.lock().unwrap().is_empty());
    assert_eq!(*host.unlocks.lock().unwrap(), 0);

    // On, owner denies: no unlock.
    let opts = AgentOptions {
        allow_unlock: true,
        max_unlocks_per_hour: 1,
        ..AgentOptions::default()
    };
    let p = Scripted::new(vec![call("unlock", json!({}))]);
    let host = Arc::new(Host::default());
    agent(p.clone(), &mock(), host.clone(), opts.clone())
        .run("x")
        .await;
    assert_eq!(host.confirms.lock().unwrap()[0].kind, ConfirmKind::Unlock);
    assert_eq!(*host.unlocks.lock().unwrap(), 0);

    // On, owner approves: one unlock, then the hourly limit refuses.
    let p = Scripted::new(vec![call("unlock", json!({})), call("unlock", json!({}))]);
    let host = Arc::new(Host {
        approve: true,
        ..Host::default()
    });
    agent(p.clone(), &mock(), host.clone(), opts).run("x").await;
    assert_eq!(*host.unlocks.lock().unwrap(), 1);
    let reqs = p.requests();
    assert!(last_results(&reqs[1])[0].0.contains("unlocked"));
    assert!(last_results(&reqs[2])[0].0.contains("attempts"));
}

#[tokio::test]
async fn step_limit() {
    let p = Scripted::new((0..10).map(|_| call("look", json!({}))).collect());
    let opts = AgentOptions {
        max_steps: 3,
        ..AgentOptions::default()
    };
    let r = agent(p.clone(), &mock(), Arc::new(Host::default()), opts)
        .run("x")
        .await;
    assert_eq!(r.outcome, Outcome::StepLimit { steps: 3 });
    assert_eq!(p.requests().len(), 3);
}

#[tokio::test]
async fn cancel_during_confirm() {
    let p = Scripted::new(vec![call("confirm", json!({"action": "Pay"}))]);
    let dev = mock();
    let host = Arc::new(Host::default());
    let a = agent(p.clone(), &dev, host.clone(), AgentOptions::default());
    *host.cancel_on_confirm.lock().unwrap() = Some(a.cancel_token());
    let r = a.run("x").await;
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert_eq!(p.requests().len(), 1);
}

#[tokio::test]
async fn cancel_while_waiting_for_llm() {
    let p = Arc::new(Scripted {
        replies: Mutex::new(VecDeque::new()),
        requests: Mutex::new(Vec::new()),
        delay: Duration::from_secs(30),
    });
    let a = Arc::new(agent(
        p,
        &mock(),
        Arc::new(Host::default()),
        AgentOptions::default(),
    ));
    let t = a.cancel_token();
    let a2 = a.clone();
    let h = tokio::spawn(async move { a2.run("x").await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    t.cancel();
    let r = tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.outcome, Outcome::Cancelled);
}

#[tokio::test]
async fn actions_map_to_device() {
    let p = Scripted::new(vec![
        call("type_text", json!({"text": "hi\nthere"})),
        call("scroll", json!({"direction": "down", "amount": 0.4})),
        call("press_key", json!({"key": "cmd+a"})),
        call("button", json!({"name": "app_switcher"})),
        call("open_app", json!({"app": "Settings"})),
        call("open_app", json!({"app": "Nonexistent Thing"})),
        call("back", json!({})),
        call("home", json!({})),
        call("tap", json!({"x": 2.0, "y": 0.5})),
        call("ask_user", json!({"question": "Which Alex?"})),
        call("done", json!({"summary": "ok"})),
    ]);
    let dev = mock();
    let host = Arc::new(Host {
        answer: "Alex Kim".into(),
        ..Host::default()
    });
    agent(p.clone(), &dev, host, AgentOptions::default())
        .run("x")
        .await;
    let c = dev.calls();
    assert_eq!(c[0], Gesture::TypeText { text: "hi".into() });
    assert_eq!(c[1], Gesture::Key { key: Key::Enter });
    assert_eq!(
        c[2],
        Gesture::TypeText {
            text: "there".into()
        }
    );
    // Content down = finger up.
    assert!(matches!(
        c[3],
        Gesture::Scroll {
            direction: SwipeDirection::Up,
            ..
        }
    ));
    assert!(matches!(c[4], Gesture::Key { .. }));
    assert!(matches!(c[5], Gesture::Button { .. }));
    assert_eq!(
        c[6],
        Gesture::LaunchApp {
            id: "com.apple.Preferences".into()
        }
    );
    assert_eq!(
        c[7],
        Gesture::Button {
            button: ap_core::Button::Back
        }
    );
    assert!(matches!(c[8], Gesture::Button { .. }));
    assert_eq!(c.len(), 9, "bad open_app and out-of-range tap did nothing");
    let reqs = p.requests();
    assert!(last_results(&reqs[6])[0].1, "unknown app is an error");
    assert!(last_results(&reqs[9])[0].0.contains("fractions"));
    assert!(last_results(&reqs[10])[0].0.contains("Alex Kim"));
}

#[tokio::test]
async fn nudges_then_stops_and_handles_truncation() {
    let text = |t: &str| reply(vec![ContentBlock::text(t)], StopReason::EndTurn);
    let mut cut = call("tap", json!({"x": 0.5}));
    cut.stop_reason = StopReason::MaxTokens;
    let p = Scripted::new(vec![cut, text("hmm"), text("still"), text("bye")]);
    let dev = mock();
    let r = agent(
        p.clone(),
        &dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("x")
    .await;
    assert_eq!(r.outcome, Outcome::NoAction { text: "bye".into() });
    assert!(dev.calls().is_empty(), "truncated call not run");
    let reqs = p.requests();
    assert!(last_results(&reqs[1])[0].0.contains("output limit"));
    assert!(matches!(
        reqs[2].messages.last().unwrap().content[0],
        ContentBlock::Text { .. }
    ));
}

#[tokio::test]
async fn refusal_stops() {
    let mut r = reply(vec![], StopReason::Refusal);
    r.stop_details = Some("cyber".into());
    let p = Scripted::new(vec![r]);
    let out = agent(
        p,
        &mock(),
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("x")
    .await;
    assert_eq!(
        out.outcome,
        Outcome::Refused {
            details: Some("cyber".into())
        }
    );
}

#[tokio::test]
async fn parallel_calls_one_screen() {
    let p = Scripted::new(vec![
        calls(&[
            ("tap", json!({"x": 0.1, "y": 0.1})),
            ("tap", json!({"x": 0.2, "y": 0.2})),
        ]),
        call("done", json!({"summary": "ok"})),
    ]);
    let dev = mock();
    agent(
        p.clone(),
        &dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("x")
    .await;
    assert_eq!(dev.calls().len(), 2);
    let reqs = p.requests();
    let m: &Message = reqs[1].messages.last().unwrap();
    let images: Vec<usize> = m
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::ToolResult { content, .. } => content
                .iter()
                .filter(|c| matches!(c, ContentBlock::Image { .. }))
                .count(),
            _ => 0,
        })
        .collect();
    assert_eq!(images, vec![0, 1]);
}

#[test]
fn options_from_config() {
    let mut cfg = ap_core::Config::default();
    cfg.safety.allow_unlock = true;
    cfg.safety.max_steps = 7;
    let o = AgentOptions::from_config(&cfg);
    assert_eq!(o.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(o.max_steps, 7);
    assert!(o.allow_unlock && o.confirm_irreversible);
}
