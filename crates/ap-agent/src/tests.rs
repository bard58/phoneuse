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
        call(
            "confirm",
            json!({"action": "Send the message to Alex", "tool": "tap", "input": {"x": 0.9, "y": 0.9}}),
        ),
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
    assert!(
        c[0].details
            .as_deref()
            .is_some_and(|d| d.contains("Next step: tap at (0.900, 0.900)"))
    );
}

#[tokio::test]
async fn denied_confirm_blocks_action() {
    let p = Scripted::new(vec![
        call(
            "confirm",
            json!({"action": "Delete photo", "tool": "tap", "input": {"x": 0.5, "y": 0.5}}),
        ),
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
    let p = Scripted::new(vec![call(
        "confirm",
        json!({"action": "Pay", "tool": "tap", "input": {"x": 0.5, "y": 0.9}}),
    )]);
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
    assert_eq!(r.usage.cancelled_calls, 1);
    assert!(r.usage.summary().contains("lower bound"));
}

#[tokio::test]
async fn actions_map_to_device() {
    let p = Scripted::new(vec![
        call(
            "confirm",
            json!({"action": "Send hi there", "tool": "type_text", "input": {"text": "hi\nthere"}}),
        ),
        call(
            "type_text",
            json!({"text": "hi\nthere", "irreversible": true}),
        ),
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
        approve: true,
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
    assert!(last_results(&reqs[7])[0].1, "unknown app is an error");
    assert!(last_results(&reqs[10])[0].0.contains("fractions"));
    assert!(last_results(&reqs[11])[0].0.contains("Alex Kim"));
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

/// A mock with knobs: slow typing, one failing screenshot, a foreground app.
#[derive(Clone)]
struct TestDevice {
    inner: MockDevice,
    type_delay: Duration,
    /// 1-based number of the screenshot that fails.
    fail_screenshot: Option<u32>,
    shots: Arc<Mutex<u32>>,
    foreground: Option<String>,
    /// How many times the agent called `Device::cancel`.
    cancels: Arc<Mutex<u32>>,
    /// A tap fails with a setup error and turns touch off (as an iPhone on
    /// iOS 27 that refuses the screen stream).
    tap_loses_touch: bool,
    touch_lost: Arc<Mutex<bool>>,
}

impl TestDevice {
    fn new(inner: &MockDevice) -> Self {
        Self {
            inner: inner.clone(),
            type_delay: Duration::ZERO,
            fail_screenshot: None,
            shots: Arc::default(),
            foreground: None,
            cancels: Arc::default(),
            tap_loses_touch: false,
            touch_lost: Arc::default(),
        }
    }
}

#[async_trait::async_trait]
impl ap_core::Device for TestDevice {
    fn info(&self) -> &ap_core::DeviceInfo {
        self.inner.info()
    }
    fn capabilities(&self) -> ap_core::Capabilities {
        let c = self.inner.capabilities();
        if *self.touch_lost.lock().unwrap() {
            ap_core::Capabilities {
                tap: false,
                swipe: false,
                ..c
            }
        } else {
            c
        }
    }
    fn cancel(&self) {
        *self.cancels.lock().unwrap() += 1;
    }
    async fn screenshot(&self) -> ap_core::Result<ap_core::Frame> {
        let n = {
            let mut s = self.shots.lock().unwrap();
            *s += 1;
            *s
        };
        if self.fail_screenshot == Some(n) {
            return Err(ap_core::Error::Other("capture failed".into()));
        }
        self.inner.screenshot().await
    }
    async fn tap(&self, at: Point) -> ap_core::Result<()> {
        if self.tap_loses_touch {
            *self.touch_lost.lock().unwrap() = true;
            return Err(ap_core::Error::setup(
                "Touch input is off: the iPhone did not open the screen stream that touch needs (error 9021).",
                "Run `agentphone doctor`",
            ));
        }
        self.inner.tap(at).await
    }
    async fn double_tap(&self, at: Point) -> ap_core::Result<()> {
        self.inner.double_tap(at).await
    }
    async fn long_press(&self, at: Point, d: Duration) -> ap_core::Result<()> {
        self.inner.long_press(at, d).await
    }
    async fn swipe(&self, a: Point, b: Point, d: Duration) -> ap_core::Result<()> {
        self.inner.swipe(a, b, d).await
    }
    async fn scroll(&self, at: Point, dir: SwipeDirection, amount: f32) -> ap_core::Result<()> {
        self.inner.scroll(at, dir, amount).await
    }
    async fn type_text(&self, text: &str) -> ap_core::Result<()> {
        if !self.type_delay.is_zero() {
            tokio::time::sleep(self.type_delay).await;
        }
        self.inner.type_text(text).await
    }
    async fn key(&self, key: &Key) -> ap_core::Result<()> {
        self.inner.key(key).await
    }
    async fn button(&self, b: ap_core::Button) -> ap_core::Result<()> {
        self.inner.button(b).await
    }
    async fn launch_app(&self, id: &str) -> ap_core::Result<()> {
        self.inner.launch_app(id).await
    }
    async fn foreground_app(&self) -> ap_core::Result<Option<String>> {
        Ok(self.foreground.clone())
    }
}

fn agent_on(p: Arc<Scripted>, dev: TestDevice, host: Arc<Host>, opts: AgentOptions) -> Agent {
    Agent::new(p, Box::new(dev), host, opts).with_observer(Box::new(ScreenObserver::instant()))
}

fn confirm_call(action: &str, tool: &str, input: Value) -> Response {
    call(
        "confirm",
        json!({"action": action, "tool": tool, "input": input}),
    )
}

#[tokio::test]
async fn denial_blocks_later_unflagged_actions() {
    let p = Scripted::new(vec![
        confirm_call("Send the message", "tap", json!({"x": 0.9, "y": 0.9})),
        // No irreversible flag: still refused after the denial.
        call("tap", json!({"x": 0.9, "y": 0.9})),
        call("home", json!({})),
        call("ask_user", json!({"question": "What now?"})),
        call("tap", json!({"x": 0.5, "y": 0.5})),
        call("done", json!({"summary": "ok"})),
    ]);
    let dev = mock();
    let host = Arc::new(Host {
        answer: "Open Settings instead".into(),
        ..Host::default()
    });
    agent(p.clone(), &dev, host, AgentOptions::default())
        .run("x")
        .await;
    let reqs = p.requests();
    let (t, err) = &last_results(&reqs[2])[0];
    assert!(*err && t.contains("denied"), "{t}");
    assert!(!last_results(&reqs[3])[0].1, "home still runs");
    assert!(
        !last_results(&reqs[5])[0].1,
        "the owner's answer ends the block"
    );
    assert_eq!(
        dev.calls(),
        vec![
            Gesture::Button {
                button: ap_core::Button::Home
            },
            Gesture::Tap {
                at: Point::new(0.5, 0.5)
            }
        ]
    );
}

#[tokio::test]
async fn approval_is_bound_to_one_call() {
    let p = Scripted::new(vec![
        confirm_call(
            "Send 'On my way'",
            "type_text",
            json!({"text": "On my way\n"}),
        ),
        call(
            "type_text",
            json!({"text": "different text\n", "irreversible": true}),
        ),
        // The mismatch used up the approval.
        call(
            "type_text",
            json!({"text": "On my way\n", "irreversible": true}),
        ),
        confirm_call("Send", "tap", json!({"x": 0.9, "y": 0.9})),
        call("look", json!({})),
        // `look` cleared the approval.
        call("tap", json!({"x": 0.9, "y": 0.9, "irreversible": true})),
        call("done", json!({"summary": "ok"})),
    ]);
    let dev = mock();
    let host = Arc::new(Host {
        approve: true,
        ..Host::default()
    });
    agent(p.clone(), &dev, host, AgentOptions::default())
        .run("x")
        .await;
    assert!(dev.calls().is_empty());
    let reqs = p.requests();
    assert!(
        last_results(&reqs[2])[0]
            .0
            .contains("not the approved action")
    );
    assert!(last_results(&reqs[3])[0].0.contains("no approval"));
    assert!(last_results(&reqs[6])[0].0.contains("no approval"));
}

#[tokio::test]
async fn host_classifier_requires_confirm() {
    let p = Scripted::new(vec![
        call("type_text", json!({"text": "hi\n"})),
        call("press_key", json!({"key": "enter"})),
        call("press_key", json!({"key": "cmd+return"})),
        call("type_text", json!({"text": "hi"})),
        // Right after typing, the app in front is unknown: needs confirm.
        call("tap", json!({"x": 0.9, "y": 0.9})),
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
    assert_eq!(dev.calls(), vec![Gesture::TypeText { text: "hi".into() }]);
    let reqs = p.requests();
    assert!(last_results(&reqs[1])[0].0.contains("newline"));
    assert!(last_results(&reqs[2])[0].0.contains("Enter"));
    assert!(last_results(&reqs[3])[0].0.contains("Enter"));
    assert!(last_results(&reqs[5])[0].0.contains("unknown"));

    // In a messaging app the tap after typing needs confirm; in Notes it does not.
    for (app, runs) in [
        ("com.apple.MobileSMS", false),
        ("com.apple.mobilenotes", true),
    ] {
        let p = Scripted::new(vec![
            call("type_text", json!({"text": "hi"})),
            call("tap", json!({"x": 0.9, "y": 0.9})),
            call("done", json!({"summary": "ok"})),
        ]);
        let base = mock();
        let mut dev = TestDevice::new(&base);
        dev.foreground = Some(app.into());
        agent_on(
            p.clone(),
            dev,
            Arc::new(Host::default()),
            AgentOptions::default(),
        )
        .run("x")
        .await;
        assert_eq!(base.calls().len(), if runs { 2 } else { 1 }, "{app}");
    }
}

#[tokio::test]
async fn split_and_padded_digits_are_refused() {
    let p = Scripted::new(vec![
        call("press_key", json!({"key": "4"})),
        call("press_key", json!({"key": "8"})),
        call("press_key", json!({"key": "2"})),
        call("press_key", json!({"key": "1"})),
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
    assert_eq!(dev.calls().len(), 3);
    assert!(
        last_results(&p.requests()[4])[0]
            .0
            .contains("4 digits in a row")
    );

    let p = Scripted::new(vec![
        call("type_text", json!({"text": "48"})),
        call("tap", json!({"x": 0.5, "y": 0.5})),
        call("type_text", json!({"text": "21"})),
        call("type_text", json!({"text": "hello"})),
        call("type_text", json!({"text": "4821 "})),
        call(
            "type_text",
            json!({"text": "\u{ff14}\u{ff18}\u{ff12}\u{ff11}"}),
        ),
        call("done", json!({"summary": "ok"})),
    ]);
    let base = mock();
    let mut dev = TestDevice::new(&base);
    dev.foreground = Some("com.apple.mobilenotes".into());
    agent_on(
        p.clone(),
        dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("x")
    .await;
    let reqs = p.requests();
    assert!(last_results(&reqs[3])[0].0.contains("passcode"));
    assert!(
        !last_results(&reqs[4])[0].1,
        "plain text runs and ends the run"
    );
    assert!(last_results(&reqs[5])[0].0.contains("passcode"));
    assert!(last_results(&reqs[6])[0].0.contains("passcode"));
    assert_eq!(
        base.calls()
            .iter()
            .filter(|g| matches!(g, Gesture::TypeText { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn passcode_needs_owner_even_with_confirmations_off() {
    let opts = AgentOptions {
        confirm_irreversible: false,
        ..AgentOptions::default()
    };
    let script = || {
        Scripted::new(vec![
            call("type_text", json!({"text": "123456"})),
            confirm_call("Type the code", "type_text", json!({"text": "123456"})),
            call("type_text", json!({"text": "123456"})),
            call("done", json!({"summary": "ok"})),
        ])
    };
    // The owner denies: never typed, although confirmations are off.
    let p = script();
    let dev = mock();
    let host = Arc::new(Host::default());
    agent(p.clone(), &dev, host.clone(), opts.clone())
        .run("x")
        .await;
    assert!(dev.calls().is_empty());
    assert_eq!(
        host.confirms.lock().unwrap().len(),
        1,
        "the owner was asked"
    );
    assert!(last_results(&p.requests()[2])[0].0.contains("denied"));

    // The owner approves: typed once.
    let p = script();
    let dev = mock();
    let host = Arc::new(Host {
        approve: true,
        ..Host::default()
    });
    agent(p.clone(), &dev, host, opts.clone()).run("x").await;
    assert_eq!(
        dev.calls(),
        vec![Gesture::TypeText {
            text: "123456".into()
        }]
    );

    // A non-digit confirm with confirmations off does not ask the owner.
    let p = Scripted::new(vec![
        confirm_call("Send", "tap", json!({"x": 0.9, "y": 0.9})),
        call("tap", json!({"x": 0.9, "y": 0.9, "irreversible": true})),
    ]);
    let dev = mock();
    let host = Arc::new(Host::default());
    agent(p, &dev, host.clone(), opts).run("x").await;
    assert!(host.confirms.lock().unwrap().is_empty());
    assert_eq!(dev.calls().len(), 1);
}

/// Adds lock-screen text to every observation (like an OCR observer would).
struct LockObserver(ScreenObserver);

#[async_trait::async_trait]
impl Observer for LockObserver {
    async fn look(&self, d: &dyn ap_core::Device) -> ap_core::Result<Observation> {
        let mut o = self.0.look(d).await?;
        o.text.push_str("\nScreen text: Enter Passcode | Emergency");
        Ok(o)
    }
    async fn after_action(&self, d: &dyn ap_core::Device) -> ap_core::Result<Observation> {
        let mut o = self.0.after_action(d).await?;
        o.text.push_str("\nScreen text: Enter Passcode | Emergency");
        Ok(o)
    }
}

#[tokio::test]
async fn lock_screen_blocks_text_and_keys() {
    let p = Scripted::new(vec![
        call("type_text", json!({"text": "hello"})),
        call("press_key", json!({"key": "a"})),
        call("tap", json!({"x": 0.5, "y": 0.5})),
        call("done", json!({"summary": "ok"})),
    ]);
    let dev = mock();
    Agent::new(
        p.clone(),
        Box::new(dev.clone()),
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .with_observer(Box::new(LockObserver(ScreenObserver::instant())))
    .run("x")
    .await;
    let reqs = p.requests();
    assert!(last_results(&reqs[1])[0].0.contains("looks locked"));
    assert!(last_results(&reqs[2])[0].0.contains("looks locked"));
    assert_eq!(dev.calls().len(), 1, "only the tap ran");

    // The model called `unlock` and it did not succeed: typing stays blocked.
    let p = Scripted::new(vec![
        call("unlock", json!({})),
        call("type_text", json!({"text": "hello"})),
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
    assert!(dev.calls().is_empty());
    assert!(last_results(&p.requests()[2])[0].0.contains("looks locked"));
}

#[tokio::test]
async fn events_redact_digit_text() {
    let p = Scripted::new(vec![
        call("type_text", json!({"text": "123456"})),
        call("done", json!({"summary": "ok"})),
    ]);
    let host = Arc::new(Host::default());
    agent(p, &mock(), host.clone(), AgentOptions::default())
        .run("x")
        .await;
    let ev = host.events.lock().unwrap();
    let input = ev
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall { name, input, .. } if name == "type_text" => Some(input.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(input["text"], "[6 digits, redacted]");
    assert!(!format!("{ev:?}").contains("123456"));
}

#[tokio::test]
async fn cancel_during_type_text_never_presses_enter() {
    let opts = AgentOptions {
        confirm_irreversible: false,
        ..AgentOptions::default()
    };
    let p = Scripted::new(vec![call(
        "type_text",
        json!({"text": "I quit, effective today\n"}),
    )]);
    let base = mock();
    let mut dev = TestDevice::new(&base);
    dev.type_delay = Duration::from_secs(30);
    let cancels = dev.cancels.clone();
    let a = Arc::new(agent_on(p, dev, Arc::new(Host::default()), opts));
    let t = a.cancel_token();
    let a2 = a.clone();
    let h = tokio::spawn(async move { a2.run("x").await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    t.cancel();
    let r = tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .expect("cancel stops the action at once")
        .unwrap();
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert!(
        !base
            .calls()
            .iter()
            .any(|g| matches!(g, Gesture::Key { key: Key::Enter })),
        "Enter was pressed after take-over"
    );
    assert!(
        *cancels.lock().unwrap() >= 1,
        "the device was not told to stop the call it already received"
    );
}

#[tokio::test]
async fn failed_screenshot_forces_a_look() {
    let p = Scripted::new(vec![
        call("tap", json!({"x": 0.1, "y": 0.1})),
        call("tap", json!({"x": 0.2, "y": 0.2})),
        call("look", json!({})),
        call("tap", json!({"x": 0.3, "y": 0.3})),
        call("done", json!({"summary": "ok"})),
    ]);
    let base = mock();
    let mut dev = TestDevice::new(&base);
    // 1 = the first look of the run, 2 = the screen after the first tap.
    dev.fail_screenshot = Some(2);
    agent_on(
        p.clone(),
        dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("x")
    .await;
    let reqs = p.requests();
    let (t, err) = &last_results(&reqs[1])[0];
    assert!(*err && t.contains("no screenshot"), "{t}");
    assert!(last_results(&reqs[2])[0].0.contains("Call `look` first"));
    assert!(!last_results(&reqs[3])[0].1);
    assert!(!last_results(&reqs[4])[0].1);
    assert_eq!(base.calls().len(), 2, "the refused tap did not run");
}

#[tokio::test]
async fn empty_turn_gets_a_placeholder() {
    let p = Scripted::new(vec![
        reply(vec![], StopReason::EndTurn),
        call("done", json!({"summary": "ok"})),
    ]);
    agent(
        p.clone(),
        &mock(),
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("x")
    .await;
    let reqs = p.requests();
    let m = &reqs[1].messages[1];
    assert_eq!(m.role, Role::Assistant);
    assert_eq!(m.content, vec![ContentBlock::text("(no response)")]);
}

/// An iPhone below iOS 27 has no touch input: the run stops before the
/// first LLM call with the update message, and it sends no tap.
#[tokio::test]
async fn no_touch_input_stops_before_the_first_llm_call() {
    let dev = mock().with_capabilities(ap_core::Capabilities {
        tap: false,
        swipe: false,
        ..ap_core::Capabilities::ALL
    });
    let p = Scripted::new(vec![call("tap", json!({"x": 0.5, "y": 0.5}))]);
    let out = agent(
        p.clone(),
        &dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("open Settings")
    .await;
    let Outcome::Failed { error } = &out.outcome else {
        panic!("{:?}", out.outcome);
    };
    assert!(error.starts_with("screen only, no touch input:"), "{error}");
    assert!(
        error.contains("Touch input needs iOS 27 or later. This iPhone has iOS 26.0.")
            && error.contains("Software Update"),
        "{error}"
    );
    assert_eq!(out.steps, 0);
    assert!(p.requests().is_empty());
    assert!(dev.calls().is_empty());
}

/// An iPhone on iOS 27 that refuses the screen stream: the first tap fails
/// and turns touch off. The run ends after that tool result, with no second
/// LLM call, so the model cannot loop on taps that fail.
#[tokio::test]
async fn lost_touch_ends_the_run_after_the_failed_action() {
    let mut info = ap_core::Device::info(&mock()).clone();
    info.os_version = "27.0".into();
    let mut dev = TestDevice::new(&mock().with_info(info));
    dev.tap_loses_touch = true;
    let lost = dev.touch_lost.clone();
    let p = Scripted::new(vec![
        call("tap", json!({"x": 0.5, "y": 0.5})),
        call("tap", json!({"x": 0.5, "y": 0.6})),
    ]);
    let out = agent_on(
        p.clone(),
        dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("open Settings")
    .await;
    let Outcome::Failed { error } = &out.outcome else {
        panic!("{:?}", out.outcome);
    };
    assert!(error.starts_with("screen only, no touch input:"), "{error}");
    assert!(error.contains(ap_core::IOS_TOUCH_OFF_MESSAGE), "{error}");
    assert!(error.contains("error 9021"), "{error}");
    assert!(*lost.lock().unwrap());
    assert_eq!(out.steps, 1);
    assert_eq!(p.requests().len(), 1);
    // The failed tap is in the transcript as an error result.
    let m = out.transcript.last().unwrap();
    assert!(matches!(
        m.content.first(),
        Some(ContentBlock::ToolResult { is_error: true, .. })
    ));
}

/// A failed action on a phone that still has touch does not end the run.
#[tokio::test]
async fn a_failed_action_with_touch_keeps_the_run() {
    let dev = TestDevice::new(&mock().with_capabilities(ap_core::Capabilities {
        launch_app: false,
        ..ap_core::Capabilities::ALL
    }));
    let p = Scripted::new(vec![
        call("open_app", json!({"app": "com.apple.Preferences"})),
        call("done", json!({"summary": "ok", "success": true})),
    ]);
    let out = agent_on(
        p.clone(),
        dev,
        Arc::new(Host::default()),
        AgentOptions::default(),
    )
    .run("open Settings")
    .await;
    assert!(
        matches!(out.outcome, Outcome::Done { success: true, .. }),
        "{:?}",
        out.outcome
    );
    assert_eq!(p.requests().len(), 2);
}
