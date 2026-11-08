//! mDNS browsing for iPhones on the local network (crate `mdns-sd`).
//!
//! * `_apple-mobdev2._tcp`: lockdown over Wi-Fi. The instance name is
//!   `<wifi mac>@<ipv6 link-local>`, so it matches the `WiFiMACAddress` of a
//!   lockdown pairing file.
//! * `_remotepairing._tcp`: RemotePairing (iOS 17+). TXT `identifier` and
//!   `authTag` match a saved RemotePairing file through its `alt_irk`.
//!
//! idevice 0.1.68 has no browser. Not verified against a real iPhone.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub const MOBDEV2: &str = "_apple-mobdev2._tcp.local.";
pub const REMOTE_PAIRING: &str = "_remotepairing._tcp.local.";

/// One resolved service instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub service: &'static str,
    /// Instance name without the service suffix.
    pub instance: String,
    /// Addresses with the IPv6 scope id (0 for IPv4).
    pub addrs: Vec<(IpAddr, u32)>,
    pub port: u16,
    pub txt: HashMap<String, String>,
}

impl Found {
    /// Best address: IPv4 first, then IPv6 (with scope).
    pub fn best_addr(&self) -> Option<(IpAddr, Option<u32>)> {
        let v4 = self.addrs.iter().find(|(a, _)| a.is_ipv4());
        let any = v4.or(self.addrs.first());
        any.map(|(a, s)| {
            (
                *a,
                if a.is_ipv6() && *s != 0 {
                    Some(*s)
                } else {
                    None
                },
            )
        })
    }

    /// For `_apple-mobdev2._tcp`: the Wi-Fi MAC (lower case) from the instance name.
    pub fn mobdev2_mac(&self) -> Option<String> {
        parse_mobdev2_mac(&self.instance)
    }
}

/// `aa:bb:cc:dd:ee:ff@fe80::1` -> `aa:bb:cc:dd:ee:ff`.
pub fn parse_mobdev2_mac(instance: &str) -> Option<String> {
    let (mac, _) = instance.split_once('@')?;
    let parts: Vec<&str> = mac.split(':').collect();
    let ok = parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
    ok.then(|| mac.to_ascii_lowercase())
}

/// Normalize a MAC for comparison.
pub fn normalize_mac(mac: &str) -> String {
    mac.trim().to_ascii_lowercase().replace('-', ":")
}

fn instance_of(fullname: &str, ty: &str) -> String {
    fullname
        .strip_suffix(ty)
        .map(|s| s.trim_end_matches('.'))
        .unwrap_or(fullname)
        .to_string()
}

/// Browse `services` for `timeout` (blocking). Errors give an empty list:
/// mDNS is a best-effort extra on top of usbmuxd and saved records.
pub fn browse_blocking(services: &[&'static str], timeout: Duration) -> Vec<Found> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::debug!("mDNS unavailable: {e}");
            return Vec::new();
        }
    };
    let mut rxs = Vec::new();
    for s in services {
        match daemon.browse(s) {
            Ok(rx) => rxs.push((*s, rx)),
            Err(e) => tracing::debug!("mDNS browse {s}: {e}"),
        }
    }
    let deadline = Instant::now() + timeout;
    let mut out: Vec<Found> = Vec::new();
    while Instant::now() < deadline {
        let mut any = false;
        for (ty, rx) in &rxs {
            while let Ok(ev) = rx.try_recv() {
                any = true;
                if let mdns_sd::ServiceEvent::ServiceResolved(r) = ev {
                    let addrs = r
                        .addresses
                        .iter()
                        .map(|a| {
                            let scope = match a {
                                mdns_sd::ScopedIp::V6(v6) => v6.scope_id().index,
                                _ => 0,
                            };
                            (a.to_ip_addr(), scope)
                        })
                        .collect();
                    let txt = r
                        .txt_properties
                        .iter()
                        .map(|p| (p.key().to_string(), p.val_str().to_string()))
                        .collect();
                    let f = Found {
                        service: ty,
                        instance: instance_of(&r.fullname, ty),
                        addrs,
                        port: r.port,
                        txt,
                    };
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
        if !any {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = daemon.shutdown();
    out
}

/// Async wrapper: runs the browse on a blocking thread.
pub async fn browse(services: &[&'static str], timeout: Duration) -> Vec<Found> {
    let services = services.to_vec();
    tokio::task::spawn_blocking(move || browse_blocking(&services, timeout))
        .await
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobdev2_instance() {
        assert_eq!(
            parse_mobdev2_mac("AA:bb:cc:dd:ee:ff@fe80::1c2b:3a4d"),
            Some("aa:bb:cc:dd:ee:ff".into())
        );
        assert_eq!(parse_mobdev2_mac("nope"), None);
        assert_eq!(parse_mobdev2_mac("aa:bb@fe80::1"), None);
        assert_eq!(normalize_mac("AA-BB-CC-DD-EE-FF"), "aa:bb:cc:dd:ee:ff");
        assert_eq!(
            instance_of(
                "aa:bb:cc:dd:ee:ff@fe80::1._apple-mobdev2._tcp.local.",
                MOBDEV2
            ),
            "aa:bb:cc:dd:ee:ff@fe80::1"
        );
    }

    #[test]
    fn best_addr_prefers_v4() {
        let f = Found {
            service: MOBDEV2,
            instance: "x".into(),
            addrs: vec![
                ("fe80::1".parse().unwrap(), 7),
                ("192.168.1.2".parse().unwrap(), 0),
            ],
            port: 62078,
            txt: HashMap::new(),
        };
        assert_eq!(f.best_addr(), Some(("192.168.1.2".parse().unwrap(), None)));
    }
}
