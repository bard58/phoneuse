//! HidBackend: Route B (iOS 17.4+).
//!
//! * Tunnel: CoreDeviceProxy over USB/Wi-Fi lockdown, or RemotePairing over
//!   Wi-Fi, then jktcp (userspace TCP, no admin) and RSD.
//! * Touch: `UniversalHidServiceClient` (normalized u16 0..=65535).
//! * Keys and buttons: `IndigoHidClient` (HID page 0x07 / 0x0C).
//! * Screenshot: DVT `ScreenshotClient` (`com.apple.instruments.dtservicehub`).
//! * Apps: DVT process control (launch), installation_proxy (list).
//!
//! AUTH GATE: the device drops synthetic HID events unless a displayservice
//! media stream is active (idevice `core_device/hid.rs` module doc). The
//! backend opens ONE stream per session ([`AuthGate`]) and keeps it open. The
//! device then sends RTP (audio + HEVC video) to two UDP ports in the
//! userspace stack. jktcp buffers unread datagrams without limit, so a
//! background task per port reads and drops them.
//!
//! Reconnect: a transport error closes the session. The call opens a new
//! session and runs once more. A gesture that failed mid-way can run twice.
//!
//! UNVERIFIED: nothing here ran on a device. The API use matches idevice
//! v0.1.68 `tools/src/hid.rs` and the spike in `spikes/ios-hid`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ap_core::{Button, Capabilities, Error, Frame, Key, Point, Result};
use async_trait::async_trait;
use idevice::{
    IdeviceService, ReadWrite, RsdService,
    core_device::{
        CallInfoBlob, DisplayServiceClient, GENERAL_PASTEBOARD, PasteboardServiceClient,
        build_screen_audio_offer, build_screen_video_offer, build_start_audio_parameters,
        build_start_video_parameters,
        hid::{
            ButtonState, IndigoHidClient, TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE,
            TouchscreenContact, UniversalHidServiceClient,
        },
    },
    dvt::{
        process_control::ProcessControlClient, remote_server::RemoteServerClient,
        screenshot::ScreenshotClient,
    },
    installation_proxy::InstallationProxyClient,
    provider::IdeviceProvider,
    tcp::handle::UdpSocketHandle,
};
use tokio::task::JoinHandle;

use crate::backend::IosBackend;
use crate::coords::{SAMPLE_INTERVAL, drag_path, steps_for, to_hid};
use crate::errors::{DDI_HINT, ctx, is_transport};
use crate::keymap::{self, Stroke};
use crate::tunnel::{self, Tunnel};

/// `clientSupportedFeatures` that Xcode Device Hub sends for screen sharing.
const CLIENT_SUPPORTED_FEATURES: u64 = 140;
/// Wait after the stream starts before HID events are accepted.
const GATE_SETTLE: Duration = Duration::from_millis(300);
/// Gap between key strokes.
const KEY_GAP: Duration = Duration::from_millis(12);
/// Gap between the two taps of a double tap.
const DOUBLE_TAP_GAP: Duration = Duration::from_millis(90);
/// Resend interval for a held contact (long press).
const HOLD_RESEND: Duration = Duration::from_millis(100);

/// Where the tunnel comes from. Kept so the backend can reconnect.
#[derive(Clone)]
pub(crate) enum TunnelSource {
    /// CoreDeviceProxy over a lockdown provider (USB, or Wi-Fi TCP 62078).
    Lockdown(Arc<dyn IdeviceProvider>),
    /// RemotePairing to `_remotepairing._tcp`.
    RemotePairing { addr: SocketAddr, path: PathBuf },
}

impl TunnelSource {
    async fn open(&self) -> Result<Tunnel> {
        match self {
            TunnelSource::Lockdown(p) => Box::pin(tunnel::open_core_device_proxy(&**p)).await,
            TunnelSource::RemotePairing { addr, path } => {
                Box::pin(tunnel::open_remote_pairing(
                    *addr,
                    &tunnel::host_name(),
                    path,
                ))
                .await
            }
        }
    }
}

/// The open displayservice stream that lets HID events through.
pub(crate) struct AuthGate {
    client: DisplayServiceClient<Box<dyn ReadWrite>>,
    drains: Vec<JoinHandle<()>>,
    dropped_bytes: Arc<AtomicU64>,
}

fn spawn_drain(sock: UdpSocketHandle, counter: Arc<AtomicU64>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Owning the socket keeps the port bound. Each datagram is dropped.
        while let Ok(dg) = sock.recv().await {
            counter.fetch_add(dg.data.len() as u64, Ordering::Relaxed);
        }
    })
}

impl AuthGate {
    /// Start the audio + video screen-sharing stream (as Device Hub does).
    pub(crate) async fn open(t: &mut Tunnel) -> Result<Self> {
        let mut client = DisplayServiceClient::connect_rsd(&mut t.adapter, &mut t.rsd)
            .await
            .map_err(ctx("displayservice"))?;
        let audio = t
            .adapter
            .bind_udp(0)
            .await
            .map_err(|e| Error::NotConnected(format!("bind_udp: {e}")))?;
        let video = t
            .adapter
            .bind_udp(0)
            .await
            .map_err(|e| Error::NotConnected(format!("bind_udp: {e}")))?;
        let host_ip = t.adapter.host_ip().to_string();
        let device_ip = t.adapter.peer_ip().to_string();

        // Values copied from a Device Hub offer the device accepted (upstream).
        let call_info = CallInfoBlob {
            call_id: 0,
            client_version: 1,
            device_type: "Mac17,7".into(),
            framework_version: "2205.3.1".into(),
            os_version: "25F71".into(),
            device_name: None,
            audio_device_uid: None,
        };
        let session = uuid::Uuid::new_v4();

        let audio_call = uuid::Uuid::new_v4().to_string().to_uppercase();
        let audio_offer =
            build_screen_audio_offer(&audio_call, &call_info).map_err(ctx("audio offer"))?;
        let audio_params = build_start_audio_parameters(
            &host_ip,
            audio.local_port(),
            &device_ip,
            50000,
            audio_offer,
            CLIENT_SUPPORTED_FEATURES,
            session,
        );
        Box::pin(client.start_media_stream(audio_params))
            .await
            .map_err(ctx("start audio stream"))?;

        let video_call = uuid::Uuid::new_v4().to_string().to_uppercase();
        let ssrc = uuid::Uuid::new_v4().as_u128() as u32;
        let video_offer =
            build_screen_video_offer(&video_call, &call_info, ssrc).map_err(ctx("video offer"))?;
        let video_params = build_start_video_parameters(
            &host_ip,
            video.local_port(),
            &device_ip,
            50001,
            video_offer,
            CLIENT_SUPPORTED_FEATURES,
            1, // display id 1 = main screen
            session,
        );
        Box::pin(client.start_media_stream(video_params))
            .await
            .map_err(ctx("start video stream"))?;

        let dropped_bytes = Arc::new(AtomicU64::new(0));
        let drains = vec![
            spawn_drain(audio, dropped_bytes.clone()),
            spawn_drain(video, dropped_bytes.clone()),
        ];
        tokio::time::sleep(GATE_SETTLE).await;
        Ok(Self {
            client,
            drains,
            dropped_bytes,
        })
    }

    /// Bytes of RTP read and dropped so far.
    pub(crate) fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes.load(Ordering::Relaxed)
    }

    async fn close(mut self) {
        tracing::debug!(
            "closing HID gate; dropped {} RTP bytes",
            self.dropped_bytes()
        );
        if let Err(e) = self.client.stop_media_stream().await {
            tracing::debug!("stop_media_stream: {e}");
        }
    }
}

impl Drop for AuthGate {
    fn drop(&mut self) {
        for d in &self.drains {
            d.abort();
        }
    }
}

struct Input {
    // Field order = drop order: clients before the gate.
    uhid: UniversalHidServiceClient<Box<dyn ReadWrite>>,
    indigo: IndigoHidClient<Box<dyn ReadWrite>>,
    gate: AuthGate,
}

impl Input {
    async fn open(t: &mut Tunnel) -> Result<Self> {
        let missing: Vec<_> = [tunnel::SVC_DISPLAY, tunnel::SVC_UHID, tunnel::SVC_INDIGO]
            .into_iter()
            .filter(|s| !t.has(s))
            .collect();
        if !missing.is_empty() {
            return Err(Error::setup(
                format!(
                    "the iPhone does not offer the HID services ({})",
                    missing.join(", ")
                ),
                DDI_HINT,
            ));
        }
        let gate = Box::pin(AuthGate::open(t)).await?;
        let uhid = UniversalHidServiceClient::connect_rsd(&mut t.adapter, &mut t.rsd)
            .await
            .map_err(ctx("universalhidservice"))?;
        let indigo = IndigoHidClient::connect_rsd(&mut t.adapter, &mut t.rsd)
            .await
            .map_err(ctx("hid.indigo"))?;
        Ok(Self { uhid, indigo, gate })
    }
}

struct Session {
    // Field order = drop order: services before the tunnel.
    input: Option<Input>,
    dvt: Option<RemoteServerClient<Box<dyn ReadWrite>>>,
    tunnel: Tunnel,
}

/// Route B backend.
pub struct HidBackend {
    source: TunnelSource,
    /// Lockdown provider for installation_proxy (None on RemotePairing only).
    lockdown: Option<Arc<dyn IdeviceProvider>>,
    session: Option<Session>,
}

macro_rules! retry {
    ($self:ident, $call:expr) => {{
        match $call {
            Err(e) if is_transport(&e) => {
                tracing::info!("iOS link lost ({e}); reconnecting");
                $self.reset().await;
                $call
            }
            r => r,
        }
    }};
}

impl HidBackend {
    pub(crate) fn new(source: TunnelSource, lockdown: Option<Arc<dyn IdeviceProvider>>) -> Self {
        Self {
            source,
            lockdown,
            session: None,
        }
    }

    /// Open the tunnel now (so connect errors show at connect time).
    pub(crate) async fn connect(&mut self) -> Result<()> {
        self.session().await.map(|_| ())
    }

    async fn reset(&mut self) {
        if let Some(mut s) = self.session.take() {
            if let Some(i) = s.input.take() {
                i.gate.close().await;
            }
        }
    }

    async fn session(&mut self) -> Result<&mut Session> {
        if self.session.is_none() {
            let tunnel = self.source.open().await?;
            self.session = Some(Session {
                input: None,
                dvt: None,
                tunnel,
            });
        }
        self.session
            .as_mut()
            .ok_or_else(|| Error::Other("iOS session missing".into()))
    }

    async fn input(&mut self) -> Result<&mut Input> {
        let s = self.session().await?;
        if s.input.is_none() {
            s.input = Some(Box::pin(Input::open(&mut s.tunnel)).await?);
        }
        s.input
            .as_mut()
            .ok_or_else(|| Error::Other("HID session missing".into()))
    }

    async fn dvt(&mut self) -> Result<&mut RemoteServerClient<Box<dyn ReadWrite>>> {
        let s = self.session().await?;
        if s.dvt.is_none() {
            if !s.tunnel.has(tunnel::SVC_DVT) {
                return Err(Error::setup(
                    "the iPhone does not offer the DVT service (dtservicehub)",
                    DDI_HINT,
                ));
            }
            let mut rs = RemoteServerClient::connect_rsd(&mut s.tunnel.adapter, &mut s.tunnel.rsd)
                .await
                .map_err(ctx("dtservicehub"))?;
            rs.read_message(0).await.map_err(ctx("DVT hello"))?;
            s.dvt = Some(rs);
        }
        s.dvt
            .as_mut()
            .ok_or_else(|| Error::Other("DVT session missing".into()))
    }

    async fn do_screenshot(&mut self) -> Result<Vec<u8>> {
        let rs = self.dvt().await?;
        let mut sc = ScreenshotClient::new(rs)
            .await
            .map_err(ctx("screenshot channel"))?;
        sc.take_screenshot().await.map_err(ctx("screenshot"))
    }

    async fn do_tap(&mut self, x: u16, y: u16) -> Result<()> {
        let i = self.input().await?;
        i.uhid.tap(x, y).await.map_err(ctx("tap"))
    }

    async fn do_double_tap(&mut self, x: u16, y: u16) -> Result<()> {
        let i = self.input().await?;
        i.uhid.tap(x, y).await.map_err(ctx("tap"))?;
        tokio::time::sleep(DOUBLE_TAP_GAP).await;
        i.uhid.tap(x, y).await.map_err(ctx("tap"))
    }

    async fn do_long_press(&mut self, x: u16, y: u16, duration: Duration) -> Result<()> {
        let i = self.input().await?;
        let start = tokio::time::Instant::now();
        loop {
            i.uhid
                .send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
                .await
                .map_err(ctx("press"))?;
            let left = duration.saturating_sub(start.elapsed());
            if left.is_zero() {
                break;
            }
            tokio::time::sleep(left.min(HOLD_RESEND)).await;
        }
        i.uhid
            .send_touchscreen(TOUCHSCREEN_STATE_RELEASE, x, y, None)
            .await
            .map_err(ctx("release"))
    }

    async fn do_drag(&mut self, path: &[(u16, u16)]) -> Result<()> {
        let i = self.input().await?;
        let Some(&(lx, ly)) = path.last() else {
            return Ok(());
        };
        for &(x, y) in path {
            i.uhid
                .send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
                .await
                .map_err(ctx("drag"))?;
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
        i.uhid
            .send_touchscreen(TOUCHSCREEN_STATE_RELEASE, lx, ly, None)
            .await
            .map_err(ctx("drag release"))
    }

    async fn do_multitouch(&mut self, frames: &[Vec<TouchscreenContact>]) -> Result<()> {
        let i = self.input().await?;
        for f in frames {
            i.uhid
                .send_multitouch(f, None)
                .await
                .map_err(ctx("multitouch"))?;
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
        Ok(())
    }

    async fn do_strokes(&mut self, strokes: &[Stroke]) -> Result<()> {
        let i = self.input().await?;
        for s in strokes {
            send_stroke(&mut i.indigo, s).await?;
        }
        Ok(())
    }

    async fn do_buttons(&mut self, presses: &[keymap::ButtonPress]) -> Result<()> {
        let i = self.input().await?;
        for (n, b) in presses.iter().enumerate() {
            if n > 0 {
                tokio::time::sleep(Duration::from_millis(keymap::DOUBLE_HOME_GAP_MS)).await;
            }
            i.indigo
                .send_button(b.page, b.usage, ButtonState::Down)
                .await
                .map_err(ctx("button down"))?;
            tokio::time::sleep(Duration::from_millis(b.hold_ms)).await;
            i.indigo
                .send_button(b.page, b.usage, ButtonState::Up)
                .await
                .map_err(ctx("button up"))?;
        }
        Ok(())
    }

    async fn do_set_pasteboard(&mut self, text: &str) -> Result<()> {
        let s = self.session().await?;
        let mut pb = PasteboardServiceClient::connect_rsd(&mut s.tunnel.adapter, &mut s.tunnel.rsd)
            .await
            .map_err(ctx("pasteboardservice"))?;
        pb.set_text(text, GENERAL_PASTEBOARD)
            .await
            .map_err(ctx("set pasteboard"))
    }

    async fn do_launch(&mut self, bundle_id: &str) -> Result<()> {
        let rs = self.dvt().await?;
        let mut pc = ProcessControlClient::new(rs)
            .await
            .map_err(ctx("process control"))?;
        pc.launch_app(bundle_id, None, None, false, false)
            .await
            .map(|_pid| ())
            .map_err(|e| match e {
                idevice::IdeviceError::NotFound => {
                    Error::InvalidArgument(format!("app {bundle_id} is not installed"))
                }
                e => crate::errors::map(&format!("launch {bundle_id}"), e),
            })
    }

    async fn do_list_apps(&mut self) -> Result<Vec<String>> {
        let p = self.lockdown.clone().ok_or_else(|| {
            Error::unsupported("list_apps over a RemotePairing-only link (connect by cable once)")
        })?;
        let mut ip = Box::pin(InstallationProxyClient::connect(&*p))
            .await
            .map_err(ctx("installation_proxy"))?;
        let apps = ip
            .get_apps(Some("User"), None)
            .await
            .map_err(ctx("list apps"))?;
        let mut ids: Vec<String> = apps.into_keys().collect();
        ids.sort();
        Ok(ids)
    }

    /// Paste `text` through the device pasteboard and Cmd+V. For text that
    /// has characters without a US key. UNVERIFIED on a device.
    pub(crate) async fn paste_text(&mut self, text: &str) -> Result<()> {
        retry!(self, self.do_set_pasteboard(text).await)?;
        let paste = keymap::key_stroke(&Key::parse("cmd+v").unwrap_or(Key::Char('v')))?;
        retry!(self, self.do_strokes(std::slice::from_ref(&paste)).await)
    }
}

async fn send_stroke(indigo: &mut IndigoHidClient<Box<dyn ReadWrite>>, s: &Stroke) -> Result<()> {
    for &m in &s.modifiers {
        indigo
            .send_keyboard(m, ButtonState::Down)
            .await
            .map_err(ctx("modifier down"))?;
    }
    indigo
        .send_keyboard(s.usage, ButtonState::Down)
        .await
        .map_err(ctx("key down"))?;
    indigo
        .send_keyboard(s.usage, ButtonState::Up)
        .await
        .map_err(ctx("key up"))?;
    for &m in s.modifiers.iter().rev() {
        indigo
            .send_keyboard(m, ButtonState::Up)
            .await
            .map_err(ctx("modifier up"))?;
    }
    tokio::time::sleep(KEY_GAP).await;
    Ok(())
}

/// Frames for a two-finger pinch on the horizontal axis.
pub(crate) fn pinch_frames(
    center: Point,
    from: f32,
    to: f32,
    duration: Duration,
) -> Vec<Vec<TouchscreenContact>> {
    let steps = steps_for(duration);
    let mut frames = Vec::with_capacity(steps as usize + 2);
    let contacts = |d: f32, touching: bool| {
        let half = (d / 2.0).clamp(0.0, 0.5);
        let a = to_hid(Point::new(center.x - half, center.y));
        let b = to_hid(Point::new(center.x + half, center.y));
        vec![
            TouchscreenContact {
                identity: 0,
                touching,
                x: a.0,
                y: a.1,
            },
            TouchscreenContact {
                identity: 1,
                touching,
                x: b.0,
                y: b.1,
            },
        ]
    };
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        frames.push(contacts(from + (to - from) * t, true));
    }
    frames.push(contacts(to, false));
    frames
}

#[async_trait(?Send)]
impl IosBackend for HidBackend {
    fn name(&self) -> &'static str {
        "hid"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tap: true,
            swipe: true,
            type_text: true,
            keys: true,
            buttons: true,
            launch_app: true,
            ui_tree: false,
            screenshot: true,
        }
    }

    async fn screenshot(&mut self) -> Result<Frame> {
        let png = retry!(self, self.do_screenshot().await)?;
        Frame::from_encoded(&png)
    }

    async fn tap(&mut self, at: Point) -> Result<()> {
        let (x, y) = to_hid(at);
        retry!(self, self.do_tap(x, y).await)
    }

    async fn double_tap(&mut self, at: Point) -> Result<()> {
        let (x, y) = to_hid(at);
        retry!(self, self.do_double_tap(x, y).await)
    }

    async fn long_press(&mut self, at: Point, duration: Duration) -> Result<()> {
        let (x, y) = to_hid(at);
        retry!(self, self.do_long_press(x, y, duration).await)
    }

    async fn swipe(&mut self, from: Point, to: Point, duration: Duration) -> Result<()> {
        let path = drag_path(from, to, steps_for(duration));
        retry!(self, self.do_drag(&path).await)
    }

    async fn pinch(&mut self, center: Point, from: f32, to: f32, duration: Duration) -> Result<()> {
        let frames = pinch_frames(center, from, to, duration);
        retry!(self, self.do_multitouch(&frames).await)
    }

    async fn type_text(&mut self, text: &str) -> Result<()> {
        let (strokes, skipped) = keymap::text_to_strokes(text);
        if skipped.is_empty() {
            return retry!(self, self.do_strokes(&strokes).await);
        }
        // Characters without a US key: paste the whole text instead.
        tracing::debug!("type_text: {} non-ASCII chars, using paste", skipped.len());
        self.paste_text(text).await
    }

    async fn key(&mut self, key: &Key) -> Result<()> {
        let stroke = keymap::key_stroke(key)?;
        retry!(self, self.do_strokes(std::slice::from_ref(&stroke)).await)
    }

    async fn button(&mut self, button: Button) -> Result<()> {
        if button == Button::Back {
            // iOS has no back button: slide a finger in from the left edge.
            return self
                .swipe(
                    Point::new(0.0, 0.5),
                    Point::new(0.6, 0.5),
                    Duration::from_millis(300),
                )
                .await;
        }
        let presses = keymap::button_presses(button);
        retry!(self, self.do_buttons(&presses).await)
    }

    async fn launch_app(&mut self, bundle_id: &str) -> Result<()> {
        if bundle_id.trim().is_empty() {
            return Err(Error::InvalidArgument("empty bundle id".into()));
        }
        retry!(self, self.do_launch(bundle_id).await)
    }

    async fn list_apps(&mut self) -> Result<Vec<String>> {
        self.do_list_apps().await
    }

    async fn shutdown(&mut self) {
        self.reset().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinch_frames_shape() {
        let f = pinch_frames(Point::CENTER, 0.1, 0.5, Duration::from_millis(160));
        assert_eq!(f.len(), 12);
        assert!(f[0].iter().all(|c| c.touching));
        assert!(f.last().unwrap().iter().all(|c| !c.touching));
        // First frame: fingers 0.1 apart around the center.
        assert_eq!(f[0][0].x, to_hid(Point::new(0.45, 0.5)).0);
        assert_eq!(f[0][1].x, to_hid(Point::new(0.55, 0.5)).0);
        assert_eq!(f[10][1].x, to_hid(Point::new(0.75, 0.5)).0);
    }
}
