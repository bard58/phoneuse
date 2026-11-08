//! Minimal async client for the adb server's "smart socket" protocol.
//!
//! Wire format (see AOSP `packages/modules/adb/SERVICES.TXT`):
//! a request is `<4 hex digits length><payload>`; the server answers `OKAY`
//! or `FAIL<4 hex length><message>`. Host services (`host:devices-l`,
//! `host:connect:...`) answer `OKAY<4 hex length><data>`. Device services
//! first switch the socket with `host:transport:<serial>`, then send
//! `exec:<command>`, which streams the command's output (binary safe, no PTY)
//! until EOF. `exec:` has no shell protocol, so stdout and stderr arrive merged
//! on one stream: redirect stderr (`2>/dev/null`) when the output must be clean.

use std::path::PathBuf;
use std::time::Duration;

use ap_core::{AndroidConfig, Error, Result};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::locate::{self, locate_adb};

/// Time allowed to open a TCP connection to the adb server.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Time allowed for a host service reply.
const HOST_TIMEOUT: Duration = Duration::from_secs(15);
/// Time allowed for one device command (uiautomator dump can be slow). It must
/// stay well above the longest gesture ([`crate::parse::MAX_GESTURE_MS`]).
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);
/// Time allowed for `adb start-server`.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// How the crate reaches devices. [`AdbServer`] is the real one; tests use a fake.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Run a host service (`devices-l`, `version`, `connect:ip:port`, ...)
    /// and return its length-prefixed reply. The `host:` prefix is added here.
    async fn host(&self, service: &str) -> Result<String>;

    /// Run a shell command on the device (`exec:` service) and return its
    /// output. stdout and stderr are merged on one stream.
    async fn exec(&self, serial: &str, command: &str) -> Result<Vec<u8>>;
}

/// Connection to an adb server. Starts a local server when it does not answer
/// (also later, if the server stops while this process runs).
pub struct AdbServer {
    config: AndroidConfig,
    /// Serializes start attempts. It holds no state, so a server that stops
    /// later is started again.
    start_lock: Mutex<()>,
}

impl AdbServer {
    pub fn new(config: AndroidConfig) -> Self {
        Self {
            config,
            start_lock: Mutex::new(()),
        }
    }

    pub fn config(&self) -> &AndroidConfig {
        &self.config
    }

    /// Configured host without IPv6 brackets (`[::1]` -> `::1`).
    fn host_name(&self) -> &str {
        let h = self.config.adb_host.trim();
        h.strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .unwrap_or(h)
    }

    /// `host:port` of the server.
    pub fn address(&self) -> String {
        let h = self.host_name();
        if h.contains(':') {
            format!("[{h}]:{}", self.config.adb_port)
        } else {
            format!("{h}:{}", self.config.adb_port)
        }
    }

    /// Path of the `adb` binary (config, PATH, SDK folders).
    pub fn adb_path(&self) -> Result<PathBuf> {
        locate_adb(&self.config)
    }

    /// Server protocol version (`host:version`), e.g. 41.
    pub async fn version(&self) -> Result<u32> {
        let v = self.host("version").await?;
        u32::from_str_radix(v.trim(), 16)
            .map_err(|_| Error::protocol(format!("bad adb version reply {v:?}")))
    }

    async fn open(&self) -> Result<TcpStream> {
        match self.try_connect().await {
            Ok(s) => Ok(s),
            Err(first) => {
                if !locate::is_loopback(self.host_name()) {
                    return Err(Error::setup(
                        format!("cannot reach the adb server at {}: {first}", self.address()),
                        "start adb on that host (`adb -a nodaemon server start`) or set \
                         android.adb_host to 127.0.0.1 in the config",
                    ));
                }
                self.start_server().await?;
                self.try_connect().await.map_err(|e| {
                    Error::setup(
                        format!("adb server did not answer at {}: {e}", self.address()),
                        "run `adb kill-server` then `adb start-server`, and check that no \
                         firewall blocks 127.0.0.1",
                    )
                })
            }
        }
    }

    async fn try_connect(&self) -> std::io::Result<TcpStream> {
        let addr = (self.host_name(), self.config.adb_port);
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(r) => r,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connect timed out",
            )),
        }
    }

    /// Start the local adb server, then wait until it answers. Concurrent
    /// callers wait for one attempt; a caller that finds the server already
    /// answering returns at once.
    async fn start_server(&self) -> Result<()> {
        let _guard = self.start_lock.lock().await;
        if self.try_connect().await.is_ok() {
            return Ok(());
        }
        let adb = self.adb_path()?;
        tracing::info!(adb = %adb.display(), "starting adb server");
        // Windows: no direct-spawn fallback. A direct child lets the long-lived
        // daemon inherit our stdio pipes (see launch_detached_windows).
        #[cfg(windows)]
        self.launch_detached_windows(&adb).await.map_err(|e| {
            Error::setup(
                format!("could not start the adb server in the background: {e}"),
                "run `adb start-server` once in a terminal, then retry",
            )
        })?;
        #[cfg(not(windows))]
        self.launch_direct(&adb).await?;
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if self.try_connect().await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(Error::setup(
            format!("the adb server did not start within {START_TIMEOUT:?}"),
            "run `adb start-server` in a terminal to see the error; another program may \
             use port 5037",
        ))
    }

    /// Windows: `adb start-server` started from our process lets the long-lived
    /// daemon inherit our inheritable handles (e.g. the stdout pipe of an MCP
    /// stdio server), so the parent never sees EOF. PowerShell `Start-Process`
    /// (without `-Wait`) uses ShellExecute, which passes no handles.
    #[cfg(windows)]
    async fn launch_detached_windows(&self, adb: &std::path::Path) -> Result<()> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let path = adb.display().to_string().replace('\'', "''");
        let script = format!(
            "Start-Process -FilePath '{path}' -ArgumentList '-P','{}','start-server' \
             -WindowStyle Hidden",
            self.config.adb_port
        );
        let mut cmd = tokio::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let status = tokio::time::timeout(START_TIMEOUT, cmd.status())
            .await
            .map_err(|_| Error::Timeout(START_TIMEOUT))??;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Other(format!("Start-Process failed ({status})")))
        }
    }

    /// Run `adb -P <port> start-server` as a child and wait for it.
    #[cfg(not(windows))]
    async fn launch_direct(&self, adb: &std::path::Path) -> Result<()> {
        let mut cmd = tokio::process::Command::new(adb);
        cmd.arg("-P")
            .arg(self.config.adb_port.to_string())
            .arg("start-server")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(false);
        let status = tokio::time::timeout(START_TIMEOUT, cmd.status())
            .await
            .map_err(|_| Error::Timeout(START_TIMEOUT))?
            .map_err(|e| {
                Error::setup(
                    format!("cannot run {}: {e}", adb.display()),
                    locate::INSTALL_HINT,
                )
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::setup(
                format!("`adb start-server` failed ({status})"),
                "run `adb start-server` in a terminal to see the error; another program \
                 may use port 5037",
            ))
        }
    }

    /// Run `adb pair host:port code` (Android 11+ wireless debugging).
    /// Pairing uses SPAKE2/TLS, which this client does not speak, so it uses the adb binary.
    pub async fn pair(&self, address: &str, code: &str) -> Result<String> {
        if !is_host_port(address) || code.is_empty() || !code.chars().all(|c| c.is_ascii_digit()) {
            return Err(Error::InvalidArgument(
                "pair needs ip:port and the 6-digit code from Developer options > Wireless \
                 debugging > Pair device with pairing code"
                    .into(),
            ));
        }
        let adb = self.adb_path()?;
        let mut cmd = tokio::process::Command::new(&adb);
        cmd.arg("-P")
            .arg(self.config.adb_port.to_string())
            .args(["pair", address, code])
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = tokio::time::timeout(Duration::from_secs(30), cmd.output())
            .await
            .map_err(|_| Error::Timeout(Duration::from_secs(30)))??;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.success() && text.contains("Successfully paired") {
            Ok(text.trim().to_string())
        } else {
            Err(Error::setup(
                format!("adb pair {address} failed: {}", text.trim()),
                "open Developer options > Wireless debugging > Pair device with pairing code \
                 on the phone, and use the IP:port and code shown in that dialog (the phone \
                 and PC must be on the same network)",
            ))
        }
    }
}

#[async_trait]
impl Transport for AdbServer {
    async fn host(&self, service: &str) -> Result<String> {
        let mut s = self.open().await?;
        let fut = async {
            send(&mut s, &format!("host:{service}")).await?;
            read_status(&mut s).await?;
            read_hex_string(&mut s).await
        };
        tokio::time::timeout(HOST_TIMEOUT, fut)
            .await
            .map_err(|_| Error::Timeout(HOST_TIMEOUT))?
    }

    async fn exec(&self, serial: &str, command: &str) -> Result<Vec<u8>> {
        let mut s = self.open().await?;
        tracing::debug!(serial, command, "adb exec");
        let fut = async {
            send(&mut s, &format!("host:transport:{serial}")).await?;
            read_status(&mut s).await?;
            send(&mut s, &format!("exec:{command}")).await?;
            read_status(&mut s).await?;
            let mut out = Vec::new();
            s.read_to_end(&mut out).await?;
            Ok(out)
        };
        tokio::time::timeout(EXEC_TIMEOUT, fut)
            .await
            .map_err(|_| Error::Timeout(EXEC_TIMEOUT))?
    }
}

/// True for `a.b.c.d:port`, `host:port` or `[v6]:port`.
pub(crate) fn is_host_port(s: &str) -> bool {
    match s.rsplit_once(':') {
        Some((h, p)) => {
            !h.is_empty()
                && p.parse::<u16>().is_ok()
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '[' | ']' | ':'))
        }
        None => false,
    }
}

/// Encode one request: 4 hex digits of length, then the payload.
pub(crate) fn encode_request(payload: &str) -> Result<Vec<u8>> {
    if payload.len() > 0xFFFF {
        return Err(Error::InvalidArgument(format!(
            "adb request too long ({} bytes)",
            payload.len()
        )));
    }
    let mut v = format!("{:04x}", payload.len()).into_bytes();
    v.extend_from_slice(payload.as_bytes());
    Ok(v)
}

async fn send(s: &mut TcpStream, payload: &str) -> Result<()> {
    s.write_all(&encode_request(payload)?).await?;
    Ok(())
}

async fn read_status(s: &mut TcpStream) -> Result<()> {
    let mut st = [0u8; 4];
    s.read_exact(&mut st).await?;
    match &st {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let msg = read_hex_string(s).await.unwrap_or_default();
            Err(map_failure(&msg))
        }
        other => Err(Error::protocol(format!(
            "unexpected adb status {:?}",
            String::from_utf8_lossy(other)
        ))),
    }
}

async fn read_hex_string(s: &mut TcpStream) -> Result<String> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await?;
    let n = parse_hex_len(&len)?;
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) fn parse_hex_len(b: &[u8; 4]) -> Result<usize> {
    std::str::from_utf8(b)
        .ok()
        .and_then(|t| usize::from_str_radix(t, 16).ok())
        .ok_or_else(|| Error::protocol(format!("bad adb length {:?}", String::from_utf8_lossy(b))))
}

/// Turn an adb `FAIL` message into a typed error with a fix hint.
pub(crate) fn map_failure(msg: &str) -> Error {
    let m = msg.to_ascii_lowercase();
    if m.contains("unauthorized") {
        Error::setup(format!("adb: {msg}"), crate::provider::UNAUTHORIZED_HINT)
    } else if m.contains("offline") {
        Error::setup(
            format!("adb: {msg}"),
            "unplug and replug the cable (or run `adb reconnect`), then unlock the phone",
        )
    } else if m.contains("no permissions") {
        Error::setup(
            format!("adb: {msg}"),
            "the PC has no permission to open the USB device; on Windows install the Google \
             USB driver or the phone maker's driver",
        )
    } else if m.contains("not found") || m.contains("no devices") {
        Error::DeviceNotFound(msg.to_string())
    } else {
        Error::protocol(format!("adb: {msg}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_encoding() {
        assert_eq!(encode_request("host:version").unwrap(), b"000chost:version");
        assert_eq!(encode_request("").unwrap(), b"0000");
        assert!(encode_request(&"x".repeat(0x10000)).is_err());
        assert_eq!(parse_hex_len(b"001a").unwrap(), 26);
        assert!(parse_hex_len(b"zzzz").is_err());
    }

    #[test]
    fn failure_mapping() {
        let e = map_failure("device unauthorized.\nThis adb server's $ADB_VENDOR_KEYS is not set");
        assert!(e.hint().unwrap().contains("USB debugging"));
        assert!(matches!(
            map_failure("device 'abc' not found"),
            Error::DeviceNotFound(_)
        ));
        assert!(map_failure("device offline").hint().is_some());
        assert!(matches!(map_failure("closed"), Error::Protocol(_)));
    }

    #[test]
    fn host_port() {
        assert!(is_host_port("192.168.1.20:5555"));
        assert!(is_host_port("[fe80::1]:37000"));
        assert!(!is_host_port("emulator-5554"));
        assert!(!is_host_port("1.2.3.4:x"));
        assert!(!is_host_port("a;rm:5"));
    }

    #[test]
    fn address_format() {
        let s = AdbServer::new(AndroidConfig::default());
        assert_eq!(s.address(), "127.0.0.1:5037");
        for host in ["[::1]", "::1", " [::1] "] {
            let s = AdbServer::new(AndroidConfig {
                adb_host: host.into(),
                ..AndroidConfig::default()
            });
            assert_eq!(s.address(), "[::1]:5037", "{host:?}");
            assert_eq!(s.host_name(), "::1");
        }
    }

    /// Config for a server on `port` whose adb binary does not exist, so any
    /// launch attempt fails with a recognizable error.
    fn missing_adb(port: u16) -> AndroidConfig {
        AndroidConfig {
            adb_path: Some(std::env::temp_dir().join("agentphone-no-such-adb.exe")),
            adb_host: "127.0.0.1".into(),
            adb_port: port,
        }
    }

    #[tokio::test]
    async fn bracketed_ipv6_host_connects() {
        let Ok(l) = tokio::net::TcpListener::bind("[::1]:0").await else {
            return; // no IPv6 loopback on this machine
        };
        let s = AdbServer::new(AndroidConfig {
            adb_host: "[::1]".into(),
            ..missing_adb(l.local_addr().unwrap().port())
        });
        assert!(s.try_connect().await.is_ok());
    }

    #[tokio::test]
    async fn server_is_started_again_after_it_stops() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let s = AdbServer::new(missing_adb(port));
        // Server answers: no launch needed.
        s.start_server().await.unwrap();
        s.open().await.unwrap();
        // Server stops: the next open must try to launch adb again (and fail
        // here on the missing binary) instead of trusting an old "started" flag.
        drop(l);
        let e = s.open().await.unwrap_err().to_string();
        assert!(e.contains("does not exist"), "{e}");
    }
}
