//! Byte transports to the dongle, and [`Link`]: the protocol session on top.
//!
//! - [`SerialTransport`]: USB serial (CDC ACM or a UART bridge) via `serialport`.
//! - [`MemoryTransport`]: an in-memory dongle simulator for tests. It runs the
//!   same protocol logic as `firmware/` and records every report it would send
//!   to the phone, with a timestamp.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ap_core::{Error, Result};
use async_trait::async_trait;
use tokio::time::Instant;

use crate::protocol::{
    self, Deframer, DeviceMsg, DongleInfo, HostMsg, LinkKind, PROTOCOL_VERSION, Status, msg,
};
use crate::report::Report;

/// Default baud rate (ignored by USB CDC, used by UART bridges).
pub const BAUD: u32 = 115_200;
/// How long to wait for a reply.
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(300);
/// How long to wait for HELLO_ACK (a board may still be booting after the port opens).
pub const HELLO_TIMEOUT: Duration = Duration::from_millis(1500);

/// A raw byte pipe to the dongle.
#[async_trait]
pub trait Transport: Send {
    /// Write all bytes.
    async fn write(&mut self, bytes: &[u8]) -> Result<()>;
    /// Read the bytes available within `timeout` (empty on timeout).
    async fn read(&mut self, timeout: Duration) -> Result<Vec<u8>>;
    /// Port name for messages ("COM5", "memory").
    fn name(&self) -> String;
}

// ---------- serial ----------

/// USB serial transport. Blocking `serialport` calls run on tokio's blocking pool.
pub struct SerialTransport {
    port: Option<Box<dyn serialport::SerialPort>>,
    name: String,
}

impl SerialTransport {
    /// Open `path` ("COM5", "/dev/ttyACM0") at [`BAUD`].
    pub fn open(path: &str) -> Result<Self> {
        let mut port = serialport::new(path, BAUD)
            .timeout(Duration::from_millis(20))
            .open()
            .map_err(|e| {
                Error::setup(
                    format!("cannot open serial port {path}: {e}"),
                    "Plug in the AgentPhone dongle, close other programs that use the port \
                     (Arduino IDE, PlatformIO monitor), and check `agentphone devices`.",
                )
            })?;
        // Native USB CDC on ESP32-S3/RP2040 only sends once DTR is set. Do not
        // touch RTS: on boards with a UART bridge, RTS+DTR toggles reset the chip.
        let _ = port.write_data_terminal_ready(true);
        Ok(Self {
            port: Some(port),
            name: path.to_string(),
        })
    }

    async fn with_port<T: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut Box<dyn serialport::SerialPort>) -> std::io::Result<T> + Send + 'static,
    ) -> Result<T> {
        let mut port = self
            .port
            .take()
            .ok_or_else(|| Error::NotConnected(format!("serial port {} was lost", self.name)))?;
        let (port, res) = tokio::task::spawn_blocking(move || {
            let r = f(&mut port);
            (port, r)
        })
        .await
        .map_err(|e| Error::Other(format!("serial worker failed: {e}")))?;
        self.port = Some(port);
        res.map_err(|e| Error::NotConnected(format!("serial port {}: {e}", self.name)))
    }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let bytes = bytes.to_vec();
        self.with_port(move |p| {
            p.write_all(&bytes)?;
            p.flush()
        })
        .await
    }

    async fn read(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.with_port(move |p| {
            p.set_timeout(timeout.max(Duration::from_millis(1)))
                .map_err(std::io::Error::other)?;
            let mut buf = [0u8; 256];
            match p.read(&mut buf) {
                Ok(n) => Ok(buf[..n].to_vec()),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(Vec::new()),
                Err(e) => Err(e),
            }
        })
        .await
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

// ---------- in-memory simulator ----------

/// A report the simulated dongle sent to the phone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recorded {
    /// Time since the simulator was created (tokio clock, so paused time works).
    pub at: Duration,
    pub report: Report,
}

/// State of the simulated dongle. Shared between the transport and test code.
#[derive(Debug)]
pub struct SimState {
    /// Protocol version the simulated firmware speaks.
    pub protocol: u8,
    pub phone_connected: bool,
    pub ready: bool,
    pub reports: Vec<Recorded>,
    /// Every decoded host message, in order.
    pub host_msgs: Vec<HostMsg>,
    /// Last absolute report payload (for RELEASE_ALL).
    pub last_abs: Option<(u16, u16)>,
    /// Drop the reply to the next N messages (to test timeouts).
    pub drop_replies: u32,
    start: Instant,
}

/// Handle to inspect and steer a [`MemoryTransport`].
#[derive(Debug, Clone)]
pub struct SimHandle(Arc<Mutex<SimState>>);

impl SimHandle {
    fn lock(&self) -> std::sync::MutexGuard<'_, SimState> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Recorded reports.
    pub fn recorded(&self) -> Vec<Recorded> {
        self.lock().reports.clone()
    }

    /// Recorded reports without timestamps.
    pub fn reports(&self) -> Vec<Report> {
        self.lock().reports.iter().map(|r| r.report).collect()
    }

    pub fn host_msgs(&self) -> Vec<HostMsg> {
        self.lock().host_msgs.clone()
    }

    pub fn clear(&self) {
        let mut s = self.lock();
        s.reports.clear();
        s.host_msgs.clear();
    }

    pub fn set_phone_connected(&self, on: bool) {
        self.lock().phone_connected = on;
    }

    pub fn set_protocol(&self, v: u8) {
        self.lock().protocol = v;
    }

    pub fn drop_replies(&self, n: u32) {
        self.lock().drop_replies = n;
    }
}

/// In-memory transport with a simulated dongle behind it.
pub struct MemoryTransport {
    sim: SimHandle,
    deframer: Deframer,
    outbox: VecDeque<u8>,
    /// Bytes to deliver before any reply (simulates boot log noise).
    pub noise: Vec<u8>,
}

impl MemoryTransport {
    /// A simulated dongle with a phone connected.
    pub fn new() -> (Self, SimHandle) {
        let sim = SimHandle(Arc::new(Mutex::new(SimState {
            protocol: PROTOCOL_VERSION,
            phone_connected: true,
            ready: false,
            reports: Vec::new(),
            host_msgs: Vec::new(),
            last_abs: None,
            drop_replies: 0,
            start: Instant::now(),
        })));
        (
            Self {
                sim: sim.clone(),
                deframer: Deframer::default(),
                outbox: VecDeque::new(),
                noise: b"ESP-ROM:esp32s3 boot\r\n".to_vec(),
            },
            sim,
        )
    }

    fn handle(&mut self, wire: &[u8]) {
        let mut s = self.sim.lock();
        let (seq, reply) = match protocol::decode(wire) {
            Err(_) => (0, DeviceMsg::Ack(Status::BadFrame as u8)),
            Ok(f) => (f.seq, sim_reply(&mut s, &f)),
        };
        if s.drop_replies > 0 {
            s.drop_replies -= 1;
            return;
        }
        drop(s);
        self.outbox.extend(protocol::encode(&reply.to_frame(seq)));
    }
}

/// The firmware's dispatch logic (mirrors `firmware/src/main.cpp`).
fn sim_reply(s: &mut SimState, f: &protocol::Frame) -> DeviceMsg {
    let ack = |st: Status| DeviceMsg::Ack(st as u8);
    let m = match HostMsg::from_frame(f) {
        Ok(m) => m,
        Err(_) => {
            return if f.kind == msg::HELLO || f.kind == msg::REPORT {
                ack(Status::BadReport)
            } else {
                ack(Status::UnknownType)
            };
        }
    };
    s.host_msgs.push(m.clone());
    let at = s.start.elapsed();
    match m {
        HostMsg::Hello { version } => {
            s.ready = version == s.protocol;
            DeviceMsg::HelloAck(DongleInfo {
                protocol: s.protocol,
                firmware: (0, 1, 0),
                link: LinkKind::Ble,
                phone_connected: s.phone_connected,
                name: "AgentPhone HID (sim)".into(),
            })
        }
        _ if s.protocol != PROTOCOL_VERSION => ack(Status::BadVersion),
        _ if !s.ready => ack(Status::NotReady),
        HostMsg::Ping => DeviceMsg::Pong {
            phone_connected: s.phone_connected,
        },
        HostMsg::Report(bytes) => {
            let Ok(r) = Report::from_bytes(&bytes) else {
                return ack(Status::BadReport);
            };
            if !s.phone_connected {
                return ack(Status::NotConnected);
            }
            if let Report::Absolute { x, y, .. } = r {
                s.last_abs = Some((x, y));
            }
            s.reports.push(Recorded { at, report: r });
            ack(Status::Ok)
        }
        HostMsg::ReleaseAll => {
            if !s.phone_connected {
                return ack(Status::NotConnected);
            }
            let mut rs = vec![
                Report::KEYS_UP,
                Report::Mouse {
                    buttons: 0,
                    dx: 0,
                    dy: 0,
                    wheel: 0,
                },
                Report::Consumer { eject: false },
            ];
            if let Some((x, y)) = s.last_abs {
                rs.push(Report::Absolute { buttons: 0, x, y });
            }
            for r in rs {
                s.reports.push(Recorded { at, report: r });
            }
            ack(Status::Ok)
        }
    }
}

#[async_trait]
impl Transport for MemoryTransport {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        for frame in self.deframer.push(bytes) {
            self.handle(&frame);
        }
        Ok(())
    }

    async fn read(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        if !self.noise.is_empty() {
            return Ok(std::mem::take(&mut self.noise));
        }
        if self.outbox.is_empty() {
            tokio::time::sleep(timeout).await;
            return Ok(Vec::new());
        }
        Ok(self.outbox.drain(..).collect())
    }

    fn name(&self) -> String {
        "memory".into()
    }
}

// ---------- protocol session ----------

/// A protocol session: sequence numbers, request/reply matching, handshake.
pub struct Link {
    transport: Box<dyn Transport>,
    deframer: Deframer,
    seq: u8,
    info: Option<DongleInfo>,
    /// Reply timeout for normal requests.
    pub timeout: Duration,
}

impl Link {
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self {
            transport,
            deframer: Deframer::default(),
            seq: 0,
            info: None,
            timeout: REPLY_TIMEOUT,
        }
    }

    pub fn port_name(&self) -> String {
        self.transport.name()
    }

    /// The HELLO_ACK of the last handshake.
    pub fn info(&self) -> Option<&DongleInfo> {
        self.info.as_ref()
    }

    /// Exchange HELLO / HELLO_ACK and check the protocol version.
    pub async fn hello(&mut self) -> Result<DongleInfo> {
        let mut last = None;
        // The first HELLO can be lost while a board boots or flushes its log.
        for _ in 0..3 {
            match self
                .request(
                    HostMsg::Hello {
                        version: PROTOCOL_VERSION,
                    },
                    HELLO_TIMEOUT / 3,
                )
                .await
            {
                Ok(DeviceMsg::HelloAck(info)) => {
                    if info.protocol != PROTOCOL_VERSION {
                        return Err(Error::setup(
                            format!(
                                "dongle on {} speaks protocol v{}, agentphone needs v{}",
                                self.port_name(),
                                info.protocol,
                                PROTOCOL_VERSION
                            ),
                            "Flash the firmware from this agentphone version (firmware/README.md).",
                        ));
                    }
                    self.info = Some(info.clone());
                    return Ok(info);
                }
                Ok(other) => last = Some(Error::protocol(format!("HELLO answered with {other:?}"))),
                Err(e @ Error::Timeout(_)) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(Error::setup(
            format!(
                "no AgentPhone dongle answered on {} ({})",
                self.port_name(),
                last.map_or_else(|| "no reply".into(), |e| e.to_string())
            ),
            "Check that the board runs the AgentPhone firmware (firmware/README.md) and that \
             this is its port. Press the board's RST button and try again.",
        ))
    }

    /// Ping; returns whether a phone is connected to the dongle.
    pub async fn ping(&mut self) -> Result<bool> {
        match self.request(HostMsg::Ping, self.timeout).await? {
            DeviceMsg::Pong { phone_connected } => Ok(phone_connected),
            DeviceMsg::Ack(s) => Err(status_error(s, &self.port_name())),
            other => Err(Error::protocol(format!("PING answered with {other:?}"))),
        }
    }

    /// Send one report and wait for its ACK.
    pub async fn send_report(&mut self, r: &Report) -> Result<()> {
        self.expect_ok(HostMsg::Report(r.to_bytes())).await
    }

    /// Release every key and button.
    pub async fn release_all(&mut self) -> Result<()> {
        self.expect_ok(HostMsg::ReleaseAll).await
    }

    async fn expect_ok(&mut self, m: HostMsg) -> Result<()> {
        match self.request(m, self.timeout).await? {
            DeviceMsg::Ack(0) => Ok(()),
            DeviceMsg::Ack(s) => Err(status_error(s, &self.port_name())),
            other => Err(Error::protocol(format!("expected ACK, got {other:?}"))),
        }
    }

    /// Send a message and wait for the reply with the same seq. LOG messages and
    /// stale replies are skipped.
    pub async fn request(&mut self, m: HostMsg, timeout: Duration) -> Result<DeviceMsg> {
        self.seq = self.seq.wrapping_add(1);
        if self.seq == 0 {
            self.seq = 1; // seq 0 is for unsolicited messages
        }
        let seq = self.seq;
        self.transport
            .write(&protocol::encode(&m.to_frame(seq)))
            .await?;
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::Timeout(timeout));
            }
            let bytes = self.transport.read(deadline - now).await?;
            for wire in self.deframer.push(&bytes) {
                let Ok(f) = protocol::decode(&wire) else {
                    tracing::trace!("dongle: dropped {} undecodable bytes", wire.len());
                    continue;
                };
                match DeviceMsg::from_frame(&f) {
                    Ok(DeviceMsg::Log(t)) => tracing::debug!("dongle: {t}"),
                    Ok(reply) if f.seq == seq => return Ok(reply),
                    Ok(_) => tracing::trace!("dongle: stale reply seq {}", f.seq),
                    Err(e) => tracing::debug!("dongle: {e}"),
                }
            }
        }
    }
}

fn status_error(s: u8, port: &str) -> Error {
    match Status::from_u8(s) {
        Some(Status::NotConnected) => Error::setup(
            format!("the dongle on {port} has no phone connected"),
            "Pair the iPhone with 'AgentPhone HID' in Settings > Bluetooth (BLE), or plug the \
             dongle into the iPhone (USB). Then turn on AssistiveTouch.",
        ),
        Some(Status::NotReady) | Some(Status::BadVersion) => Error::protocol(format!(
            "dongle on {port} refused the message ({:?}); reconnect",
            Status::from_u8(s)
        )),
        Some(st) => Error::protocol(format!("dongle on {port} answered {st:?}")),
        None => Error::protocol(format!("dongle on {port} answered status {s}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn handshake_and_report() {
        let (t, sim) = MemoryTransport::new();
        let mut link = Link::new(Box::new(t));
        let info = link.hello().await.unwrap();
        assert_eq!(info.protocol, PROTOCOL_VERSION);
        assert_eq!(info.link, LinkKind::Ble);
        assert!(link.ping().await.unwrap());
        link.send_report(&Report::rel(1, 2)).await.unwrap();
        assert_eq!(sim.reports(), vec![Report::rel(1, 2)]);
    }

    #[tokio::test(start_paused = true)]
    async fn version_mismatch_is_setup_error() {
        let (t, sim) = MemoryTransport::new();
        sim.set_protocol(2);
        let mut link = Link::new(Box::new(t));
        let e = link.hello().await.unwrap_err();
        assert!(e.hint().unwrap().contains("Flash the firmware"), "{e}");
    }

    #[tokio::test(start_paused = true)]
    async fn report_before_hello_is_refused() {
        let (t, _sim) = MemoryTransport::new();
        let mut link = Link::new(Box::new(t));
        assert!(link.send_report(&Report::KEYS_UP).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn no_phone_and_timeouts() {
        let (t, sim) = MemoryTransport::new();
        let mut link = Link::new(Box::new(t));
        link.hello().await.unwrap();
        sim.set_phone_connected(false);
        let e = link.send_report(&Report::KEYS_UP).await.unwrap_err();
        assert!(e.hint().unwrap().contains("AssistiveTouch"));
        sim.set_phone_connected(true);
        sim.drop_replies(1);
        assert!(matches!(
            link.send_report(&Report::KEYS_UP).await,
            Err(Error::Timeout(_))
        ));
        // The next request still matches its own reply.
        link.send_report(&Report::rel(0, 0)).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn release_all_keeps_position() {
        let (t, sim) = MemoryTransport::new();
        let mut link = Link::new(Box::new(t));
        link.hello().await.unwrap();
        link.send_report(&Report::abs(0.5, 0.5, true))
            .await
            .unwrap();
        sim.clear();
        link.release_all().await.unwrap();
        let r = sim.reports();
        assert_eq!(*r.last().unwrap(), Report::abs(0.5, 0.5, false));
    }
}
