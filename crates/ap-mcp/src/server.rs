//! [`PhoneServer`]: the MCP tool handler.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ap_core::{Device, Error, Frame, Key, Modifiers, Platform, Point, Result};
use ap_screen::{
    DEFAULT_STRIP_OFFSETS, GridOptions, LlmImage, LlmImageOptions, Settler, StripDedup,
    StripLayout, compose_strip, dedup_strip, draw_grid, encode_for_llm, frame_strip,
    resize_max_side, strip_legend,
};
use ap_vision::{Coords, TextBox, TextRecognizer, find_text, format_lines, screen_lines};
use base64::Engine as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use tokio::sync::{Mutex, OnceCell};

use crate::params::*;
use crate::source::DeviceSource;

/// Longest side of a single screenshot sent to the client (upstream: phone points, 874).
const SINGLE_MAX_SIDE: u32 = 874;
/// JPEG quality for strips (upstream 82).
const STRIP_QUALITY: u8 = 82;
/// UI trees longer than this are cut (characters).
const UI_TREE_MAX: usize = 60_000;

/// How the server finds an OCR engine.
#[derive(Clone, Default)]
pub enum OcrSetting {
    /// Use [`ap_vision::default_recognizer`] on first use (needs the `ocr`
    /// feature and downloaded models; otherwise text lines are left out).
    #[default]
    Auto,
    /// Never run OCR.
    Off,
    /// Use this recognizer.
    Recognizer(Arc<dyn TextRecognizer>),
}

impl std::fmt::Debug for OcrSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OcrSetting::Auto => f.write_str("Auto"),
            OcrSetting::Off => f.write_str("Off"),
            OcrSetting::Recognizer(r) => write!(f, "Recognizer({})", r.name()),
        }
    }
}

/// Server settings.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// When the frames of an action's strip are taken. Default +0.1, +0.4, +0.8 s.
    pub strip_offsets: Vec<Duration>,
    pub ocr: OcrSetting,
    /// Text appended to the server instructions (site rules, device notes).
    pub extra_instructions: Option<String>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            strip_offsets: DEFAULT_STRIP_OFFSETS.to_vec(),
            ocr: OcrSetting::Auto,
            extra_instructions: None,
        }
    }
}

struct Inner<S> {
    source: S,
    options: ServerOptions,
    device: Mutex<Option<Arc<dyn Device>>>,
    /// One device action (and its strip) at a time.
    busy: Mutex<()>,
    ocr: OnceCell<std::result::Result<Arc<dyn TextRecognizer>, String>>,
}

/// The MCP server: phone tools over a [`DeviceSource`]. Clones share the
/// device and its state (one per HTTP session, same phone).
pub struct PhoneServer<S: DeviceSource> {
    inner: Arc<Inner<S>>,
    tool_router: ToolRouter<Self>,
}

impl<S: DeviceSource> Clone for PhoneServer<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            tool_router: self.tool_router.clone(),
        }
    }
}

/// What the server could see of the screen.
#[derive(Default)]
struct Observation {
    image: Option<ContentBlock>,
    lines: Vec<String>,
}

impl Observation {
    fn into_result(self, msg: Option<String>) -> CallToolResult {
        let mut text: Vec<String> = msg.into_iter().collect();
        text.extend(self.lines);
        let mut blocks = Vec::new();
        blocks.extend(self.image);
        if !text.is_empty() {
            blocks.push(ContentBlock::text(text.join("\n")));
        }
        CallToolResult::success(blocks)
    }
}

/// An error as a tool result the client shows to the model.
pub(crate) fn error_result(e: &Error) -> CallToolResult {
    let mut msg = format!("error: {e}");
    if e.is_unsupported() {
        msg.push_str("\n(call status to see what this phone's backend can do)");
    }
    CallToolResult::error(vec![ContentBlock::text(msg)])
}

fn image_block(img: &LlmImage) -> ContentBlock {
    ContentBlock::image(
        base64::engine::general_purpose::STANDARD.encode(&img.jpeg),
        LlmImage::MIME,
    )
}

fn point(x: f32, y: f32) -> Result<Point> {
    let p = Point::new(x, y);
    if p.is_valid() {
        Ok(p)
    } else {
        Err(Error::InvalidArgument(format!(
            "({x}, {y}) is outside 0..1: coordinates are normalized fractions of the screen, (0.5, 0.5) is the centre"
        )))
    }
}

fn secs(v: Option<f32>, default: f32, min: f32, max: f32) -> Duration {
    let s = v.filter(|s| s.is_finite()).unwrap_or(default);
    Duration::from_secs_f32(s.clamp(min, max))
}

fn label(d: Duration) -> String {
    format!("+{:.1}s", d.as_secs_f64())
}

fn is_connection_error(e: &Error) -> bool {
    matches!(e, Error::NotConnected(_) | Error::Io(_) | Error::Timeout(_))
}

impl<S: DeviceSource> PhoneServer<S> {
    /// A server with default options.
    pub fn new(source: S) -> Self {
        Self::with_options(source, ServerOptions::default())
    }

    pub fn with_options(source: S, options: ServerOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                source,
                options,
                device: Mutex::new(None),
                busy: Mutex::new(()),
                ocr: OnceCell::new(),
            }),
            tool_router: Self::tool_router(),
        }
    }

    pub fn options(&self) -> &ServerOptions {
        &self.inner.options
    }

    /// The server instructions sent to clients.
    pub fn instructions(&self) -> String {
        crate::instructions::instructions(&self.inner.options)
    }

    /// Names of all tools, in registration order.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    /// The connected device, connecting it on first use.
    pub async fn device(&self) -> Result<Arc<dyn Device>> {
        let mut slot = self.inner.device.lock().await;
        if let Some(d) = slot.as_ref() {
            return Ok(d.clone());
        }
        let d = self.inner.source.connect().await?;
        tracing::info!(id = %d.info().id, backend = %d.info().backend, "device connected");
        *slot = Some(d.clone());
        Ok(d)
    }

    /// Drop the device; the next tool call connects again.
    pub async fn release(&self) -> bool {
        self.inner.device.lock().await.take().is_some()
    }

    /// Run `op` on the device. A lost connection drops the device; for
    /// [`Error::NotConnected`] the server reconnects and retries once.
    async fn run<F, Fut, T>(&self, op: F) -> Result<(Arc<dyn Device>, T)>
    where
        F: Fn(Arc<dyn Device>) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T>> + Send,
        T: Send,
    {
        let d = self.device().await?;
        match op(d.clone()).await {
            Ok(v) => Ok((d, v)),
            Err(Error::NotConnected(why)) => {
                tracing::warn!(%why, "device lost, reconnecting");
                self.release().await;
                let d = self.device().await?;
                let v = op(d.clone()).await?;
                Ok((d, v))
            }
            Err(e) => {
                if is_connection_error(&e) {
                    self.release().await;
                }
                Err(e)
            }
        }
    }

    /// Do one action, then return the screen as `look` says.
    async fn act<F, Fut>(&self, look: Option<Look>, op: F) -> CallToolResult
    where
        F: Fn(Arc<dyn Device>) -> Fut + Send + Sync,
        Fut: Future<Output = Result<String>> + Send,
    {
        let _busy = self.inner.busy.lock().await;
        match self.run(op).await {
            Ok((d, msg)) => self
                .observe(&d, look.unwrap_or(Look::Strip), false)
                .await
                .into_result(Some(msg)),
            Err(e) => error_result(&e),
        }
    }

    /// The OCR engine, loaded once.
    async fn recognizer(&self) -> std::result::Result<Arc<dyn TextRecognizer>, String> {
        self.inner
            .ocr
            .get_or_init(|| async {
                match &self.inner.options.ocr {
                    OcrSetting::Off => Err("OCR is turned off in the server options".to_string()),
                    OcrSetting::Recognizer(r) => Ok(r.clone()),
                    OcrSetting::Auto => {
                        match tokio::task::spawn_blocking(ap_vision::default_recognizer).await {
                            Ok(Ok(r)) => Ok(Arc::from(r)),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(e) => Err(format!("OCR engine failed to load: {e}")),
                        }
                    }
                }
            })
            .await
            .clone()
    }

    /// The screen's text lines as `"x y  text"`, or `None` without OCR.
    async fn text_lines(&self, frame: &Frame) -> Option<String> {
        let rec = self.recognizer().await.ok()?;
        Some(match recognize(rec, frame.clone()).await {
            Ok(boxes) => {
                let lines = screen_lines(&boxes, false);
                if lines.is_empty() {
                    "(no text on screen)".into()
                } else {
                    format_lines(&lines, Coords::Normalized)
                }
            }
            Err(e) => format!("(OCR failed: {e})"),
        })
    }

    async fn observe(&self, d: &Arc<dyn Device>, look: Look, grid: bool) -> Observation {
        match look {
            Look::None => Observation::default(),
            Look::Strip => match self.strip(d, grid).await {
                Ok(o) => o,
                Err(e) => Observation {
                    image: None,
                    lines: vec![format!("error: no picture of the phone ({e})")],
                },
            },
            Look::Both | Look::Image | Look::Text => match self.settled(d, look, grid).await {
                Ok(o) => o,
                Err(e) => Observation {
                    image: None,
                    lines: vec![format!("error: no picture of the phone ({e})")],
                },
            },
        }
    }

    async fn settled(&self, d: &Arc<dyn Device>, look: Look, grid: bool) -> Result<Observation> {
        let settled = Settler::default().settle(d.as_ref(), None).await?;
        let mut lines = Vec::new();
        if !settled.still {
            lines.push("note: the screen was still changing when this was taken".to_string());
        }
        let text = if look == Look::Image {
            None
        } else {
            self.text_lines(&settled.frame).await
        };
        let want_image = look != Look::Text || text.is_none();
        if look == Look::Text && text.is_none() {
            lines.push("(OCR is not available: showing the image instead)".into());
        }
        let image = if want_image {
            let mut small = resize_max_side(&settled.frame, SINGLE_MAX_SIDE);
            if grid {
                draw_grid(&mut small, &GridOptions::default());
            }
            let img = encode_for_llm(
                &small,
                &LlmImageOptions {
                    max_side: 0,
                    ..LlmImageOptions::default()
                },
            )?;
            Some(image_block(&img))
        } else {
            None
        };
        lines.extend(text);
        Ok(Observation { image, lines })
    }

    async fn strip(&self, d: &Arc<dyn Device>, grid: bool) -> Result<Observation> {
        let offsets = &self.inner.options.strip_offsets;
        let frames = frame_strip(d.as_ref(), offsets).await?;
        let (kept, times): (Vec<Frame>, Vec<Duration>) = if frames.len() == offsets.len() {
            dedup_strip(&frames, &StripDedup::default())
                .into_iter()
                .map(|i| (frames[i].clone(), offsets[i]))
                .unzip()
        } else {
            // Some screenshots failed: the times no longer line up, show the newest.
            let last = frames.last().cloned().expect("frame_strip returns frames");
            (
                vec![last],
                vec![offsets.last().copied().unwrap_or_default()],
            )
        };
        let kept: Vec<Frame> = if grid {
            kept.iter()
                .map(|f| {
                    let mut s = resize_max_side(f, SINGLE_MAX_SIDE);
                    draw_grid(&mut s, &GridOptions::default());
                    s
                })
                .collect()
        } else {
            kept
        };
        let labels: Option<Vec<String>> =
            (kept.len() > 1).then(|| times.iter().copied().map(label).collect());
        let strip = compose_strip(&kept, &StripLayout::default(), labels.as_deref());
        let img = encode_for_llm(
            &strip,
            &LlmImageOptions {
                max_side: 0,
                quality: STRIP_QUALITY,
            },
        )?;
        let mut legend = strip_legend(&times);
        if times.len() > 1 {
            legend.push_str("; text from the last");
        }
        let mut lines = vec![legend];
        let last = kept.last().expect("at least one frame");
        lines.extend(self.text_lines(last).await);
        Ok(Observation {
            image: Some(image_block(&img)),
            lines,
        })
    }
}

async fn recognize(rec: Arc<dyn TextRecognizer>, frame: Frame) -> Result<Vec<TextBox>> {
    tokio::task::spawn_blocking(move || rec.recognize(&frame))
        .await
        .map_err(|e| Error::Other(format!("OCR task failed: {e}")))?
}

macro_rules! xy {
    () => {
        " Coordinates are normalized: x and y are fractions of the screen from 0.0 to 1.0, measured from the top-left corner (centre = 0.5, 0.5); in a multi-frame strip, measure inside one frame."
    };
}

macro_rules! confirm {
    () => {
        " Ask the user first if this sends, posts, buys, pays, deletes, or changes an account, privacy or security setting."
    };
}

macro_rules! returns {
    () => {
        " Returns the screen: frames at +0.1/+0.4/+0.8 s in one image (unchanged frames dropped) and, with OCR, text lines \"x y  text\"."
    };
}

#[tool_router]
impl<S: DeviceSource> PhoneServer<S> {
    #[tool(
        description = "The phone's screen now. look=\"both\" (default): one settled screenshot plus OCR text lines \"x y  text\" (normalized 0..1 positions); \"strip\": three frames over 0.8 s to watch something move; \"text\" or \"image\" for one of them. grid=true draws a labelled 0.1 coordinate grid.",
        annotations(read_only_hint = true)
    )]
    async fn look(&self, Parameters(a): Parameters<LookArgs>) -> CallToolResult {
        let _busy = self.inner.busy.lock().await;
        match self.device().await {
            Ok(d) => self
                .observe(&d, a.look.unwrap_or(Look::Both), a.grid.unwrap_or(false))
                .await
                .into_result(None),
            Err(e) => error_result(&e),
        }
    }

    #[tool(
        description = "Wait, then look again: for a page, spinner or result still loading in the last frames. Choose 0.5 to 10 seconds from what you saw.",
        annotations(read_only_hint = true)
    )]
    async fn wait(&self, Parameters(a): Parameters<WaitArgs>) -> CallToolResult {
        tokio::time::sleep(secs(a.seconds, 1.0, 0.0, 10.0)).await;
        let _busy = self.inner.busy.lock().await;
        match self.device().await {
            Ok(d) => self
                .observe(&d, a.look.unwrap_or(Look::Strip), false)
                .await
                .into_result(None),
            Err(e) => error_result(&e),
        }
    }

    #[tool(description = concat!(
        "Tap at (x, y); double=true taps twice.", xy!(), confirm!(), returns!()
    ))]
    async fn tap(&self, Parameters(a): Parameters<TapArgs>) -> CallToolResult {
        let p = match point(a.x, a.y) {
            Ok(p) => p,
            Err(e) => return error_result(&e),
        };
        let double = a.double.unwrap_or(false);
        self.act(a.look, move |d| async move {
            if double {
                d.double_tap(p).await?;
                Ok(format!("double-tapped ({:.3}, {:.3})", p.x, p.y))
            } else {
                d.tap(p).await?;
                Ok(format!("tapped ({:.3}, {:.3})", p.x, p.y))
            }
        })
        .await
    }

    #[tool(description = concat!(
        "Tap visible text by what it says (\"Wi-Fi\", \"General\"), at the exact position OCR reads. ",
        "Text that is not on screen is refused (scroll first). Icons without text are not supported: use look and tap(x, y). ",
        "Needs OCR (cargo feature `ocr` and `agentphone setup ocr`).", confirm!(), returns!()
    ))]
    async fn tap_on(&self, Parameters(a): Parameters<TapOnArgs>) -> CallToolResult {
        let rec = match self.recognizer().await {
            Ok(r) => r,
            Err(why) => {
                return CallToolResult::error(vec![ContentBlock::text(format!(
                    "error: tap_on needs OCR, which is not available: {why}\n\
                     hint: call look to see the screen, then tap(x, y) with normalized 0..1 coordinates."
                ))]);
            }
        };
        let target = a.target;
        self.act(a.look, move |d| {
            let rec = rec.clone();
            let target = target.clone();
            async move {
                let frame = d.screenshot().await?;
                let boxes = recognize(rec, frame).await?;
                match find_text(&boxes, &target, true) {
                    Some(p) => {
                        d.tap(p).await?;
                        Ok(format!("tapped \"{target}\" at ({:.3}, {:.3})", p.x, p.y))
                    }
                    None => {
                        let lines = screen_lines(&boxes, false);
                        let visible = if lines.is_empty() {
                            "(no text on screen)".to_string()
                        } else {
                            format_lines(&lines, Coords::Normalized)
                        };
                        Err(Error::InvalidArgument(format!(
                            "\"{target}\" is not on screen (scroll, or tap(x, y) for icons). Text on screen:\n{visible}"
                        )))
                    }
                }
            }
        })
        .await
    }

    #[tool(description = concat!(
        "Press and hold at (x, y) for `seconds` (default 0.8): context menus, rearranging.", xy!(), confirm!(), returns!()
    ))]
    async fn long_press(&self, Parameters(a): Parameters<LongPressArgs>) -> CallToolResult {
        let p = match point(a.x, a.y) {
            Ok(p) => p,
            Err(e) => return error_result(&e),
        };
        let dur = secs(a.seconds, 0.8, 0.1, 5.0);
        self.act(a.look, move |d| async move {
            d.long_press(p, dur).await?;
            Ok(format!(
                "pressed ({:.3}, {:.3}) for {:.1} s",
                p.x,
                p.y,
                dur.as_secs_f32()
            ))
        })
        .await
    }

    #[tool(description = concat!(
        "Drag from (x0, y0) to (x1, y1) over `seconds` (default 0.3): sliders, pull-down panels, custom gestures. For lists and pages prefer scroll.",
        xy!(), confirm!(), returns!()
    ))]
    async fn swipe(&self, Parameters(a): Parameters<SwipeArgs>) -> CallToolResult {
        let (from, to) = match (point(a.x0, a.y0), point(a.x1, a.y1)) {
            (Ok(f), Ok(t)) => (f, t),
            (Err(e), _) | (_, Err(e)) => return error_result(&e),
        };
        let dur = secs(a.seconds, 0.3, 0.05, 5.0);
        self.act(a.look, move |d| async move {
            d.swipe(from, to, dur).await?;
            Ok(format!(
                "swiped ({:.3}, {:.3}) -> ({:.3}, {:.3})",
                from.x, from.y, to.x, to.y
            ))
        })
        .await
    }

    #[tool(description = concat!(
        "Scroll the content. direction=\"down\" (default) shows what is below, \"up\" what is above, left/right for carousels. ",
        "amount is in half-screens (default 1.0). Optional x, y pick the scrollable area (default the centre).",
        xy!(), returns!()
    ))]
    async fn scroll(&self, Parameters(a): Parameters<ScrollArgs>) -> CallToolResult {
        let at = match (a.x, a.y) {
            (None, None) => Point::CENTER,
            (x, y) => match point(x.unwrap_or(0.5), y.unwrap_or(0.5)) {
                Ok(p) => p,
                Err(e) => return error_result(&e),
            },
        };
        let dir = a.direction.unwrap_or_default();
        let amount = a
            .amount
            .filter(|v| v.is_finite())
            .unwrap_or(1.0)
            .clamp(0.1, 2.0);
        self.act(a.look, move |d| async move {
            d.scroll(at, dir.finger(), amount / 2.0).await?;
            Ok(format!("scrolled {dir:?} by {amount:.1} half-screens").to_lowercase())
        })
        .await
    }

    #[tool(description = concat!(
        "Type text into the focused field (tap the field first). \"\\n\" presses Enter. By default the text replaces what the field holds ",
        "(select all first); replace=false adds to it. The phone may autocorrect: check the result before sending. ",
        "Never type a passcode, PIN or password.", confirm!(), returns!()
    ))]
    async fn type_text(&self, Parameters(a): Parameters<TypeTextArgs>) -> CallToolResult {
        let replace = a.replace.unwrap_or(true);
        let text = a.text;
        self.act(a.look, move |d| {
            let text = text.clone();
            async move {
                let mut notes = Vec::new();
                if replace {
                    let mods = match d.info().platform {
                        Platform::Ios => Modifiers {
                            meta: true,
                            ..Modifiers::NONE
                        },
                        Platform::Android => Modifiers {
                            ctrl: true,
                            ..Modifiers::NONE
                        },
                    };
                    match d.key(&Key::Char('a').with(mods)).await {
                        Ok(()) => {}
                        Err(e) if e.is_unsupported() => {
                            notes.push("note: could not select the old text first (no select-all key on this backend); the text was added");
                        }
                        Err(e) => return Err(e),
                    }
                }
                let mut parts = text.split('\n').peekable();
                while let Some(part) = parts.next() {
                    if !part.is_empty() {
                        d.type_text(part).await?;
                    }
                    if parts.peek().is_some() {
                        d.key(&Key::Enter).await?;
                    }
                }
                let mut msg = format!("typed {} characters", text.chars().count());
                for n in notes {
                    msg.push('\n');
                    msg.push_str(n);
                }
                Ok(msg)
            }
        })
        .await
    }

    #[tool(description = concat!(
        "Press a key or shortcut on the keyboard: enter, escape, backspace, delete, tab, space, arrows, or combos like cmd+a, cmd+space, shift+tab.",
        confirm!(), returns!()
    ))]
    async fn press_key(&self, Parameters(a): Parameters<PressKeyArgs>) -> CallToolResult {
        let Some(key) = Key::parse(&a.combo) else {
            return error_result(&Error::InvalidArgument(format!(
                "unknown key \"{}\": use enter, escape, backspace, delete, tab, space, up, down, left, right, one character, or modifiers like cmd+a, ctrl+shift+left",
                a.combo
            )));
        };
        let combo = a.combo;
        self.act(a.look, move |d| {
            let key = key.clone();
            let combo = combo.clone();
            async move {
                d.key(&key).await?;
                Ok(format!("pressed {combo}"))
            }
        })
        .await
    }

    #[tool(description = concat!(
        "Press a hardware or system button: home, lock, volume_up, volume_down, app_switcher.",
        " There is no unlock: on the lock screen, ask the user to unlock the phone.", returns!()
    ))]
    async fn button(&self, Parameters(a): Parameters<ButtonArgs>) -> CallToolResult {
        let b: ap_core::Button = a.name.into();
        self.act(a.look, move |d| async move {
            d.button(b).await?;
            Ok(format!("pressed {b:?}"))
        })
        .await
    }

    #[tool(description = concat!(
        "Open an app by name (\"Settings\") or by iOS bundle id / Android package name (\"com.apple.Preferences\"). ",
        "Apps reopen where they were left: check the first screen.", returns!()
    ))]
    async fn open_app(&self, Parameters(a): Parameters<OpenAppArgs>) -> CallToolResult {
        let name = a.name;
        self.act(a.look, move |d| {
            let name = name.clone();
            async move {
                let id = crate::apps::resolve(d.as_ref(), &name).await?;
                d.launch_app(&id).await?;
                Ok(format!("opened {id}"))
            }
        })
        .await
    }

    #[tool(description = concat!(
        "System navigation: home, back (a finger sliding in from the left edge), app_switcher, control_center (iPhone; quick settings on Android), notifications.",
        returns!()
    ))]
    async fn navigate(&self, Parameters(a): Parameters<NavigateArgs>) -> CallToolResult {
        let to = a.to;
        self.act(a.look, move |d| async move {
            navigate(d.as_ref(), to).await?;
            Ok(format!("{to:?}").to_lowercase())
        })
        .await
    }

    #[tool(
        description = "Installed apps (iOS bundle ids or Android package names), one per line, when the backend can list them.",
        annotations(read_only_hint = true)
    )]
    async fn list_apps(&self) -> CallToolResult {
        match self.run(|d| async move { d.list_apps().await }).await {
            Ok((_, mut apps)) => {
                apps.sort();
                let text = if apps.is_empty() {
                    "(no apps reported)".to_string()
                } else {
                    apps.join("\n")
                };
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            Err(e) => error_result(&e),
        }
    }

    #[tool(
        description = "The accessibility / UI hierarchy of the current screen as text (XML or JSON), when the backend has one (status lists ui_tree in capabilities). Element frames there are in device points or pixels, not normalized.",
        annotations(read_only_hint = true)
    )]
    async fn ui_tree(&self) -> CallToolResult {
        match self.run(|d| async move { d.ui_tree().await }).await {
            Ok((_, Some(mut tree))) => {
                if tree.len() > UI_TREE_MAX {
                    let mut cut = UI_TREE_MAX;
                    while !tree.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    tree.truncate(cut);
                    tree.push_str("\n... (cut)");
                }
                CallToolResult::success(vec![ContentBlock::text(tree)])
            }
            Ok((_, None)) => CallToolResult::error(vec![ContentBlock::text(
                "error: this backend has no UI tree for the current screen; use look",
            )]),
            Err(e) => error_result(&e),
        }
    }

    #[tool(
        description = "Call when your task on the phone is done: releases the phone (the next tool call connects again)."
    )]
    async fn end_session(&self) -> CallToolResult {
        let _busy = self.inner.busy.lock().await;
        let msg = if self.release().await {
            "session ended: the phone is released"
        } else {
            "no session was open"
        };
        CallToolResult::success(vec![ContentBlock::text(msg)])
    }

    #[tool(
        description = "Connection, device, backend capabilities and OCR state. Call it first if anything fails; it shows what to fix.",
        annotations(read_only_hint = true)
    )]
    async fn status(&self) -> CallToolResult {
        let mut out = vec![
            format!("server: agentphone {}", env!("CARGO_PKG_VERSION")),
            format!("source: {}", self.inner.source.describe()),
        ];
        match self.device().await {
            Ok(d) => {
                let i = d.info();
                out.push(format!(
                    "device: {} ({}, {} {}), id {}",
                    i.name, i.model, i.platform, i.os_version, i.id
                ));
                out.push(format!(
                    "backend: {}; screen {}x{} points, {}x{} pixels",
                    i.backend,
                    i.screen_points.width,
                    i.screen_points.height,
                    i.screen_pixels.width,
                    i.screen_pixels.height
                ));
                out.push(format!(
                    "capabilities: {}",
                    d.capabilities().names().join(", ")
                ));
            }
            Err(e) => {
                out.push("device: not connected".into());
                out.push(format!("error: {e}"));
            }
        }
        match self.recognizer().await {
            Ok(r) => out.push(format!(
                "OCR: on ({}); tap_on and text lines work",
                r.name()
            )),
            Err(why) => out.push(format!(
                "OCR: off ({why}); tap_on is unavailable, read positions from the image"
            )),
        }
        out.push("coordinates: normalized 0..1 from the top-left corner".into());
        CallToolResult::success(vec![ContentBlock::text(out.join("\n"))])
    }
}

/// System gestures.
async fn navigate(d: &dyn Device, to: NavTarget) -> Result<()> {
    let ios = d.info().platform == Platform::Ios;
    let drag = Duration::from_millis(300);
    match to {
        NavTarget::Home => d.home().await,
        NavTarget::AppSwitcher => d.button(ap_core::Button::AppSwitcher).await,
        // Android: KEYCODE_BACK. iOS: back edge swipe (done by the backend).
        NavTarget::Back => d.button(ap_core::Button::Back).await,
        NavTarget::Notifications => {
            let x = if ios { 0.25 } else { 0.5 };
            d.swipe(Point::new(x, 0.0), Point::new(x, 0.6), drag).await
        }
        NavTarget::ControlCenter => {
            if ios {
                d.swipe(Point::new(0.9, 0.0), Point::new(0.9, 0.5), drag)
                    .await
            } else {
                // Android: the first pull shows notifications, the second quick settings.
                d.swipe(Point::new(0.5, 0.0), Point::new(0.5, 0.6), drag)
                    .await?;
                tokio::time::sleep(Duration::from_millis(400)).await;
                d.swipe(Point::new(0.5, 0.0), Point::new(0.5, 0.6), drag)
                    .await
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl<S: DeviceSource> ServerHandler for PhoneServer<S> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("agentphone", env!("CARGO_PKG_VERSION"))
                    .with_title("agentphone: operate a real phone"),
            )
            .with_instructions(self.instructions())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert!(point(0.5, 1.0).is_ok());
        let e = point(1.5, 0.2).unwrap_err().to_string();
        assert!(e.contains("normalized"));
        assert_eq!(
            secs(Some(f32::NAN), 0.3, 0.05, 5.0),
            Duration::from_secs_f32(0.3)
        );
        assert_eq!(secs(Some(99.0), 0.3, 0.05, 5.0), Duration::from_secs(5));
        assert_eq!(label(Duration::from_millis(400)), "+0.4s");
        assert_eq!(Direction::Down.finger(), ap_core::SwipeDirection::Up);
    }
}
