//! [`AndroidProvider`]: discovery, connect, Wi-Fi connect and pairing.

use std::sync::Arc;

use ap_core::{AndroidConfig, Device, DeviceInfo, DeviceProvider, Error, Platform, Result, Size};
use async_trait::async_trait;

use crate::adb::{self, AdbServer, Transport};
use crate::device::{AndroidDevice, query_info};
use crate::parse::{self, DeviceEntry, DeviceState};

/// Fix hint for devices that did not accept this PC yet.
pub const UNAUTHORIZED_HINT: &str = "unlock the phone and accept the \"Allow USB debugging?\" \
     prompt (tick \"Always allow from this computer\"); if no prompt shows, revoke USB debugging \
     authorizations in Developer options and replug the cable";

/// Finds Android devices through the adb server.
pub struct AndroidProvider {
    transport: Arc<dyn Transport>,
    server: Option<Arc<AdbServer>>,
}

impl AndroidProvider {
    /// Provider using the adb server described by `config`.
    pub fn new(config: AndroidConfig) -> Self {
        let server = Arc::new(AdbServer::new(config));
        Self {
            transport: server.clone(),
            server: Some(server),
        }
    }

    /// Provider over any transport (tests, proxies). Pairing is unavailable.
    pub fn with_transport(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            server: None,
        }
    }

    /// Raw `adb devices -l` entries, every state (for `doctor`-style output).
    pub async fn entries(&self) -> Result<Vec<DeviceEntry>> {
        Ok(parse::parse_devices(
            &self.transport.host("devices-l").await?,
        ))
    }

    /// `adb connect ip:port` for Wi-Fi debugging. Returns the server's message.
    pub async fn connect_wifi(&self, address: &str) -> Result<String> {
        let address = if address.contains(':') {
            address.to_string()
        } else {
            format!("{address}:5555")
        };
        if !adb::is_host_port(&address) {
            return Err(Error::InvalidArgument(format!(
                "{address:?} is not ip:port"
            )));
        }
        let msg = self.transport.host(&format!("connect:{address}")).await?;
        if msg.contains("connected to") {
            Ok(msg.trim().to_string())
        } else {
            Err(Error::setup(
                format!("adb connect {address}: {}", msg.trim()),
                "on Android 11+ open Developer options > Wireless debugging, pair once with \
                 `agentphone`/`adb pair <ip:port> <code>` using \"Pair device with pairing \
                 code\", then connect to the IP:port shown on that screen; on older phones run \
                 `adb tcpip 5555` over USB first. PC and phone must be on the same network",
            ))
        }
    }

    /// `adb disconnect ip:port`.
    pub async fn disconnect_wifi(&self, address: &str) -> Result<String> {
        self.transport.host(&format!("disconnect:{address}")).await
    }

    /// `adb pair ip:port code` (needs the adb binary).
    pub async fn pair(&self, address: &str, code: &str) -> Result<String> {
        match &self.server {
            Some(s) => s.pair(address, code).await,
            None => Err(Error::unsupported("pairing needs the adb server transport")),
        }
    }

    async fn ready_entry(&self, id: &str) -> Result<DeviceEntry> {
        let mut entries = self.entries().await?;
        if !entries.iter().any(|e| e.serial == id) && adb::is_host_port(id) {
            // A Wi-Fi address that is not connected yet.
            self.connect_wifi(id).await?;
            entries = self.entries().await?;
        }
        let e = entries
            .into_iter()
            .find(|e| e.serial == id)
            .ok_or_else(|| Error::DeviceNotFound(id.to_string()))?;
        check_state(&e)?;
        Ok(e)
    }
}

/// Setup error for a device that cannot be used yet.
fn check_state(e: &DeviceEntry) -> Result<()> {
    let s = &e.serial;
    match &e.state {
        DeviceState::Device => Ok(()),
        DeviceState::Unauthorized | DeviceState::Authorizing => Err(Error::setup(
            format!("Android device {s} has not authorized this PC"),
            UNAUTHORIZED_HINT,
        )),
        DeviceState::Offline => Err(Error::setup(
            format!("Android device {s} is offline"),
            "unlock the phone, replug the cable or run `adb reconnect`; for Wi-Fi run \
             `adb connect <ip:port>` again",
        )),
        DeviceState::NoPermissions => Err(Error::setup(
            format!("no USB permission for Android device {s}"),
            "install the Google USB driver (Windows) or fix udev rules (Linux), then replug",
        )),
        DeviceState::Other(st) => Err(Error::setup(
            format!("Android device {s} is in '{st}' mode"),
            "boot the phone normally and enable USB debugging in Developer options",
        )),
    }
}

/// Minimal info when the device could not be queried.
fn fallback_info(e: &DeviceEntry) -> DeviceInfo {
    let model = e.model.clone().unwrap_or_default().replace('_', " ");
    DeviceInfo {
        id: e.serial.clone(),
        name: if model.is_empty() {
            e.serial.clone()
        } else {
            model.clone()
        },
        platform: Platform::Android,
        os_version: String::new(),
        model,
        screen_points: Size::default(),
        screen_pixels: Size::default(),
        backend: "adb".into(),
    }
}

#[async_trait]
impl DeviceProvider for AndroidProvider {
    fn name(&self) -> &str {
        "android"
    }

    async fn discover(&self) -> Result<Vec<DeviceInfo>> {
        let entries = self.entries().await?;
        let mut found = Vec::new();
        let mut blocked = Vec::new();
        for e in &entries {
            if let Err(err) = check_state(e) {
                tracing::warn!(serial = %e.serial, "skipping Android device: {err}");
                blocked.push(err);
                continue;
            }
            match query_info(self.transport.as_ref(), &e.serial).await {
                Ok((info, _)) => found.push(info),
                Err(err) => {
                    tracing::warn!(serial = %e.serial, "cannot read device info: {err}");
                    found.push(fallback_info(e));
                }
            }
        }
        if found.is_empty() && !blocked.is_empty() {
            // Nothing usable: surface the first reason so the user can fix it.
            return Err(blocked.swap_remove(0));
        }
        Ok(found)
    }

    async fn connect(&self, id: &str) -> Result<Box<dyn Device>> {
        let e = self.ready_entry(id).await?;
        let dev = AndroidDevice::open(self.transport.clone(), &e.serial).await?;
        Ok(Box::new(dev))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::{FakeTransport, INFO_REPLY};

    fn provider(devices: &str) -> (Arc<FakeTransport>, AndroidProvider) {
        let t = Arc::new(FakeTransport::default());
        *t.host_reply.lock().unwrap() = devices.to_string();
        (t.clone(), AndroidProvider::with_transport(t))
    }

    #[tokio::test]
    async fn discover_skips_unauthorized() {
        let (t, p) = provider(
            "List of devices attached\nAAA unauthorized usb:1 transport_id:1\nBBB device model:Pixel_8 transport_id:2\n",
        );
        t.reply(INFO_REPLY);
        let d = p.discover().await.unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id, "BBB");
        assert_eq!(d[0].name, "Google Pixel 8");
    }

    #[tokio::test]
    async fn only_unauthorized_gives_hint() {
        let (_, p) = provider("AAA unauthorized transport_id:1\n");
        let e = p.discover().await.unwrap_err();
        assert!(e.hint().unwrap().contains("Allow USB debugging"));
        let e = match p.connect("AAA").await {
            Err(e) => e,
            Ok(_) => panic!("unauthorized device connected"),
        };
        assert!(e.hint().is_some());
        assert!(matches!(
            p.connect("ZZZ").await,
            Err(Error::DeviceNotFound(_))
        ));
        assert!(p.discover().await.is_err());
    }

    #[tokio::test]
    async fn empty_and_fallback() {
        let (_, p) = provider("List of devices attached\n\n");
        assert!(p.discover().await.unwrap().is_empty());
        // Device whose info query fails (empty reply) still shows up.
        let (_, p) = provider("CCC device model:Galaxy_S24 transport_id:3\n");
        let d = p.discover().await.unwrap();
        assert_eq!(d[0].name, "Galaxy S24");
    }

    #[tokio::test]
    async fn wifi_connect() {
        let (t, p) = provider("connected to 192.168.1.5:5555");
        assert!(p.connect_wifi("192.168.1.5").await.is_ok());
        assert_eq!(t.calls(), vec!["host:connect:192.168.1.5:5555"]);
        let (_, p) = provider("failed to connect to '10.0.0.9:5555': Connection refused");
        let e = p.connect_wifi("10.0.0.9:5555").await.unwrap_err();
        assert!(e.hint().unwrap().contains("Wireless debugging"));
        assert!(p.connect_wifi("bad host;x").await.is_err());
        assert!(
            p.pair("1.2.3.4:5", "123456")
                .await
                .unwrap_err()
                .is_unsupported()
        );
    }

    /// Live check against the real adb server (`cargo test -p ap-android -- --ignored`).
    #[tokio::test]
    #[ignore]
    async fn live_discover() {
        let p = AndroidProvider::new(AndroidConfig::default());
        match p.discover().await {
            Ok(list) => {
                for d in &list {
                    println!(
                        "{} {} Android {} {:?}",
                        d.id, d.name, d.os_version, d.screen_pixels
                    );
                }
            }
            Err(e) => println!("discover error: {e}"),
        }
    }
}
