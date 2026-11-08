//! [`IosProvider`]: discovery over usbmuxd, saved Wi-Fi pairings and mDNS.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use ap_core::{
    Device, DeviceInfo, DeviceProvider, Error, IosConfig, Platform, Result, async_trait,
};
use idevice::{pairing_file::PairingFile, provider::IdeviceProvider, usbmuxd::Connection};

use crate::IosDevice;
use crate::connect::{self, LockdownInfo};
use crate::errors::PAIR_HINT;
use crate::hid::TunnelSource;
use crate::mdns::{self, Found};
use crate::models;
use crate::store::{self, DeviceRecord};
use crate::tunnel;
use crate::version::IosVersion;
use crate::worker::offload;

/// Lockdown port on the device.
const LOCKDOWN_PORT: u16 = 62078;
/// Time limit for one lockdown query during discovery.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Finds iPhones and connects them.
#[derive(Debug, Clone)]
pub struct IosProvider {
    cfg: IosConfig,
    mdns_timeout: Duration,
}

impl IosProvider {
    pub fn new(cfg: IosConfig) -> Self {
        Self {
            cfg,
            mdns_timeout: Duration::from_millis(1500),
        }
    }

    /// How long discovery listens for mDNS (default 1.5 s; zero disables it).
    pub fn with_mdns_timeout(mut self, t: Duration) -> Self {
        self.mdns_timeout = t;
        self
    }

    pub fn config(&self) -> &IosConfig {
        &self.cfg
    }

    /// Connect and return the concrete type (for iOS-only calls like `pinch`).
    pub async fn connect_ios(&self, id: &str) -> Result<IosDevice> {
        IosDevice::connect(&self.cfg, id).await
    }
}

impl Default for IosProvider {
    fn default() -> Self {
        Self::new(IosConfig::default())
    }
}

/// Build a [`DeviceInfo`] from lockdown values.
pub(crate) fn device_info(udid: &str, li: &LockdownInfo, backend: &str) -> DeviceInfo {
    let (points, pixels, _) = models::screen_sizes(&li.product_type);
    DeviceInfo {
        id: udid.to_string(),
        name: if li.name.is_empty() {
            models::marketing_name(&li.product_type)
        } else {
            li.name.clone()
        },
        platform: Platform::Ios,
        os_version: li.product_version.clone(),
        model: li.product_type.clone(),
        screen_points: points,
        screen_pixels: pixels,
        backend: backend.to_string(),
    }
}

/// Backend label for discovery output: "hid" when the route applies.
fn backend_for(version: &str) -> &'static str {
    match IosVersion::parse(version) {
        Some(v) if v.has_core_device_proxy() => "hid",
        _ => "unsupported",
    }
}

fn record_info(r: &DeviceRecord) -> DeviceInfo {
    let li = LockdownInfo {
        udid: r.udid.clone(),
        name: r.name.clone(),
        product_type: r.product_type.clone(),
        product_version: r.os_version.clone(),
        ..LockdownInfo::default()
    };
    device_info(&r.udid, &li, backend_for(&r.os_version))
}

/// Where a saved device is on the network.
#[derive(Debug, Clone, Default)]
struct NetworkSpot {
    lockdown: Option<(IpAddr, Option<u32>)>,
    remote_pairing: Option<SocketAddr>,
}

async fn tcp_reachable(ip: IpAddr, port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(400),
            tokio::net::TcpStream::connect((ip, port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// True when a `_apple-mobdev2._tcp` entry belongs to the saved device: by
/// the Wi-Fi MAC in the instance name, or (iOS 26.4+) by the `identifier`
/// and `authTag` TXT values derived from the pairing's HostID. mdns-sd keeps
/// only the first value of a repeated TXT key, so only one `authTag` is
/// checked.
pub(crate) fn mobdev2_matches(f: &Found, mac: &str, host_id: Option<&str>) -> bool {
    if f.service != mdns::MOBDEV2 {
        return false;
    }
    if !mac.is_empty() && f.mobdev2_mac().as_deref() == Some(mac) {
        return true;
    }
    match (host_id, f.txt.get("identifier"), f.txt.get("authTag")) {
        (Some(h), Some(id), Some(tag)) => {
            idevice::mdns::txt_record_matches(h.as_bytes(), id.as_bytes(), &[tag.as_bytes()])
        }
        _ => false,
    }
}

/// Find a saved device on the network: mDNS first, then the last IP.
async fn locate(dir: &std::path::Path, rec: &DeviceRecord, found: &[Found]) -> NetworkSpot {
    let mut spot = NetworkSpot::default();
    let mac = mdns::normalize_mac(&rec.wifi_mac);
    let host_id = store::paths(dir, &rec.udid)
        .ok()
        .and_then(|p| PairingFile::read_from_file(&p.lockdown).ok())
        .map(|pf| pf.host_id);
    spot.lockdown = found
        .iter()
        .find(|f| mobdev2_matches(f, &mac, host_id.as_deref()))
        .and_then(Found::best_addr);
    if let Ok(p) = store::paths(dir, &rec.udid)
        && let Ok(bytes) = std::fs::read(&p.remote_pairing)
        && let Ok(rpf) = idevice::remote_pairing::RpPairingFile::from_bytes(&bytes)
    {
        spot.remote_pairing = found
            .iter()
            .filter(|f| f.service == mdns::REMOTE_PAIRING)
            .find(|f| {
                let id = f.txt.get("identifier").map(String::as_str).unwrap_or("");
                let tag = f.txt.get("authTag").map(String::as_str).unwrap_or("");
                tunnel::rp_matches(&rpf, id, tag)
            })
            .and_then(|f| {
                f.best_addr().map(|(ip, scope)| match (ip, scope) {
                    (IpAddr::V6(v6), Some(s)) => {
                        SocketAddr::V6(std::net::SocketAddrV6::new(v6, f.port, 0, s))
                    }
                    _ => SocketAddr::new(ip, f.port),
                })
            });
    }
    if spot.lockdown.is_none()
        && let Some(ip) = rec.last_ip
        && tcp_reachable(ip, LOCKDOWN_PORT).await
    {
        spot.lockdown = Some((ip, None));
    }
    spot
}

/// A device resolved for connection.
pub(crate) struct Resolved {
    pub info: DeviceInfo,
    pub version: IosVersion,
    /// Lockdown provider (USB or Wi-Fi TCP), if any.
    pub lockdown: Option<Arc<dyn IdeviceProvider>>,
    pub remote_pairing: Option<(SocketAddr, std::path::PathBuf)>,
}

impl Resolved {
    /// Tunnel sources in the order to try.
    pub fn sources(&self) -> Vec<TunnelSource> {
        let mut v = Vec::new();
        if let Some(p) = &self.lockdown {
            v.push(TunnelSource::Lockdown(p.clone()));
        }
        if let Some((addr, path)) = &self.remote_pairing {
            v.push(TunnelSource::RemotePairing {
                addr: *addr,
                path: path.clone(),
            });
        }
        v
    }
}

/// Parse `ProductVersion`. The version gate itself is in `select_backends`.
fn parse_version(v: &str) -> Result<IosVersion> {
    IosVersion::parse(v).ok_or_else(|| Error::protocol(format!("bad ProductVersion {v:?}")))
}

/// Pick a UDID: exact match, else a unique prefix.
pub(crate) fn pick_udid<'a>(id: &str, candidates: impl Iterator<Item = &'a str>) -> Result<String> {
    let all: Vec<&str> = candidates.collect();
    if let Some(u) = all.iter().find(|u| u.eq_ignore_ascii_case(id)) {
        return Ok(u.to_string());
    }
    let pre: Vec<&&str> = all
        .iter()
        .filter(|u| !id.is_empty() && u.to_ascii_lowercase().starts_with(&id.to_ascii_lowercase()))
        .collect();
    match pre.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(Error::DeviceNotFound(id.to_string())),
        _ => Err(Error::InvalidArgument(format!(
            "device id {id:?} matches more than one iPhone; give more characters"
        ))),
    }
}

/// Resolve `id` to providers. Runs on a worker thread.
pub(crate) async fn resolve(cfg: &IosConfig, id: &str) -> Result<Resolved> {
    let dir = store::pairing_dir(cfg)?;
    let records = store::list_records(&dir);
    let mux_devs = match connect::list_usbmuxd().await {
        Ok(d) => d,
        // No usbmuxd and no saved Wi-Fi pairing: nothing can work.
        Err(e) if records.is_empty() => return Err(e),
        Err(e) => {
            tracing::warn!("usbmuxd: {e}; trying Wi-Fi only");
            Vec::new()
        }
    };
    let udid = pick_udid(
        id,
        mux_devs
            .iter()
            .map(|d| d.udid.as_str())
            .chain(records.iter().map(|r| r.udid.as_str())),
    )?;

    // 1. usbmuxd (USB first, then usbmuxd's Wi-Fi sync entry).
    if let Some(dev) = mux_devs.iter().find(|d| d.udid == udid) {
        let p: Arc<dyn IdeviceProvider> = Arc::from(connect::usbmuxd_provider(dev)?);
        let li = tokio::time::timeout(QUERY_TIMEOUT, Box::pin(connect::lockdown_info(&*p)))
            .await
            .map_err(|_| Error::Timeout(QUERY_TIMEOUT))??;
        if !li.trusted {
            return Err(Error::setup(
                "this computer is not trusted by the iPhone",
                PAIR_HINT,
            ));
        }
        let version = parse_version(&li.product_version)?;
        return Ok(Resolved {
            info: device_info(&udid, &li, "hid"),
            version,
            lockdown: Some(p),
            remote_pairing: None,
        });
    }

    // 2. Saved Wi-Fi pairing.
    let rec = records
        .iter()
        .find(|r| r.udid == udid)
        .cloned()
        .ok_or_else(|| Error::DeviceNotFound(udid.clone()))?;
    let found = mdns::browse(
        &[mdns::MOBDEV2, mdns::REMOTE_PAIRING],
        Duration::from_millis(1500),
    )
    .await;
    let spot = locate(&dir, &rec, &found).await;
    let paths = store::paths(&dir, &udid)?;
    let mut lockdown: Option<Arc<dyn IdeviceProvider>> = None;
    let mut info = record_info(&rec);
    let mut version = IosVersion::parse(&rec.os_version);
    if let Some((ip, scope)) = spot.lockdown
        && let Ok(pf) = PairingFile::read_from_file(&paths.lockdown)
    {
        let p: Arc<dyn IdeviceProvider> = Arc::from(connect::tcp_provider(ip, scope, pf));
        match tokio::time::timeout(QUERY_TIMEOUT, Box::pin(connect::lockdown_info(&*p))).await {
            Ok(Ok(li)) if li.trusted => {
                version = IosVersion::parse(&li.product_version).or(version);
                info = device_info(&udid, &li, "hid");
                lockdown = Some(p);
            }
            Ok(Ok(_)) => tracing::info!("Wi-Fi lockdown at {ip}: not trusted"),
            Ok(Err(e)) => tracing::info!("Wi-Fi lockdown at {ip}: {e}"),
            Err(_) => tracing::info!("Wi-Fi lockdown at {ip}: timed out"),
        }
    }
    let remote_pairing = spot
        .remote_pairing
        .map(|a| (a, paths.remote_pairing.clone()));
    if lockdown.is_none() && remote_pairing.is_none() {
        return Err(Error::setup(
            format!("{} is not reachable over Wi-Fi", info.name),
            "put the iPhone and this computer on the same Wi-Fi and unlock the iPhone, or connect it with a cable",
        ));
    }
    let version = version.ok_or_else(|| Error::protocol("unknown iOS version for this device"))?;
    Ok(Resolved {
        info,
        version,
        lockdown,
        remote_pairing,
    })
}

/// Discovery body. Runs on a worker thread.
pub(crate) async fn discover_all(
    cfg: IosConfig,
    mdns_timeout: Duration,
) -> Result<Vec<DeviceInfo>> {
    let dir = store::pairing_dir(&cfg)?;
    let records = store::list_records(&dir);
    let mut out: Vec<DeviceInfo> = Vec::new();

    match connect::list_usbmuxd().await {
        Ok(devs) => {
            for d in devs {
                if out.iter().any(|i| i.id == d.udid) {
                    continue;
                }
                let p = connect::usbmuxd_provider(&d)?;
                match tokio::time::timeout(QUERY_TIMEOUT, Box::pin(connect::lockdown_info(&*p)))
                    .await
                {
                    Ok(Ok(li)) => {
                        let backend = if li.trusted {
                            backend_for(&li.product_version)
                        } else {
                            "untrusted"
                        };
                        out.push(device_info(&d.udid, &li, backend));
                        // Keep a saved Wi-Fi record fresh.
                        if let Some(mut rec) = store::load_record(&dir, &d.udid) {
                            rec.name = li.name.clone();
                            rec.os_version = li.product_version.clone();
                            rec.product_type = li.product_type.clone();
                            if let Connection::Network(ip) = d.connection_type {
                                rec.last_ip = Some(ip);
                            }
                            let _ = store::save_record(&dir, &rec);
                        }
                    }
                    Ok(Err(e)) => tracing::warn!("{}: lockdown failed: {e}", d.udid),
                    Err(_) => tracing::warn!("{}: lockdown timed out", d.udid),
                }
            }
        }
        // No usbmuxd and nothing saved: the user must install it.
        Err(e) if records.is_empty() => return Err(e),
        Err(e) => tracing::warn!("usbmuxd: {e}; listing Wi-Fi devices only"),
    }

    let pending: Vec<&DeviceRecord> = records
        .iter()
        .filter(|r| !out.iter().any(|i| i.id == r.udid))
        .collect();
    if !pending.is_empty() {
        let found = if mdns_timeout.is_zero() {
            Vec::new()
        } else {
            mdns::browse(&[mdns::MOBDEV2, mdns::REMOTE_PAIRING], mdns_timeout).await
        };
        for rec in pending {
            let spot = locate(&dir, rec, &found).await;
            if spot.lockdown.is_some() || spot.remote_pairing.is_some() {
                out.push(record_info(rec));
            }
        }
    }
    Ok(out)
}

#[async_trait]
impl DeviceProvider for IosProvider {
    fn name(&self) -> &str {
        "ios"
    }

    /// List iPhones over usbmuxd (USB and Wi-Fi sync) and saved Wi-Fi
    /// pairings found on the network. Devices below iOS 17.4 are listed with
    /// backend `"unsupported"`; untrusted ones with `"untrusted"`.
    async fn discover(&self) -> Result<Vec<DeviceInfo>> {
        let cfg = self.cfg.clone();
        let t = self.mdns_timeout;
        offload(move || discover_all(cfg, t)).await
    }

    async fn connect(&self, id: &str) -> Result<Box<dyn Device>> {
        Ok(Box::new(self.connect_ios(id).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udid_picking() {
        let ids = ["00008110-AAAA", "00008110-BBBB", "abcdef"];
        assert_eq!(pick_udid("abcdef", ids.iter().copied()).unwrap(), "abcdef");
        assert_eq!(
            pick_udid("00008110-a", ids.iter().copied()).unwrap(),
            "00008110-AAAA"
        );
        assert!(matches!(
            pick_udid("00008110", ids.iter().copied()),
            Err(Error::InvalidArgument(_))
        ));
        assert!(matches!(
            pick_udid("zzz", ids.iter().copied()),
            Err(Error::DeviceNotFound(_))
        ));
        assert!(matches!(
            pick_udid("", ids.iter().copied()),
            Err(Error::DeviceNotFound(_))
        ));
    }

    #[test]
    fn info_from_record() {
        let r = DeviceRecord {
            udid: "u1".into(),
            product_type: "iPhone14,5".into(),
            os_version: "16.7".into(),
            ..DeviceRecord::default()
        };
        let i = record_info(&r);
        assert_eq!(i.name, "iPhone 13");
        assert_eq!(i.screen_points, ap_core::Size::new(390.0, 844.0));
        assert_eq!(i.backend, "unsupported");
        assert_eq!(backend_for("26.1"), "hid");
    }

    #[test]
    fn mobdev2_match_by_mac_or_auth_tag() {
        fn b64(bytes: &[u8]) -> String {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for c in bytes.chunks(3) {
                let n = (u32::from(c[0]) << 16)
                    | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                    | u32::from(*c.get(2).unwrap_or(&0));
                for i in 0..4 {
                    if i <= c.len() {
                        out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                    } else {
                        out.push('=');
                    }
                }
            }
            out
        }
        let host_id = "6A1B2C3D-0000-1111-2222-333344445555";
        let identifier = "ABCDEF012345";
        let tag = b64(&idevice::mdns::derive_auth_tag(
            host_id.as_bytes(),
            identifier.as_bytes(),
        ));
        let mut f = Found {
            service: mdns::MOBDEV2,
            instance: "aa:bb:cc:dd:ee:ff@fe80::1".into(),
            addrs: vec![("192.168.1.2".parse().unwrap(), 0)],
            port: 62078,
            txt: Default::default(),
        };
        assert!(mobdev2_matches(&f, "aa:bb:cc:dd:ee:ff", None));
        assert!(!mobdev2_matches(&f, "11:22:33:44:55:66", Some(host_id)));
        // iOS 26.4+: no MAC match, but the auth tag matches the HostID.
        f.instance = "random-name".into();
        f.txt.insert("identifier".into(), identifier.into());
        f.txt.insert("authTag".into(), tag);
        assert!(mobdev2_matches(&f, "aa:bb:cc:dd:ee:ff", Some(host_id)));
        assert!(!mobdev2_matches(&f, "", Some("other-host")));
        f.service = mdns::REMOTE_PAIRING;
        assert!(!mobdev2_matches(&f, "", Some(host_id)));
    }

    #[tokio::test]
    async fn discover_without_usbmuxd_gives_setup_hint() {
        let dir = std::env::temp_dir().join(format!("ap-ios-disc-{}", std::process::id()));
        let cfg = IosConfig {
            pairing_dir: Some(dir),
            ..IosConfig::default()
        };
        let p = IosProvider::new(cfg).with_mdns_timeout(Duration::ZERO);
        // With usbmuxd installed this lists devices; without it the error
        // must carry the install hint.
        if let Err(e) = p.discover().await {
            assert_eq!(e.hint(), Some(connect::usbmuxd_hint()), "{e}");
        }
    }
}
