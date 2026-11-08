//! In-process tests: an rmcp client talks to PhoneServer over a duplex pipe,
//! against ap-core's MockDevice.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ap_core::mock::MockDevice;
use ap_core::{
    Button, Capabilities, Device, DeviceInfo, Error, Frame, Gesture, Key, Modifiers, Point, Result,
    SwipeDirection, async_trait,
};
use ap_mcp::{DeviceSource, OcrSetting, PhoneServer, ServerOptions, StaticSource};
use ap_vision::{NormRect, TextBox, TextRecognizer};
use base64::Engine as _;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

struct FakeOcr;

impl TextRecognizer for FakeOcr {
    fn name(&self) -> &str {
        "fake"
    }

    fn recognize(&self, _frame: &Frame) -> Result<Vec<TextBox>> {
        Ok(vec![
            TextBox::new("9:41", NormRect::new(0.05, 0.01, 0.1, 0.02), 1.0),
            TextBox::new("Wi-Fi", NormRect::new(0.25, 0.19, 0.1, 0.02), 1.0),
            TextBox::new("General", NormRect::new(0.25, 0.39, 0.1, 0.02), 1.0),
        ])
    }
}

fn opts(ocr: OcrSetting) -> ServerOptions {
    ServerOptions {
        strip_offsets: vec![Duration::from_millis(5), Duration::from_millis(10)],
        ocr,
        ..ServerOptions::default()
    }
}

async fn client<S: DeviceSource>(server: PhoneServer<S>) -> RunningService<RoleClient, ()> {
    let (a, b) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        if let Ok(s) = server.serve(a).await {
            let _ = s.waiting().await;
        }
    });
    ().serve(b).await.expect("client connects")
}

async fn call(
    c: &RunningService<RoleClient, ()>,
    name: &'static str,
    args: Value,
) -> CallToolResult {
    let mut p = CallToolRequestParams::new(name);
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    c.call_tool(p).await.expect("tool call")
}

fn text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_error(r: &CallToolResult) -> bool {
    r.is_error == Some(true)
}

fn assert_jpeg(r: &CallToolResult) {
    let img = r
        .content
        .iter()
        .find_map(|c| c.as_image())
        .expect("an image block");
    assert_eq!(img.mime_type, "image/jpeg");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&img.data)
        .unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    let frame = Frame::from_encoded(&bytes).unwrap();
    assert_eq!(frame.height, 874, "single frame at phone point height");
}

#[tokio::test]
async fn lists_tools_with_schemas_and_instructions() {
    let c = client(PhoneServer::new(StaticSource::new(MockDevice::new()))).await;
    let tools = c.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "button",
            "end_session",
            "list_apps",
            "long_press",
            "look",
            "navigate",
            "open_app",
            "press_key",
            "scroll",
            "status",
            "swipe",
            "tap",
            "tap_on",
            "type_text",
            "ui_tree",
            "wait"
        ]
    );
    let tap = tools.iter().find(|t| t.name == "tap").unwrap();
    let desc = tap.description.as_deref().unwrap();
    assert!(desc.contains("normalized") && desc.contains("Ask the user first"));
    let schema = Value::Object((*tap.input_schema).clone());
    let req: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(req.contains(&"x") && req.contains(&"y"));
    assert_eq!(schema["properties"]["x"]["minimum"], json!(0.0));
    assert_eq!(schema["properties"]["x"]["maximum"], json!(1.0));
    assert_eq!(
        schema["properties"]["look"]["anyOf"][0]["$ref"]
            .as_str()
            .map(|s| s.ends_with("Look")),
        Some(true),
        "look schema: {}",
        schema["properties"]["look"]
    );
    let button = tools.iter().find(|t| t.name == "button").unwrap();
    let bs = serde_json::to_string(&*button.input_schema).unwrap();
    assert!(bs.contains("volume_up") && bs.contains("app_switcher") && !bs.contains("siri"));
    let look = tools.iter().find(|t| t.name == "look").unwrap();
    assert_eq!(
        look.annotations.as_ref().and_then(|a| a.read_only_hint),
        Some(true)
    );
    let info = c.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "agentphone");
    let ins = info.instructions.as_deref().unwrap();
    assert!(ins.contains("passcode") && ins.contains("end_session") && ins.contains("0.0 to 1.0"));
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn tap_returns_image_and_text() {
    let mock = MockDevice::new();
    let c = client(PhoneServer::with_options(
        StaticSource::new(mock.clone()),
        opts(OcrSetting::Recognizer(Arc::new(FakeOcr))),
    ))
    .await;
    let r = call(&c, "tap", json!({"x": 0.5, "y": 0.25})).await;
    assert!(!is_error(&r), "{}", text(&r));
    assert_eq!(r.content.len(), 2);
    assert!(r.content[0].as_image().is_some(), "image first");
    assert_jpeg(&r);
    let t = text(&r);
    assert!(t.starts_with("tapped (0.500, 0.250)"), "{t}");
    assert!(t.contains("the screen was still (one frame)"), "{t}");
    // OCR lines, status bar dropped.
    assert!(
        t.contains("0.300 0.200  Wi-Fi") && !t.contains("9:41"),
        "{t}"
    );
    assert_eq!(
        mock.calls(),
        vec![Gesture::Tap {
            at: Point::new(0.5, 0.25)
        }]
    );
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn look_without_arguments_and_look_none() {
    let mock = MockDevice::new();
    let c = client(PhoneServer::with_options(
        StaticSource::new(mock.clone()),
        opts(OcrSetting::Off),
    ))
    .await;
    let r = c
        .call_tool(CallToolRequestParams::new("look"))
        .await
        .unwrap();
    assert!(!is_error(&r), "{}", text(&r));
    assert_jpeg(&r);
    let r = call(&c, "look", json!({"look": "image", "grid": true})).await;
    assert_jpeg(&r);
    let r = call(&c, "tap", json!({"x": 0.1, "y": 0.1, "look": "none"})).await;
    assert_eq!(r.content.len(), 1);
    assert_eq!(text(&r), "tapped (0.100, 0.100)");
    let r = call(&c, "wait", json!({"seconds": 0.01})).await;
    assert_jpeg(&r);
    assert!(mock.screenshot_count() >= 3);
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn tap_on_uses_ocr() {
    let mock = MockDevice::new();
    let c = client(PhoneServer::with_options(
        StaticSource::new(mock.clone()),
        opts(OcrSetting::Recognizer(Arc::new(FakeOcr))),
    ))
    .await;
    let r = call(
        &c,
        "tap_on",
        json!({"target": "general row", "look": "none"}),
    )
    .await;
    assert!(!is_error(&r), "{}", text(&r));
    assert_eq!(text(&r), "tapped \"general row\" at (0.300, 0.400)");
    let Gesture::Tap { at } = mock.calls()[0] else {
        panic!("expected a tap")
    };
    assert!((at.x - 0.3).abs() < 1e-5 && (at.y - 0.4).abs() < 1e-5);

    let r = call(&c, "tap_on", json!({"target": "Bluetooth"})).await;
    assert!(is_error(&r));
    let t = text(&r);
    assert!(t.contains("not on screen") && t.contains("Wi-Fi"), "{t}");
    assert_eq!(mock.calls().len(), 1, "nothing tapped");
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn tap_on_without_ocr_explains() {
    let c = client(PhoneServer::with_options(
        StaticSource::new(MockDevice::new()),
        opts(OcrSetting::Auto),
    ))
    .await;
    let r = call(&c, "tap_on", json!({"target": "Wi-Fi"})).await;
    if !ap_vision::OCR_ENABLED {
        assert!(is_error(&r));
        let t = text(&r);
        assert!(
            t.contains("needs OCR") && t.contains("--features ocr") && t.contains("tap(x, y)"),
            "{t}"
        );
        let s = text(&call(&c, "status", json!({})).await);
        assert!(s.contains("OCR: off"), "{s}");
    }
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn argument_errors_are_readable() {
    let mock = MockDevice::new();
    let c = client(PhoneServer::with_options(
        StaticSource::new(mock.clone()),
        opts(OcrSetting::Off),
    ))
    .await;
    let r = call(&c, "tap", json!({"x": 1.5, "y": 0.5})).await;
    assert!(is_error(&r));
    assert!(text(&r).contains("normalized"), "{}", text(&r));
    let r = call(&c, "press_key", json!({"combo": "hyper+x"})).await;
    assert!(is_error(&r) && text(&r).contains("unknown key"));
    let r = call(&c, "open_app", json!({"name": "No Such App"})).await;
    assert!(is_error(&r) && text(&r).contains("list_apps"));
    // Missing required argument: a readable tool error, not a panic.
    let r = c
        .call_tool(CallToolRequestParams::new("tap"))
        .await
        .unwrap();
    assert!(
        is_error(&r) && text(&r).contains("missing field `x`"),
        "{}",
        text(&r)
    );
    assert!(mock.calls().is_empty());
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn actions_map_to_device_calls() {
    let mock = MockDevice::new();
    let c = client(PhoneServer::with_options(
        StaticSource::new(mock.clone()),
        opts(OcrSetting::Off),
    ))
    .await;
    for (tool, args) in [
        ("type_text", json!({"text": "hi\nthere", "look": "none"})),
        ("scroll", json!({"look": "none"})),
        ("button", json!({"name": "volume_up", "look": "none"})),
        ("open_app", json!({"name": "Settings", "look": "none"})),
        ("navigate", json!({"to": "home", "look": "none"})),
        ("press_key", json!({"combo": "cmd+space", "look": "none"})),
        ("long_press", json!({"x": 0.2, "y": 0.3, "look": "none"})),
        (
            "swipe",
            json!({"x0": 0.5, "y0": 0.8, "x1": 0.5, "y1": 0.2, "look": "none"}),
        ),
        ("navigate", json!({"to": "back", "look": "none"})),
    ] {
        let r = call(&c, tool, args).await;
        assert!(!is_error(&r), "{tool}: {}", text(&r));
    }
    let meta = Modifiers {
        meta: true,
        ..Modifiers::NONE
    };
    assert_eq!(
        mock.calls(),
        vec![
            Gesture::Key {
                key: Key::Char('a').with(meta)
            },
            Gesture::TypeText { text: "hi".into() },
            Gesture::Key { key: Key::Enter },
            Gesture::TypeText {
                text: "there".into()
            },
            Gesture::Scroll {
                at: Point::CENTER,
                direction: SwipeDirection::Up,
                amount: 0.5
            },
            Gesture::Button {
                button: Button::VolumeUp
            },
            Gesture::LaunchApp {
                id: "com.apple.Preferences".into()
            },
            Gesture::Button {
                button: Button::Home
            },
            Gesture::Key {
                key: Key::Space.with(meta)
            },
            Gesture::LongPress {
                at: Point::new(0.2, 0.3),
                duration: Duration::from_secs_f32(0.8)
            },
            Gesture::Swipe {
                from: Point::new(0.5, 0.8),
                to: Point::new(0.5, 0.2),
                duration: Duration::from_secs_f32(0.3)
            },
            Gesture::Button {
                button: Button::Back
            },
        ]
    );
    let apps = text(&call(&c, "list_apps", json!({})).await);
    assert!(apps.contains("com.apple.mobilesafari"));
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn unsupported_and_ui_tree() {
    let mock = MockDevice::new().with_capabilities(Capabilities {
        screenshot: true,
        tap: true,
        ..Capabilities::NONE
    });
    let c = client(PhoneServer::with_options(
        StaticSource::new(mock),
        opts(OcrSetting::Off),
    ))
    .await;
    let r = call(
        &c,
        "swipe",
        json!({"x0": 0.1, "y0": 0.1, "x1": 0.2, "y1": 0.2}),
    )
    .await;
    assert!(is_error(&r));
    assert!(text(&r).contains("not supported") && text(&r).contains("status"));
    let r = call(&c, "ui_tree", json!({})).await;
    assert!(is_error(&r));
    c.cancel().await.unwrap();

    let mock = MockDevice::new().with_ui_tree("<XCUIElementTypeApplication/>");
    let c = client(PhoneServer::new(StaticSource::new(mock))).await;
    let r = call(&c, "ui_tree", json!({})).await;
    assert_eq!(text(&r), "<XCUIElementTypeApplication/>");
    c.cancel().await.unwrap();
}

/// Fails to connect with a setup error.
struct NoPhone;

#[async_trait]
impl DeviceSource for NoPhone {
    async fn connect(&self) -> Result<Arc<dyn Device>> {
        Err(Error::setup(
            "usbmuxd is not running",
            "install Apple Devices from the Microsoft Store",
        ))
    }
}

#[tokio::test]
async fn setup_hint_reaches_client() {
    let c = client(PhoneServer::with_options(NoPhone, opts(OcrSetting::Off))).await;
    let r = call(&c, "tap", json!({"x": 0.5, "y": 0.5})).await;
    assert!(is_error(&r));
    let t = text(&r);
    assert!(
        t.contains("usbmuxd is not running") && t.contains("fix: install Apple Devices"),
        "{t}"
    );
    let s = text(&call(&c, "status", json!({})).await);
    assert!(
        s.contains("device: not connected") && s.contains("fix: install Apple Devices"),
        "{s}"
    );
    c.cancel().await.unwrap();
}

/// A mock device whose first tap reports a lost connection.
struct Flaky {
    mock: MockDevice,
    fail_next_tap: Arc<AtomicU32>,
}

#[async_trait]
impl Device for Flaky {
    fn info(&self) -> &DeviceInfo {
        self.mock.info()
    }
    fn capabilities(&self) -> Capabilities {
        self.mock.capabilities()
    }
    async fn screenshot(&self) -> Result<Frame> {
        self.mock.screenshot().await
    }
    async fn tap(&self, at: Point) -> Result<()> {
        if self.fail_next_tap.swap(0, Ordering::SeqCst) > 0 {
            return Err(Error::NotConnected("usb unplugged".into()));
        }
        self.mock.tap(at).await
    }
    async fn double_tap(&self, at: Point) -> Result<()> {
        self.mock.double_tap(at).await
    }
    async fn long_press(&self, at: Point, d: Duration) -> Result<()> {
        self.mock.long_press(at, d).await
    }
    async fn swipe(&self, a: Point, b: Point, d: Duration) -> Result<()> {
        self.mock.swipe(a, b, d).await
    }
    async fn scroll(&self, at: Point, dir: SwipeDirection, amount: f32) -> Result<()> {
        self.mock.scroll(at, dir, amount).await
    }
    async fn type_text(&self, t: &str) -> Result<()> {
        self.mock.type_text(t).await
    }
    async fn key(&self, k: &Key) -> Result<()> {
        self.mock.key(k).await
    }
    async fn button(&self, b: Button) -> Result<()> {
        self.mock.button(b).await
    }
    async fn launch_app(&self, id: &str) -> Result<()> {
        self.mock.launch_app(id).await
    }
}

struct Counting {
    mock: MockDevice,
    fail_next_tap: Arc<AtomicU32>,
    connects: Arc<AtomicU32>,
}

#[async_trait]
impl DeviceSource for Counting {
    async fn connect(&self) -> Result<Arc<dyn Device>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Flaky {
            mock: self.mock.clone(),
            fail_next_tap: self.fail_next_tap.clone(),
        }))
    }
}

#[tokio::test]
async fn reconnects_and_end_session_releases() {
    let mock = MockDevice::new();
    let fail = Arc::new(AtomicU32::new(0));
    let connects = Arc::new(AtomicU32::new(0));
    let c = client(PhoneServer::with_options(
        Counting {
            mock: mock.clone(),
            fail_next_tap: fail.clone(),
            connects: connects.clone(),
        },
        opts(OcrSetting::Off),
    ))
    .await;
    // Lazy: nothing connects before the first call.
    assert_eq!(connects.load(Ordering::SeqCst), 0);
    let r = call(&c, "tap", json!({"x": 0.5, "y": 0.5, "look": "none"})).await;
    assert!(!is_error(&r));
    assert_eq!(connects.load(Ordering::SeqCst), 1);

    fail.store(1, Ordering::SeqCst);
    let r = call(&c, "tap", json!({"x": 0.4, "y": 0.4, "look": "none"})).await;
    assert!(!is_error(&r), "{}", text(&r));
    assert_eq!(connects.load(Ordering::SeqCst), 2, "reconnected once");
    assert_eq!(mock.calls().len(), 2);

    let r = call(&c, "end_session", json!({})).await;
    assert_eq!(text(&r), "session ended: the phone is released");
    let r = call(&c, "end_session", json!({})).await;
    assert_eq!(text(&r), "no session was open");
    let s = text(&call(&c, "status", json!({})).await);
    assert!(
        s.contains("Mock iPhone") && s.contains("capabilities: tap"),
        "{s}"
    );
    assert_eq!(connects.load(Ordering::SeqCst), 3);
    c.cancel().await.unwrap();
}

#[cfg(feature = "http")]
#[tokio::test]
async fn http_transport_initializes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server =
        PhoneServer::with_options(StaticSource::new(MockDevice::new()), opts(OcrSetting::Off));
    let task = tokio::spawn(ap_mcp::serve_http_listener(server, listener));

    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), async {
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if String::from_utf8_lossy(&buf).contains("agentphone") {
                break;
            }
        }
    })
    .await;
    let resp = String::from_utf8_lossy(&buf);
    assert!(read.is_ok(), "timed out: {resp}");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("agentphone"), "{resp}");
    task.abort();

    let refused = ap_mcp::serve_http(
        PhoneServer::new(StaticSource::new(MockDevice::new())),
        "0.0.0.0:0".parse().unwrap(),
    )
    .await;
    assert!(refused.unwrap_err().to_string().contains("loopback"));
}
