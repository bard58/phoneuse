//! Saved pairing records for Wi-Fi use.
//!
//! Folder: `IosConfig::pairing_dir`, or `<data dir>/agentphone/ios-pairing`
//! (Windows: `%APPDATA%\agentphone\data\ios-pairing`). Per device:
//! * `<udid>.plist`: lockdown pairing file (from usbmuxd). Used for
//!   lockdown over TCP 62078 and CoreDeviceProxy over Wi-Fi.
//! * `<udid>.rppairing`: RemotePairing file (`RpPairingFile`), for the
//!   `_remotepairing._tcp` tunnel.
//! * `<udid>.json`: [`DeviceRecord`] (name, model, last IP).
//!
//! These files are secrets: anyone with them can control the paired iPhone.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use ap_core::{Error, IosConfig, Result};
use serde::{Deserialize, Serialize};

/// What agentphone remembers about a paired device.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceRecord {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub os_version: String,
    /// Wi-Fi MAC from the pairing file (matches `_apple-mobdev2._tcp`).
    pub wifi_mac: String,
    /// Last IP seen (usbmuxd network entry or mDNS).
    pub last_ip: Option<IpAddr>,
}

/// Paths for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePaths {
    pub lockdown: PathBuf,
    pub remote_pairing: PathBuf,
    pub record: PathBuf,
}

/// The pairing folder for this config.
pub fn pairing_dir(cfg: &IosConfig) -> Result<PathBuf> {
    if let Some(d) = &cfg.pairing_dir {
        return Ok(d.clone());
    }
    directories::ProjectDirs::from("", "", "agentphone")
        .map(|d| d.data_dir().join("ios-pairing"))
        .ok_or_else(|| Error::Config("cannot find the user's home directory".into()))
}

/// Cache folder (DDI downloads).
pub fn cache_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "", "agentphone")
        .map(|d| d.cache_dir().to_path_buf())
        .ok_or_else(|| Error::Config("cannot find the user's home directory".into()))
}

fn safe_name(udid: &str) -> Result<&str> {
    if udid.is_empty()
        || !udid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::InvalidArgument(format!("bad device id {udid:?}")));
    }
    Ok(udid)
}

pub fn paths(dir: &Path, udid: &str) -> Result<DevicePaths> {
    let n = safe_name(udid)?;
    Ok(DevicePaths {
        lockdown: dir.join(format!("{n}.plist")),
        remote_pairing: dir.join(format!("{n}.rppairing")),
        record: dir.join(format!("{n}.json")),
    })
}

/// Read every `<udid>.json` record in `dir`. A missing folder gives none.
pub fn list_records(dir: &Path) -> Vec<DeviceRecord> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<DeviceRecord> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| std::fs::read(&p).ok())
        .filter_map(|b| serde_json::from_slice::<DeviceRecord>(&b).ok())
        .filter(|r| safe_name(&r.udid).is_ok())
        .collect();
    out.sort_by(|a, b| a.udid.cmp(&b.udid));
    out
}

pub fn load_record(dir: &Path, udid: &str) -> Option<DeviceRecord> {
    let p = paths(dir, udid).ok()?;
    serde_json::from_slice(&std::fs::read(p.record).ok()?).ok()
}

pub fn save_record(dir: &Path, rec: &DeviceRecord) -> Result<()> {
    let p = paths(dir, &rec.udid)?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(rec).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(p.record, json)?;
    Ok(())
}

/// Write a secret file. Best effort to keep it private (Unix mode 600).
pub fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        let dir = std::env::temp_dir().join(format!("ap-ios-store-{}", std::process::id()));
        let rec = DeviceRecord {
            udid: "00008110-001A2B3C4D5E801E".into(),
            name: "Test iPhone".into(),
            product_type: "iPhone14,5".into(),
            os_version: "26.0".into(),
            wifi_mac: "aa:bb:cc:dd:ee:ff".into(),
            last_ip: Some("192.168.1.20".parse().unwrap()),
        };
        save_record(&dir, &rec).unwrap();
        assert_eq!(load_record(&dir, &rec.udid), Some(rec.clone()));
        assert_eq!(list_records(&dir), vec![rec]);
        assert!(paths(&dir, "../evil").is_err());
        assert!(list_records(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
