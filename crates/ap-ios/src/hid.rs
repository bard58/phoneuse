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
//! iOS below 27: the device refuses the stream with CoreDevice error 9021
//! ("Remote control requires iOS 27.0 or later on this device"). The
//! backend does not send the offer there. IndigoHid connects alone, for
//! buttons and keys: quern #624 reports that iOS 26 accepts buttons without
//! a stream (unverified for keys). Touch calls and Back (an edge swipe)
//! return a setup error with the update fix ([`ap_core::ios_touch_error`]),
//! and they send nothing.
//!
//! iOS 27 or later: a refused stream, a stream offer with no answer, or a
//! missing touch service turns touch off for the session. Buttons and keys
//! stay on. Touch calls return [`touch_off_error`], and the shared flag of
//! [`IosBackend::touch_flag`] removes tap and swipe from the device
//! capabilities, so the hosts stop sending touch calls.
//!
//! Reconnect: a transport error closes the session, and the next call opens
//! a new one. Input is never replayed. A call runs once more only when the
//! error came before its first event reached the link. After that, it returns
//! an "input may be partly delivered" error (not a transport error, so upper
//! layers do not replay it either). Screenshot, pasteboard and launch are
//! safe to repeat and get one full retry.
//!
//! On an error or a cancel in the middle of a gesture or a key stroke, the
//! backend sends the release (touch up, key up) as best effort.
//!
//! Time limits: every attempt has a budget (setup + expected work + slack),
//! and the session close after an error has a short one. An expired budget
//! is [`Error::Timeout`], a transport error.
//!
//! UNVERIFIED: nothing here ran on a device. The API use matches idevice
//! v0.1.68 `tools/src/hid.rs` and the spike in `spikes/ios-hid`.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use crate::cancel::{CancelCheck, cancelled};
use crate::coords::{SAMPLE_INTERVAL, drag_path, steps_for, to_hid};
use crate::errors::{DDI_HINT, ctx, is_stream_refusal, is_transport};
use crate::keymap::{self, Stroke};
use crate::tunnel::{self, Tunnel};
use crate::version::IosVersion;

/// `clientSupportedFeatures` that Xcode Device Hub sends for screen sharing.
const CLIENT_SUPPORTED_FEATURES: u64 = 140;
/// Time for the device to answer the stream offer.
const GATE_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait after the stream starts before HID events are accepted.
const GATE_SETTLE: Duration = Duration::from_millis(300);
/// Gap between key strokes.
const KEY_GAP: Duration = Duration::from_millis(12);
/// Gap between the two taps of a double tap.
const DOUBLE_TAP_GAP: Duration = Duration::from_millis(90);
/// Resend interval for a held contact (long press).
const HOLD_RESEND: Duration = Duration::from_millis(100);
/// Contact time of one tap (as idevice `tap`).
const TAP_HOLD: Duration = Duration::from_millis(50);
/// Time for one attempt to open what a call needs (tunnel, HID gate, DVT).
pub(crate) const SETUP_BUDGET: Duration = Duration::from_secs(30);
/// Extra time on top of the expected work of one attempt.
const SEND_SLACK: Duration = Duration::from_secs(10);
/// Expected work of a DVT or lockdown call (screenshot, launch, list).
pub(crate) const WORK_DVT: Duration = Duration::from_secs(20);
/// Expected work of a tap, a key or a short button press.
pub(crate) const WORK_SHORT: Duration = Duration::from_secs(2);
/// Time for one best-effort release on the error path.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);
/// Time for stop_media_stream when a session closes.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// DVT channels per RemoteServerClient before it is reopened.
const DVT_CHANNEL_LIMIT: u32 = 200;

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
        if let Err(e) = Box::pin(client.start_media_stream(video_params)).await {
            // The audio stream can be running: stop it before the client goes.
            best_effort("stop_media_stream", client.stop_media_stream()).await;
            return Err(ctx("start video stream")(e));
        }

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
    /// `None` when touch is off (see `refused`).
    uhid: Option<UniversalHidServiceClient<Box<dyn ReadWrite>>>,
    indigo: IndigoHidClient<Box<dyn ReadWrite>>,
    /// `None` when the stream did not open (see `refused`).
    gate: Option<AuthGate>,
    /// Why touch input is off. Touch calls then fail. Buttons and keys
    /// still go to IndigoHid.
    refused: Option<String>,
}

/// What to do when a touch step (the touch client or the stream) failed.
#[derive(Debug, PartialEq, Eq)]
enum GateFailure {
    /// Keep the HID clients and block touch. The text is the reason.
    TouchOff(String),
    /// Return the error (the link is gone: reconnect and try again).
    Fail,
}

/// Decide what a failed touch step means (iOS 27 or later only: below 27
/// the backend does not open the stream). A lost link (`NotConnected`,
/// `Io`) fails the open, so the call can reconnect. Every other failure
/// turns touch off and keeps buttons and keys: a refusal (error 9021),
/// another device answer, and a stream offer that gets no answer in
/// [`GATE_TIMEOUT`].
fn gate_failure(e: &Error) -> GateFailure {
    if !is_stream_refusal(e) && matches!(e, Error::NotConnected(_) | Error::Io(_)) {
        GateFailure::Fail
    } else {
        GateFailure::TouchOff(e.to_string())
    }
}

/// Why touch is off before any touch I/O, or `None` when the backend must
/// try the touch client and the stream.
fn touch_skip_reason(version: IosVersion, has_uhid: bool, has_display: bool) -> Option<String> {
    if !version.touch_supported() {
        return Some(ap_core::ios_touch_problem(&version.to_string()));
    }
    let missing: Vec<_> = [
        (has_uhid, tunnel::SVC_UHID),
        (has_display, tunnel::SVC_DISPLAY),
    ]
    .into_iter()
    .filter_map(|(has, s)| (!has).then_some(s))
    .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "the iPhone does not offer the touch services ({})",
            missing.join(", ")
        )
    })
}

/// Run one touch step (connect the touch client, or open the stream) with
/// the [`GATE_TIMEOUT`] limit. `Ok(Err(why))`: touch is off, and buttons
/// and keys stay on. `Err`: the link is gone.
async fn touch_step<G>(
    what: &str,
    step: impl Future<Output = Result<G>>,
) -> Result<Result<G, String>> {
    match within(GATE_TIMEOUT, step).await {
        Ok(g) => Ok(Ok(g)),
        Err(e) => match gate_failure(&e) {
            GateFailure::TouchOff(why) => Ok(Err(format!("{what} ({why})"))),
            GateFailure::Fail => Err(e),
        },
    }
}

impl Input {
    async fn open(t: &mut Tunnel, version: IosVersion) -> Result<Self> {
        if !t.has(tunnel::SVC_INDIGO) {
            return Err(Error::setup(
                format!(
                    "the iPhone does not offer the HID service for buttons and keys ({})",
                    tunnel::SVC_INDIGO
                ),
                DDI_HINT,
            ));
        }
        // Buttons and keys first: a touch problem must not stop them.
        let indigo = IndigoHidClient::connect_rsd(&mut t.adapter, &mut t.rsd)
            .await
            .map_err(ctx("hid.indigo"))?;
        let mut input = Self {
            uhid: None,
            indigo,
            gate: None,
            refused: None,
        };
        // Below iOS 27 the device refuses the stream: do not send the offer.
        if let Some(why) =
            touch_skip_reason(version, t.has(tunnel::SVC_UHID), t.has(tunnel::SVC_DISPLAY))
        {
            tracing::warn!("iOS {version}: touch input is off ({why}); buttons and keys only");
            input.refused = Some(why);
            return Ok(input);
        }
        let uhid = touch_step("the touch client did not connect", async {
            UniversalHidServiceClient::connect_rsd(&mut t.adapter, &mut t.rsd)
                .await
                .map_err(ctx("universalhidservice"))
        })
        .await?;
        let uhid = match uhid {
            Ok(u) => u,
            Err(why) => {
                tracing::warn!("iOS {version}: {why}; touch input is off, buttons and keys only");
                input.refused = Some(why);
                return Ok(input);
            }
        };
        input.uhid = Some(uhid);
        match touch_step(
            "the iPhone did not open the screen stream that touch needs",
            Box::pin(AuthGate::open(t)),
        )
        .await?
        {
            Ok(g) => input.gate = Some(g),
            Err(why) => {
                tracing::warn!("iOS {version}: {why}; touch input is off, buttons and keys only");
                input.refused = Some(why);
            }
        }
        Ok(input)
    }
}

/// The fix text for touch that is off on iOS 27 or later.
const TOUCH_OFF_HINT: &str = "Run `agentphone doctor` to mount the Developer Disk Image again, then try again. If touch stays off, update the iPhone in Settings > General > Software Update";

/// The error for a touch call when touch is off on iOS 27 or later. Below
/// 27 the call gets [`ap_core::ios_touch_error`] instead.
fn touch_off_error(why: &str) -> Error {
    Error::setup(format!("Touch input is off: {why}."), TOUCH_OFF_HINT)
}

/// The error for a touch call on `version`, before any I/O, or `None` when
/// the version allows touch.
fn touch_version_error(version: IosVersion) -> Option<Error> {
    (!version.touch_supported()).then(|| ap_core::ios_touch_error(&version.to_string()))
}

/// The error for the Back button on `version`, or `None` when the version
/// allows touch. iOS has no Back button: Back is an edge swipe.
fn back_version_error(version: IosVersion) -> Option<Error> {
    (!version.touch_supported()).then(|| {
        Error::setup(
            format!(
                "Back is an edge swipe (iOS has no Back button), so it needs touch input. {}",
                ap_core::ios_touch_problem(&version.to_string())
            ),
            ap_core::IOS_UPDATE_HINT,
        )
    })
}

struct Session {
    // Field order = drop order: services before the tunnel.
    input: Option<Input>,
    dvt: Option<RemoteServerClient<Box<dyn ReadWrite>>>,
    /// DVT channels opened on `dvt`. idevice cannot close a channel, so the
    /// client is reopened after [`DVT_CHANNEL_LIMIT`] channels.
    dvt_channels: u32,
    tunnel: Tunnel,
}

/// Route B backend.
pub struct HidBackend {
    source: TunnelSource,
    /// Lockdown provider for installation_proxy (None on RemotePairing only).
    lockdown: Option<Arc<dyn IdeviceProvider>>,
    session: Option<Session>,
    /// Cancel check of the running call (set by the worker).
    cancel: CancelCheck,
    /// The iOS version of the device (touch input needs 27 or later).
    version: IosVersion,
    /// Set while touch is off on a version that allows touch (the stream
    /// or the touch client did not open). `IosDevice::capabilities` reads
    /// it, so the hosts stop sending taps that cannot work.
    touch_off: Arc<AtomicBool>,
}

/// Run `f` with a time limit. An expired limit gives [`Error::Timeout`].
async fn within<T>(limit: Duration, f: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(limit, f)
        .await
        .unwrap_or(Err(Error::Timeout(limit)))
}

/// Run a release step on the error path: short time limit, result ignored.
async fn best_effort<T, E: std::fmt::Display>(
    what: &str,
    f: impl Future<Output = std::result::Result<T, E>>,
) {
    match tokio::time::timeout(RELEASE_TIMEOUT, f).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::debug!("{what} after an error: {e}"),
        Err(_) => tracing::debug!("{what} after an error: timed out"),
    }
}

/// The error for input that failed after some events reached the link. It
/// is not a transport error, so no layer runs the input again.
fn partial(e: Error) -> Error {
    Error::Other(format!(
        "input may be partly delivered: the iPhone link failed during the action ({e}); take a screenshot before you try again"
    ))
}

/// Reconnect and run the call once more after a transport error. Only for
/// calls that are safe to repeat: screenshot, pasteboard, launch, list.
/// `$call` is a future expression. It is built again for the second attempt.
macro_rules! retry {
    ($self:ident, $work:expr, $call:expr) => {{
        let limit = attempt_budget($work);
        match within(limit, $call).await {
            Err(e) if is_transport(&e) => {
                tracing::info!("iOS link lost ({e}); reconnecting");
                $self.reset().await;
                let r = within(limit, $call).await;
                if matches!(&r, Err(e) if is_transport(e)) {
                    $self.reset().await;
                }
                r
            }
            r => r,
        }
    }};
}

/// Send input with no replay. `$sent` is a `bool` that the call sets after
/// its first event reached the link. A transport error before that event
/// reconnects and runs the call once more. A transport error after it
/// closes the session (the next call reconnects) and returns [`partial`].
macro_rules! send_input {
    ($self:ident, $work:expr, |$sent:ident| $call:expr) => {{
        let limit = attempt_budget($work);
        let mut $sent = false;
        let mut r = within(limit, $call).await;
        if !$sent && matches!(&r, Err(e) if is_transport(e)) {
            if let Err(e) = &r {
                tracing::info!("iOS link lost before any input was sent ({e}); reconnecting");
            }
            $self.reset().await;
            r = within(limit, $call).await;
        }
        match r {
            Err(e) if is_transport(&e) => {
                $self.reset().await;
                Err(if $sent { partial(e) } else { e })
            }
            r => r,
        }
    }};
}

/// Expected time of `n` key strokes (or a paste).
pub(crate) fn work_text(chars: usize) -> Duration {
    let n = u32::try_from(chars).unwrap_or(u32::MAX);
    WORK_DVT.saturating_add(Duration::from_millis(100).saturating_mul(n))
}

/// Expected time of a gesture that lasts `d` (a drag samples every
/// `SAMPLE_INTERVAL`, plus the send time).
pub(crate) fn work_gesture(d: Duration) -> Duration {
    d.saturating_mul(2).saturating_add(WORK_SHORT)
}

/// Time limit of one attempt of a call with `work` expected time.
pub(crate) fn attempt_budget(work: Duration) -> Duration {
    SETUP_BUDGET.saturating_add(work).saturating_add(SEND_SLACK)
}

/// Worker deadline (backstop) of a call with `work` expected time: two
/// attempts and two session closes, plus slack.
pub(crate) fn job_deadline(work: Duration) -> Duration {
    attempt_budget(work)
        .saturating_mul(2)
        .saturating_add(CLOSE_TIMEOUT.saturating_mul(2))
        .saturating_add(SEND_SLACK)
}

type Uhid = UniversalHidServiceClient<Box<dyn ReadWrite>>;
type Indigo = IndigoHidClient<Box<dyn ReadWrite>>;

/// End a touch sequence that held a contact at (`x`, `y`). On success, send
/// the release. On an error (or a cancel) after a contact was sent, send the
/// release as best effort and return the error.
async fn end_touch(uhid: &mut Uhid, r: Result<()>, sent: bool, x: u16, y: u16) -> Result<()> {
    match r {
        Ok(()) => uhid
            .send_touchscreen(TOUCHSCREEN_STATE_RELEASE, x, y, None)
            .await
            .map_err(ctx("release")),
        Err(e) => {
            if sent {
                best_effort(
                    "touch release",
                    uhid.send_touchscreen(TOUCHSCREEN_STATE_RELEASE, x, y, None),
                )
                .await;
            }
            Err(e)
        }
    }
}

/// One tap: contact, a short hold, release (as idevice `tap`, but the
/// contact and the release are separate sends, so `sent` is exact).
async fn tap_once(uhid: &mut Uhid, sent: &mut bool, x: u16, y: u16) -> Result<()> {
    uhid.send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
        .await
        .map_err(ctx("tap"))?;
    *sent = true;
    tokio::time::sleep(TAP_HOLD).await;
    end_touch(uhid, Ok(()), true, x, y).await
}

impl HidBackend {
    pub(crate) fn new(
        source: TunnelSource,
        lockdown: Option<Arc<dyn IdeviceProvider>>,
        version: IosVersion,
    ) -> Self {
        Self {
            source,
            lockdown,
            session: None,
            cancel: CancelCheck::default(),
            version,
            touch_off: Arc::default(),
        }
    }

    /// Open the tunnel now (so connect errors show at connect time).
    pub(crate) async fn connect(&mut self) -> Result<()> {
        let r = within(SETUP_BUDGET, async { self.session().await.map(|_| ()) }).await;
        if r.is_err() {
            self.session = None;
        }
        r
    }

    /// Close the session. The stream stop has a short time limit, because
    /// the link is often dead when this runs.
    async fn reset(&mut self) {
        if let Some(mut s) = self.session.take()
            && let Some(i) = s.input.take()
            && let Some(gate) = i.gate
            && tokio::time::timeout(CLOSE_TIMEOUT, gate.close())
                .await
                .is_err()
        {
            tracing::debug!("stop_media_stream: timed out");
        }
    }

    async fn session(&mut self) -> Result<&mut Session> {
        if self.session.is_none() {
            let tunnel = self.source.open().await?;
            self.session = Some(Session {
                input: None,
                dvt: None,
                dvt_channels: 0,
                tunnel,
            });
        }
        self.session
            .as_mut()
            .ok_or_else(|| Error::Other("iOS session missing".into()))
    }

    async fn input(&mut self) -> Result<&mut Input> {
        let version = self.version;
        let touch_off = self.touch_off.clone();
        let s = self.session().await?;
        if s.input.is_none() {
            let i = Box::pin(Input::open(&mut s.tunnel, version)).await?;
            touch_off.store(
                version.touch_supported() && i.refused.is_some(),
                Ordering::Relaxed,
            );
            s.input = Some(i);
        }
        s.input
            .as_mut()
            .ok_or_else(|| Error::Other("HID session missing".into()))
    }

    /// The touch client. Below iOS 27 this fails before any I/O. From iOS
    /// 27 it fails when the stream did not open. The error is a setup
    /// error, so no layer runs the call again.
    async fn touch(&mut self) -> Result<&mut Uhid> {
        if let Some(e) = touch_version_error(self.version) {
            return Err(e);
        }
        let i = self.input().await?;
        match (&mut i.uhid, &i.refused) {
            (Some(u), None) => Ok(u),
            (_, why) => Err(touch_off_error(
                why.as_deref().unwrap_or("the touch client is missing"),
            )),
        }
    }

    /// The DVT client, for one new channel (screenshot or process control).
    async fn dvt(&mut self) -> Result<&mut RemoteServerClient<Box<dyn ReadWrite>>> {
        let s = self.session().await?;
        if s.dvt_channels >= DVT_CHANNEL_LIMIT {
            tracing::debug!("reopening DVT after {} channels", s.dvt_channels);
            s.dvt = None;
            s.dvt_channels = 0;
        }
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
        s.dvt_channels += 1;
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

    async fn do_tap(&mut self, sent: &mut bool, x: u16, y: u16) -> Result<()> {
        let uhid = self.touch().await?;
        tap_once(uhid, sent, x, y).await
    }

    async fn do_double_tap(&mut self, sent: &mut bool, x: u16, y: u16) -> Result<()> {
        let uhid = self.touch().await?;
        tap_once(uhid, sent, x, y).await?;
        tokio::time::sleep(DOUBLE_TAP_GAP).await;
        tap_once(uhid, sent, x, y).await
    }

    async fn do_long_press(
        &mut self,
        sent: &mut bool,
        x: u16,
        y: u16,
        duration: Duration,
    ) -> Result<()> {
        let cancel = self.cancel.clone();
        let uhid = self.touch().await?;
        let start = tokio::time::Instant::now();
        let r: Result<()> = async {
            loop {
                uhid.send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
                    .await
                    .map_err(ctx("press"))?;
                *sent = true;
                let left = duration.saturating_sub(start.elapsed());
                if left.is_zero() {
                    return Ok(());
                }
                tokio::time::sleep(left.min(HOLD_RESEND)).await;
                if cancel.is_set() {
                    return Err(cancelled());
                }
            }
        }
        .await;
        end_touch(uhid, r, *sent, x, y).await
    }

    async fn do_drag(&mut self, sent: &mut bool, path: &[(u16, u16)]) -> Result<()> {
        let cancel = self.cancel.clone();
        let uhid = self.touch().await?;
        let Some(&(lx, ly)) = path.last() else {
            return Ok(());
        };
        let mut at = (lx, ly);
        let r: Result<()> = async {
            for &(x, y) in path {
                if cancel.is_set() {
                    return Err(cancelled());
                }
                uhid.send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
                    .await
                    .map_err(ctx("drag"))?;
                *sent = true;
                at = (x, y);
                tokio::time::sleep(SAMPLE_INTERVAL).await;
            }
            Ok(())
        }
        .await;
        end_touch(uhid, r, *sent, at.0, at.1).await
    }

    async fn do_multitouch(
        &mut self,
        sent: &mut bool,
        frames: &[Vec<TouchscreenContact>],
    ) -> Result<()> {
        let cancel = self.cancel.clone();
        let uhid = self.touch().await?;
        let mut last: Option<&Vec<TouchscreenContact>> = None;
        let r: Result<()> = async {
            for f in frames {
                if cancel.is_set() {
                    return Err(cancelled());
                }
                uhid.send_multitouch(f, None)
                    .await
                    .map_err(ctx("multitouch"))?;
                *sent = true;
                last = Some(f);
                tokio::time::sleep(SAMPLE_INTERVAL).await;
            }
            Ok(())
        }
        .await;
        if r.is_err()
            && let Some(f) = last.filter(|f| f.iter().any(|c| c.touching))
        {
            let up = lifted(f);
            best_effort("multitouch release", uhid.send_multitouch(&up, None)).await;
        }
        r
    }

    async fn do_strokes(&mut self, sent: &mut bool, strokes: &[Stroke]) -> Result<()> {
        let cancel = self.cancel.clone();
        let i = self.input().await?;
        for s in strokes {
            if cancel.is_set() {
                return Err(cancelled());
            }
            send_stroke(&mut i.indigo, sent, s).await?;
        }
        Ok(())
    }

    async fn do_buttons(&mut self, sent: &mut bool, presses: &[keymap::ButtonPress]) -> Result<()> {
        let cancel = self.cancel.clone();
        let i = self.input().await?;
        for (n, b) in presses.iter().enumerate() {
            if n > 0 {
                tokio::time::sleep(Duration::from_millis(keymap::DOUBLE_HOME_GAP_MS)).await;
            }
            if cancel.is_set() {
                return Err(cancelled());
            }
            i.indigo
                .send_button(b.page, b.usage, ButtonState::Down)
                .await
                .map_err(ctx("button down"))?;
            *sent = true;
            tokio::time::sleep(Duration::from_millis(b.hold_ms)).await;
            if let Err(e) = i.indigo.send_button(b.page, b.usage, ButtonState::Up).await {
                best_effort(
                    "button up",
                    i.indigo.send_button(b.page, b.usage, ButtonState::Up),
                )
                .await;
                return Err(ctx("button up")(e));
            }
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
            .map_err(|e| launch_error(bundle_id, e))
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
    /// has characters without a US key. The pasteboard step can run twice.
    /// The Cmd+V step is not replayed. UNVERIFIED on a device.
    pub(crate) async fn paste_text(&mut self, text: &str) -> Result<()> {
        retry!(self, WORK_DVT, self.do_set_pasteboard(text))?;
        let paste = keymap::key_stroke(&Key::parse("cmd+v").unwrap_or(Key::Char('v')))?;
        send_input!(self, WORK_SHORT, |sent| self
            .do_strokes(&mut sent, std::slice::from_ref(&paste)))
    }
}

/// Map a process-control launch error. idevice 0.1.68 gives
/// `UnexpectedResponse("expected integer PID in launch app response")` when
/// the device returns no PID (usually: the app is not installed).
fn launch_error(bundle_id: &str, e: idevice::IdeviceError) -> Error {
    match e {
        idevice::IdeviceError::UnexpectedResponse(ref m) if m.contains("launch app response") => {
            Error::InvalidArgument(format!(
                "could not launch {bundle_id}: the device returned no PID (the app is probably not installed; use list_apps to see installed bundle ids)"
            ))
        }
        e => crate::errors::map(&format!("launch {bundle_id}"), e),
    }
}

/// The same contacts with every finger lifted.
fn lifted(frame: &[TouchscreenContact]) -> Vec<TouchscreenContact> {
    frame
        .iter()
        .map(|c| TouchscreenContact {
            touching: false,
            ..*c
        })
        .collect()
}

/// Send one stroke: modifiers down, key down, key up, modifiers up. On an
/// error after the first down, send every up as best effort, so no key stays
/// held on the device.
async fn send_stroke(indigo: &mut Indigo, sent: &mut bool, s: &Stroke) -> Result<()> {
    let r: Result<()> = async {
        for &m in &s.modifiers {
            indigo
                .send_keyboard(m, ButtonState::Down)
                .await
                .map_err(ctx("modifier down"))?;
            *sent = true;
        }
        indigo
            .send_keyboard(s.usage, ButtonState::Down)
            .await
            .map_err(ctx("key down"))?;
        *sent = true;
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
        Ok(())
    }
    .await;
    if r.is_err() && *sent {
        best_effort("key up", indigo.send_keyboard(s.usage, ButtonState::Up)).await;
        for &m in s.modifiers.iter().rev() {
            best_effort("modifier up", indigo.send_keyboard(m, ButtonState::Up)).await;
        }
    }
    r?;
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

    /// From the iOS version: below iOS 27 there is no tap or swipe (screen
    /// only). On iOS 27+ a refused stream shows at the first input call: the
    /// touch call gets [`touch_off_error`], and [`IosBackend::touch_flag`]
    /// turns tap and swipe off.
    fn capabilities(&self) -> Capabilities {
        self.version.capabilities()
    }

    fn touch_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(self.touch_off.clone())
    }

    async fn screenshot(&mut self) -> Result<Frame> {
        let png = retry!(self, WORK_DVT, self.do_screenshot())?;
        Frame::from_encoded(&png)
    }

    async fn tap(&mut self, at: Point) -> Result<()> {
        let (x, y) = to_hid(at);
        send_input!(self, WORK_SHORT, |sent| self.do_tap(&mut sent, x, y))
    }

    async fn double_tap(&mut self, at: Point) -> Result<()> {
        let (x, y) = to_hid(at);
        send_input!(self, WORK_SHORT, |sent| self.do_double_tap(&mut sent, x, y))
    }

    async fn long_press(&mut self, at: Point, duration: Duration) -> Result<()> {
        let (x, y) = to_hid(at);
        send_input!(self, work_gesture(duration), |sent| self
            .do_long_press(&mut sent, x, y, duration))
    }

    async fn swipe(&mut self, from: Point, to: Point, duration: Duration) -> Result<()> {
        let path = drag_path(from, to, steps_for(duration));
        send_input!(self, work_gesture(duration), |sent| self
            .do_drag(&mut sent, &path))
    }

    async fn pinch(&mut self, center: Point, from: f32, to: f32, duration: Duration) -> Result<()> {
        let frames = pinch_frames(center, from, to, duration);
        send_input!(self, work_gesture(duration), |sent| self
            .do_multitouch(&mut sent, &frames))
    }

    async fn type_text(&mut self, text: &str) -> Result<()> {
        let (strokes, skipped) = keymap::text_to_strokes(text);
        if skipped.is_empty() {
            return send_input!(self, work_text(strokes.len()), |sent| self
                .do_strokes(&mut sent, &strokes));
        }
        // Characters without a US key: paste the whole text instead.
        tracing::debug!("type_text: {} non-ASCII chars, using paste", skipped.len());
        self.paste_text(text).await
    }

    async fn key(&mut self, key: &Key) -> Result<()> {
        let stroke = keymap::key_stroke(key)?;
        send_input!(self, WORK_SHORT, |sent| self
            .do_strokes(&mut sent, std::slice::from_ref(&stroke)))
    }

    async fn button(&mut self, button: Button) -> Result<()> {
        if button == Button::Back {
            if let Some(e) = back_version_error(self.version) {
                return Err(e);
            }
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
        let work = presses
            .iter()
            .map(|p| Duration::from_millis(p.hold_ms + keymap::DOUBLE_HOME_GAP_MS))
            .fold(WORK_SHORT, Duration::saturating_add);
        send_input!(self, work, |sent| self.do_buttons(&mut sent, &presses))
    }

    async fn launch_app(&mut self, bundle_id: &str) -> Result<()> {
        if bundle_id.trim().is_empty() {
            return Err(Error::InvalidArgument("empty bundle id".into()));
        }
        retry!(self, WORK_DVT, self.do_launch(bundle_id))
    }

    async fn list_apps(&mut self) -> Result<Vec<String>> {
        within(attempt_budget(WORK_DVT), self.do_list_apps()).await
    }

    fn set_cancel(&mut self, cancel: CancelCheck) {
        self.cancel = cancel;
    }

    fn abandon(&mut self) {
        // No clean close: the stream may be mid-read. Drop aborts the drains.
        self.session = None;
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

    /// Stands in for HidBackend in the retry macros. Each attempt pops one
    /// planned outcome: (events sent before the result, result).
    struct Link {
        plan: Vec<(bool, Result<()>)>,
        attempts: usize,
        resets: usize,
    }

    impl Link {
        fn new(mut plan: Vec<(bool, Result<()>)>) -> Self {
            plan.reverse();
            Self {
                plan,
                attempts: 0,
                resets: 0,
            }
        }

        async fn reset(&mut self) {
            self.resets += 1;
        }

        async fn op(&mut self, sent: &mut bool) -> Result<()> {
            self.attempts += 1;
            let (s, r) = self.plan.pop().unwrap_or((true, Ok(())));
            *sent |= s;
            r
        }

        async fn send(&mut self) -> Result<()> {
            send_input!(self, WORK_SHORT, |sent| self.op(&mut sent))
        }

        async fn repeat(&mut self) -> Result<()> {
            retry!(self, WORK_SHORT, async {
                let mut s = false;
                self.op(&mut s).await
            })
        }
    }

    fn lost() -> Result<()> {
        Err(Error::NotConnected("broken pipe".into()))
    }

    #[tokio::test]
    async fn input_is_retried_only_before_the_first_event() {
        // Link lost before any event: reconnect and run once more.
        let mut l = Link::new(vec![(false, lost()), (true, Ok(()))]);
        l.send().await.unwrap();
        assert_eq!((l.attempts, l.resets), (2, 1));

        // Link lost after an event: no replay, session closed, and the error
        // is not a transport error (so ap-mcp does not replay it either).
        let mut l = Link::new(vec![(true, lost())]);
        let e = l.send().await.unwrap_err();
        assert_eq!((l.attempts, l.resets), (1, 1));
        assert!(!is_transport(&e), "{e}");
        assert!(e.to_string().contains("partly delivered"), "{e}");

        // Second attempt fails after an event: no third attempt.
        let mut l = Link::new(vec![(false, lost()), (true, lost())]);
        let e = l.send().await.unwrap_err();
        assert_eq!((l.attempts, l.resets), (2, 2));
        assert!(!is_transport(&e));

        // Second attempt fails before any event: still a transport error,
        // and nothing reached the device.
        let mut l = Link::new(vec![(false, lost()), (false, lost())]);
        let e = l.send().await.unwrap_err();
        assert_eq!((l.attempts, l.resets), (2, 2));
        assert!(is_transport(&e));

        // A cancel or a device error is never retried.
        let mut l = Link::new(vec![(true, Err(cancelled()))]);
        assert!(crate::cancel::is_cancelled(&l.send().await.unwrap_err()));
        assert_eq!((l.attempts, l.resets), (1, 0));
    }

    #[tokio::test]
    async fn idempotent_calls_retry_once_and_close_a_dead_session() {
        let mut l = Link::new(vec![(true, lost()), (true, Ok(()))]);
        l.repeat().await.unwrap();
        assert_eq!((l.attempts, l.resets), (2, 1));
        let mut l = Link::new(vec![(true, lost()), (true, lost())]);
        assert!(is_transport(&l.repeat().await.unwrap_err()));
        assert_eq!((l.attempts, l.resets), (2, 2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_attempt_times_out() {
        let r: Result<()> = within(Duration::from_secs(1), std::future::pending()).await;
        assert!(matches!(r, Err(Error::Timeout(_))));
        assert!(is_transport(&r.unwrap_err()));
        assert!(job_deadline(WORK_SHORT) > attempt_budget(WORK_SHORT) * 2);
        assert!(work_text(1000) > Duration::from_secs(100));
    }

    #[test]
    fn launch_without_pid_says_not_installed() {
        let e = launch_error(
            "com.example.none",
            idevice::IdeviceError::UnexpectedResponse(
                "expected integer PID in launch app response".into(),
            ),
        );
        assert!(
            matches!(&e, Error::InvalidArgument(m) if m.contains("not installed")),
            "{e}"
        );
        let e = launch_error(
            "x",
            idevice::IdeviceError::Socket(std::io::Error::other("reset")),
        );
        assert!(is_transport(&e));
    }

    fn closed_backend(version: IosVersion) -> HidBackend {
        // The source points at a closed port: any I/O fails.
        HidBackend::new(
            TunnelSource::RemotePairing {
                addr: "127.0.0.1:1".parse().unwrap(),
                path: PathBuf::from("does-not-exist"),
            },
            None,
            version,
        )
    }

    fn refusal() -> Error {
        Error::Protocol(
            "start audio stream: device returned an error: Dictionary({\"code\": Integer(9021)})"
                .into(),
        )
    }

    #[test]
    fn a_refused_stream_turns_touch_off_and_a_lost_link_fails() {
        let lost = Error::NotConnected("broken pipe".into());
        assert!(matches!(
            gate_failure(&refusal()),
            GateFailure::TouchOff(w) if w.contains("9021")
        ));
        // Another device answer and a stream offer with no answer: touch off.
        assert!(matches!(
            gate_failure(&Error::Protocol("bad offer".into())),
            GateFailure::TouchOff(_)
        ));
        assert!(matches!(
            gate_failure(&Error::Timeout(GATE_TIMEOUT)),
            GateFailure::TouchOff(_)
        ));
        // A lost link reconnects.
        assert_eq!(gate_failure(&lost), GateFailure::Fail);
        assert_eq!(
            gate_failure(&Error::Io(std::io::Error::other("reset"))),
            GateFailure::Fail
        );
        let e = touch_off_error("the iPhone did not open the screen stream (9021)");
        assert!(e.to_string().starts_with("Touch input is off: "), "{e}");
        let hint = e.hint().unwrap();
        assert!(hint.starts_with("Run `agentphone doctor`"), "{hint}");
        assert!(hint.contains("Software Update"), "{hint}");
        assert!(!is_transport(&e));
    }

    /// The touch step keeps the input (buttons and keys) on a refusal and on
    /// a stream offer that gets no answer. Only a lost link fails the open.
    #[tokio::test(start_paused = true)]
    async fn touch_step_keeps_buttons_on_a_refusal_and_a_timeout() {
        let r = touch_step("stream", async { Err::<(), _>(refusal()) })
            .await
            .unwrap();
        assert!(matches!(&r, Err(w) if w.starts_with("stream (") && w.contains("9021")));
        let r = touch_step("stream", std::future::pending::<Result<()>>())
            .await
            .unwrap();
        assert!(matches!(&r, Err(w) if w.contains("timed out")), "{r:?}");
        let e = touch_step("stream", async {
            Err::<(), _>(Error::NotConnected("x".into()))
        })
        .await
        .unwrap_err();
        assert!(is_transport(&e));
        assert_eq!(touch_step("stream", async { Ok(7) }).await.unwrap(), Ok(7));
    }

    #[test]
    fn below_27_the_stream_is_not_opened() {
        let v = |s: &str| IosVersion::parse(s).unwrap();
        // Below 27: touch is off before any offer, also with every service.
        let why = touch_skip_reason(v("26.3"), true, true).unwrap();
        assert!(why.contains("needs iOS 27"), "{why}");
        assert!(touch_skip_reason(v("17.4"), true, true).is_some());
        // From 27: a missing touch service turns touch off (buttons stay).
        assert!(touch_skip_reason(v("27.0"), true, true).is_none());
        let why = touch_skip_reason(v("27.0"), false, true).unwrap();
        assert!(why.contains(tunnel::SVC_UHID), "{why}");
        let why = touch_skip_reason(v("27.0"), true, false).unwrap();
        assert!(why.contains(tunnel::SVC_DISPLAY) && !why.contains(tunnel::SVC_UHID));
    }

    #[test]
    fn capabilities_follow_the_version() {
        let v = |s: &str| IosVersion::parse(s).unwrap();
        let mut b = closed_backend(v("26.3"));
        let c = b.capabilities();
        assert!(!c.tap && !c.swipe && c.buttons && c.keys && c.screenshot);
        b.version = v("27.0");
        assert!(b.capabilities().tap);
        // The flag for IosDevice starts off.
        let flag = b.touch_flag().unwrap();
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn touch_is_blocked_below_ios_27() {
        let v = |s: &str| IosVersion::parse(s).unwrap();
        let e = touch_version_error(v("26.3")).unwrap();
        assert_eq!(
            e.to_string(),
            "Touch input needs iOS 27 or later. This iPhone has iOS 26.3.
  fix: Update the iPhone in Settings > General > Software Update."
        );
        // A setup error: no layer replays it as a lost link.
        assert!(!is_transport(&e));
        assert!(touch_version_error(v("27.0")).is_none());
        assert!(back_version_error(v("27.0")).is_none());
    }

    /// Below iOS 27 every touch call (and Back, an edge swipe) fails at once
    /// with the update fix, and it opens no tunnel.
    #[tokio::test]
    async fn touch_call_below_27_fails_without_io() {
        let mut b = closed_backend(IosVersion::new(26, 7, 0));
        let d = Duration::from_millis(50);
        let errors = vec![
            b.tap(Point::CENTER).await.unwrap_err(),
            b.double_tap(Point::CENTER).await.unwrap_err(),
            b.long_press(Point::CENTER, d).await.unwrap_err(),
            b.swipe(Point::CENTER, Point::new(0.5, 0.1), d)
                .await
                .unwrap_err(),
            b.pinch(Point::CENTER, 0.1, 0.5, d).await.unwrap_err(),
        ];
        for e in &errors {
            assert!(e.to_string().contains("needs iOS 27"), "{e}");
            assert_eq!(e.hint(), Some(ap_core::IOS_UPDATE_HINT));
        }
        let e = b.button(Button::Back).await.unwrap_err();
        assert!(
            e.to_string()
                .starts_with("Back is an edge swipe (iOS has no Back button), so it needs touch input. Touch input needs iOS 27"),
            "{e}"
        );
        assert_eq!(e.hint(), Some(ap_core::IOS_UPDATE_HINT));
        assert!(b.session.is_none());
    }

    #[test]
    fn lifted_releases_every_finger() {
        let f = pinch_frames(Point::CENTER, 0.1, 0.5, Duration::from_millis(160));
        let up = lifted(&f[3]);
        assert_eq!(up.len(), 2);
        assert!(up.iter().all(|c| !c.touching));
        assert_eq!((up[0].x, up[0].identity), (f[3][0].x, f[3][0].identity));
    }
}
