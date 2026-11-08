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
//! On Unix the folder is mode 700 and the files are mode 600 from creation.
//! On Windows the files get the ACL of the folder: the default folder (under
//! `%APPDATA%`) is private to the user. A custom `pairing_dir` must be a
//! folder that only the user can read.

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

/// Save a record. It holds the Wi-Fi MAC and IP, so it is written like a
/// secret (see [`write_secret`]).
pub fn save_record(dir: &Path, rec: &DeviceRecord) -> Result<()> {
    let p = paths(dir, &rec.udid)?;
    let json = serde_json::to_vec_pretty(rec).map_err(|e| Error::Other(e.to_string()))?;
    write_secret(&p.record, &json)
}

/// Create the pairing folder. Unix: mode 700, also when it exists already.
fn private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    Ok(())
}

fn write_new(tmp: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(tmp, path)
}

/// Write a secret file atomically: a new temp file in the same folder
/// (Unix: mode 600 from creation), then a rename onto `path`. Every error is
/// returned.
pub fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    private_dir(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::InvalidArgument(format!("bad secret path {}", path.display())))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = parent.join(tmp_name);
    // A temp file left by a crash: remove it, so create_new can succeed.
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let r = write_new(&tmp, path, bytes);
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(Error::from)
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

    #[test]
    fn secrets_replace_and_stay_private() {
        let dir = std::env::temp_dir().join(format!("ap-ios-secret-{}", std::process::id()));
        let sub = dir.join("sub");
        let f = sub.join("dev.plist");
        write_secret(&f, b"one").unwrap();
        // A stale temp file from a crash does not block the next write.
        std::fs::write(sub.join("dev.plist.tmp"), b"junk").unwrap();
        write_secret(&f, b"two").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        assert!(!sub.join("dev.plist.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&f), 0o600);
            assert_eq!(mode(&sub), 0o700);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
