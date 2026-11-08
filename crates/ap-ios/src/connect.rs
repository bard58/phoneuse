//! usbmuxd access, lockdown values, and device providers.
//!
//! Transports in idevice 0.1.68:
//! * usbmuxd: USB, plus "Wi-Fi sync" entries. Windows: the Apple Mobile
//!   Device Service on TCP 127.0.0.1:27015. macOS/Linux: `/var/run/usbmuxd`.
//!   `USBMUXD_SOCKET_ADDRESS=ip:port` overrides the address.
//! * Direct TCP to lockdownd (port 62078) with a saved lockdown pairing file
//!   (`TcpProvider`). This is the Wi-Fi path.

use std::net::IpAddr;

use ap_core::{Error, Result};
use idevice::{
    IdeviceService,
    lockdown::LockdownClient,
    pairing_file::PairingFile,
    provider::{IdeviceProvider, TcpProvider},
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};

use crate::errors::ctx;

/// Label idevice sends to lockdownd.
pub const LABEL: &str = "agentphone";

/// Fix hint when usbmuxd is not reachable, for this platform.
pub fn usbmuxd_hint() -> &'static str {
    if cfg!(windows) {
        "Install the Apple Devices app from the Microsoft Store (or iTunes), then connect the iPhone and tap Trust. The Apple Mobile Device Service must run (it listens on 127.0.0.1:27015)."
    } else if cfg!(target_os = "macos") {
        "usbmuxd is part of macOS: connect the iPhone with a cable and tap Trust. If it still fails, restart the Mac."
    } else {
        "Install and start usbmuxd (for example `sudo apt install usbmuxd` or `sudo pacman -S usbmuxd`), then connect the iPhone and tap Trust."
    }
}

/// Address of usbmuxd for this platform (env override honored).
pub fn usbmuxd_addr() -> Result<UsbmuxdAddr> {
    UsbmuxdAddr::from_env_var().map_err(|e| {
        Error::setup(
            format!("USBMUXD_SOCKET_ADDRESS is not valid: {e}"),
            "unset USBMUXD_SOCKET_ADDRESS or set it to ip:port",
        )
    })
}

/// Open a usbmuxd connection, or a setup error with the install hint.
pub async fn usbmuxd() -> Result<(UsbmuxdAddr, UsbmuxdConnection)> {
    let addr = usbmuxd_addr()?;
    let socket = addr
        .to_socket()
        .await
        .map_err(|e| Error::setup(format!("usbmuxd is not reachable ({e})"), usbmuxd_hint()))?;
    Ok((addr, UsbmuxdConnection::new(socket, 1)))
}

/// Devices that usbmuxd knows, USB entries first.
pub async fn list_usbmuxd() -> Result<Vec<UsbmuxdDevice>> {
    let (_, mut mux) = usbmuxd().await?;
    let mut devs = mux
        .get_devices()
        .await
        .map_err(ctx("usbmuxd ListDevices"))?;
    devs.sort_by_key(|d| d.connection_type != Connection::Usb);
    Ok(devs)
}

/// Provider for a usbmuxd device.
pub fn usbmuxd_provider(dev: &UsbmuxdDevice) -> Result<Box<dyn IdeviceProvider>> {
    Ok(Box::new(dev.to_provider(usbmuxd_addr()?, LABEL)))
}

/// Provider for the direct TCP (Wi-Fi) path.
pub fn tcp_provider(
    addr: IpAddr,
    scope_id: Option<u32>,
    pairing_file: PairingFile,
) -> Box<dyn IdeviceProvider> {
    Box::new(TcpProvider {
        addr,
        scope_id,
        pairing_file,
        label: LABEL.to_string(),
    })
}

/// Lockdown values agentphone uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockdownInfo {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub product_version: String,
    pub build_version: String,
    pub unique_chip_id: Option<u64>,
    /// False when the session could not start (not paired / not trusted).
    pub trusted: bool,
}

/// A lockdown client with a session (needs a valid pairing).
pub async fn lockdown_session(provider: &dyn IdeviceProvider) -> Result<LockdownClient> {
    let mut lc = LockdownClient::connect(provider)
        .await
        .map_err(ctx("lockdown connect"))?;
    let pf = provider
        .get_pairing_file()
        .await
        .map_err(ctx("pairing record"))?;
    lc.start_session(&pf)
        .await
        .map_err(ctx("lockdown session"))?;
    Ok(lc)
}

async fn get_string(lc: &mut LockdownClient, key: &str) -> Option<String> {
    lc.get_value(Some(key), None)
        .await
        .ok()
        .and_then(|v| v.as_string().map(str::to_string))
}

/// Read device values. Tries a session first; without one, reads the values
/// lockdown gives to anyone (name, type, version) and sets `trusted = false`.
pub async fn lockdown_info(provider: &dyn IdeviceProvider) -> Result<LockdownInfo> {
    let (mut lc, trusted) = match lockdown_session(provider).await {
        Ok(lc) => (lc, true),
        Err(e) => {
            tracing::debug!("lockdown session failed: {e}");
            let lc = LockdownClient::connect(provider)
                .await
                .map_err(ctx("lockdown connect"))?;
            (lc, false)
        }
    };
    let mut info = LockdownInfo {
        trusted,
        ..LockdownInfo::default()
    };
    info.udid = get_string(&mut lc, "UniqueDeviceID")
        .await
        .unwrap_or_default();
    info.name = get_string(&mut lc, "DeviceName").await.unwrap_or_default();
    info.product_type = get_string(&mut lc, "ProductType").await.unwrap_or_default();
    info.product_version = get_string(&mut lc, "ProductVersion")
        .await
        .unwrap_or_default();
    info.build_version = get_string(&mut lc, "BuildVersion")
        .await
        .unwrap_or_default();
    if trusted {
        info.unique_chip_id = lc
            .get_value(Some("UniqueChipID"), None)
            .await
            .ok()
            .and_then(|v| v.as_unsigned_integer());
    }
    if info.product_version.is_empty() {
        return Err(Error::protocol("lockdown gave no ProductVersion"));
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hint_mentions_fix() {
        let h = super::usbmuxd_hint();
        if cfg!(windows) {
            assert!(
                h.contains("Install the Apple Devices app from the Microsoft Store (or iTunes)")
            );
        }
        assert!(!h.is_empty());
    }
}
