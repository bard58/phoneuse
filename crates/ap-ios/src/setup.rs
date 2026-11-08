//! Setup checks and fix actions, for `agentphone doctor` and a setup wizard.
//!
//! [`check`] runs every check in order and never fails as a whole: each item
//! is a [`CheckResult`]. When a check fails, the checks that depend on it are
//! [`Status::Skip`]. The actions ([`pair`], [`enable_developer_mode_prompt`],
//! [`mount_ddi`], [`save_wifi_pairing`]) fix one item each. They need the
//! iPhone on a USB cable.
//!
//! Every function runs on an internal big-stack thread, so it is safe to call
//! from any async context. UNVERIFIED on a device: only the usbmuxd-missing
//! path ran (no iPhone was attached during development).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ap_core::{Error, IosConfig, Result};
use futures::StreamExt;
use idevice::{
    IdeviceError, IdeviceService,
    amfi::AmfiClient,
    lockdown::LockdownClient,
    mobile_image_mounter::ImageMounter,
    provider::IdeviceProvider,
    remote_pairing::{RemotePairingLockdownService, RpPairingFile},
    usbmuxd::{Connection, UsbmuxdDevice},
};
use serde::{Deserialize, Serialize};

use crate::connect::{self, LockdownInfo};
use crate::errors::{DDI_HINT, DEVMODE_HINT, PAIR_HINT, ctx};
use crate::models;
use crate::provider::pick_udid;
use crate::store::{self, DeviceRecord};
use crate::tunnel;
use crate::version::{IosVersion, Route, UNSUPPORTED_IOS_HINT};
use crate::worker::offload;

/// Result of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    /// Not run, because an earlier check failed.
    Skip,
}

/// One line of `agentphone doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Stable id: `usbmuxd`, `device`, `paired`, `ios_version`,
    /// `developer_mode`, `ddi`, `tunnel`, `hid_services`, `wifi_pairing`.
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    /// What the user (or a setup action) must do. `None` when OK.
    pub fix_hint: Option<String>,
    /// The [`Action`] that fixes this item, if there is one.
    pub fix_action: Option<Action>,
}

/// A fix that the setup wizard can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// [`pair`]
    Pair,
    /// [`enable_developer_mode_prompt`]
    RevealDeveloperMode,
    /// [`mount_ddi`]
    MountDdi,
    /// [`save_wifi_pairing`]
    SaveWifiPairing,
}

impl CheckResult {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Ok,
            detail: detail.into(),
            fix_hint: None,
            fix_action: None,
        }
    }

    fn bad(
        name: &'static str,
        status: Status,
        detail: impl Into<String>,
        hint: impl Into<String>,
        action: Option<Action>,
    ) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
            fix_hint: Some(hint.into()),
            fix_action: action,
        }
    }

    fn skip(name: &'static str, because: &str) -> Self {
        Self {
            name,
            status: Status::Skip,
            detail: format!("skipped: {because}"),
            fix_hint: None,
            fix_action: None,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.status == Status::Ok
    }
}

/// The problem text of an error, without the "fix:" part. The fix text goes
/// into [`CheckResult::fix_hint`] instead.
fn problem(e: &Error) -> String {
    match e {
        Error::Setup { problem, .. } => problem.clone(),
        other => other.to_string(),
    }
}

/// Every check name, in order.
pub const CHECK_NAMES: &[&str] = &[
    "usbmuxd",
    "device",
    "paired",
    "ios_version",
    "developer_mode",
    "ddi",
    "tunnel",
    "hid_services",
    "wifi_pairing",
];

fn skip_rest(out: &mut Vec<CheckResult>, because: &str) {
    for name in CHECK_NAMES.iter().skip(out.len()) {
        out.push(CheckResult::skip(name, because));
    }
}

/// Pick the USB device: `id` (UDID or unique prefix), or the only one.
async fn select_usb(id: Option<&str>) -> Result<UsbmuxdDevice> {
    let devs: Vec<UsbmuxdDevice> = connect::list_usbmuxd()
        .await?
        .into_iter()
        .filter(|d| d.connection_type == Connection::Usb)
        .collect();
    match id {
        Some(id) => {
            let udid =
                pick_udid(id, devs.iter().map(|d| d.udid.as_str())).map_err(|e| match e {
                    Error::DeviceNotFound(_) => Error::setup(
                        format!("iPhone {id} is not connected by USB"),
                        "connect the iPhone with a USB cable and unlock it",
                    ),
                    e => e,
                })?;
            devs.into_iter()
                .find(|d| d.udid == udid)
                .ok_or(Error::DeviceNotFound(udid))
        }
        None => match devs.len() {
            0 => Err(Error::setup(
                "no iPhone is connected by USB",
                "connect the iPhone with a USB cable, unlock it, and tap Trust",
            )),
            1 => Ok(devs
                .into_iter()
                .next()
                .ok_or(Error::DeviceNotFound(String::new()))?),
            n => Err(Error::InvalidArgument(format!(
                "{n} iPhones are connected; give a device id"
            ))),
        },
    }
}

async fn developer_mode(p: &dyn IdeviceProvider) -> Result<bool> {
    let mut amfi = Box::pin(AmfiClient::connect(p))
        .await
        .map_err(ctx("amfi"))?;
    amfi.get_developer_mode_status()
        .await
        .map_err(ctx("Developer Mode status"))
}

async fn ddi_mounted(p: &dyn IdeviceProvider, v: IosVersion) -> Result<bool> {
    let mut m = Box::pin(ImageMounter::connect(p))
        .await
        .map_err(ctx("image mounter"))?;
    let kind = if v.uses_personalized_ddi() {
        "Personalized"
    } else {
        "Developer"
    };
    match m.lookup_image(kind).await {
        Ok(sig) => Ok(!sig.is_empty()),
        Err(IdeviceError::NotFound) => Ok(false),
        Err(e) => Err(crate::errors::map("lookup DDI", e)),
    }
}

async fn run_checks(cfg: IosConfig, id: Option<String>) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // 1. usbmuxd
    match connect::list_usbmuxd().await {
        Ok(devs) => out.push(CheckResult::ok(
            "usbmuxd",
            format!("usbmuxd is running ({} device(s))", devs.len()),
        )),
        Err(e) => {
            out.push(CheckResult::bad(
                "usbmuxd",
                Status::Fail,
                problem(&e),
                connect::usbmuxd_hint(),
                None,
            ));
            skip_rest(&mut out, "usbmuxd is not reachable");
            return out;
        }
    }

    // 2. device
    let dev = match select_usb(id.as_deref()).await {
        Ok(d) => {
            out.push(CheckResult::ok("device", format!("USB device {}", d.udid)));
            d
        }
        Err(e) => {
            let hint = e
                .hint()
                .unwrap_or("connect the iPhone with a USB cable")
                .to_string();
            out.push(CheckResult::bad(
                "device",
                Status::Fail,
                problem(&e),
                hint,
                None,
            ));
            skip_rest(&mut out, "no USB device");
            return out;
        }
    };
    let provider = match connect::usbmuxd_provider(&dev) {
        Ok(p) => p,
        Err(e) => {
            out.push(CheckResult::bad(
                "paired",
                Status::Fail,
                problem(&e),
                PAIR_HINT,
                None,
            ));
            skip_rest(&mut out, "no provider");
            return out;
        }
    };

    // 3. paired / trusted
    let li: LockdownInfo = match Box::pin(connect::lockdown_info(&*provider)).await {
        Ok(li) if li.trusted => {
            out.push(CheckResult::ok(
                "paired",
                format!("{} trusts this computer", li.name),
            ));
            li
        }
        Ok(li) => {
            out.push(CheckResult::bad(
                "paired",
                Status::Fail,
                format!("{} does not trust this computer", li.name),
                PAIR_HINT,
                Some(Action::Pair),
            ));
            skip_rest(&mut out, "not paired");
            return out;
        }
        Err(e) => {
            out.push(CheckResult::bad(
                "paired",
                Status::Fail,
                problem(&e),
                PAIR_HINT,
                Some(Action::Pair),
            ));
            skip_rest(&mut out, "lockdown failed");
            return out;
        }
    };

    // 4. iOS version and route
    let model = models::marketing_name(&li.product_type);
    let Some(version) = IosVersion::parse(&li.product_version) else {
        out.push(CheckResult::bad(
            "ios_version",
            Status::Fail,
            format!("unknown iOS version {:?}", li.product_version),
            "update agentphone",
            None,
        ));
        skip_rest(&mut out, "unknown iOS version");
        return out;
    };
    let route = version.route();
    if route == Route::Unsupported {
        out.push(CheckResult::bad(
            "ios_version",
            Status::Fail,
            format!("{model}, iOS {version}: {}", route.describe()),
            UNSUPPORTED_IOS_HINT,
            None,
        ));
        skip_rest(&mut out, "iOS is older than 17.4");
        return out;
    }
    out.push(CheckResult::ok(
        "ios_version",
        format!("{model}, iOS {version}: {}", route.describe()),
    ));

    // 5. Developer Mode
    match developer_mode(&*provider).await {
        Ok(true) => out.push(CheckResult::ok("developer_mode", "Developer Mode is on")),
        Ok(false) => {
            out.push(CheckResult::bad(
                "developer_mode",
                Status::Fail,
                "Developer Mode is off",
                DEVMODE_HINT,
                Some(Action::RevealDeveloperMode),
            ));
            skip_rest(&mut out, "Developer Mode is off");
            return out;
        }
        Err(e) => {
            out.push(CheckResult::bad(
                "developer_mode",
                Status::Fail,
                problem(&e),
                DEVMODE_HINT,
                Some(Action::RevealDeveloperMode),
            ));
            skip_rest(&mut out, "Developer Mode unknown");
            return out;
        }
    }

    // 6. DDI
    match ddi_mounted(&*provider, version).await {
        Ok(true) => out.push(CheckResult::ok(
            "ddi",
            "personalized Developer Disk Image is mounted",
        )),
        Ok(false) => {
            out.push(CheckResult::bad(
                "ddi",
                Status::Fail,
                "the Developer Disk Image is not mounted (it unmounts at each restart)",
                DDI_HINT,
                Some(Action::MountDdi),
            ));
            skip_rest(&mut out, "DDI not mounted");
            return out;
        }
        Err(e) => {
            out.push(CheckResult::bad(
                "ddi",
                Status::Fail,
                problem(&e),
                DDI_HINT,
                Some(Action::MountDdi),
            ));
            skip_rest(&mut out, "DDI state unknown");
            return out;
        }
    }

    // 7. tunnel / RSD
    let t = match Box::pin(tunnel::open_core_device_proxy(&*provider)).await {
        Ok(t) => {
            out.push(CheckResult::ok(
                "tunnel",
                format!(
                    "CoreDeviceProxy tunnel and RSD are up ({} services)",
                    t.rsd.services.len()
                ),
            ));
            t
        }
        Err(e) => {
            out.push(CheckResult::bad(
                "tunnel",
                Status::Fail,
                problem(&e),
                e.hint()
                    .unwrap_or("unlock the iPhone, reconnect the cable, and try again"),
                None,
            ));
            skip_rest(&mut out, "no tunnel");
            return out;
        }
    };

    // 8. HID + displayservice
    let missing = t.missing();
    if missing.is_empty() {
        out.push(CheckResult::ok(
            "hid_services",
            "HID (indigo, universalhidservice), displayservice and dtservicehub are present",
        ));
    } else {
        out.push(CheckResult::bad(
            "hid_services",
            Status::Fail,
            format!("missing RSD services: {}", missing.join(", ")),
            format!("this iOS build or this DDI does not have the CoreDevice HID services; remount the DDI. If they stay missing, agentphone cannot drive iOS {version}"),
            Some(Action::MountDdi),
        ));
    }
    drop(t);

    // 9. Wi-Fi pairing
    let wifi = store::pairing_dir(&cfg)
        .and_then(|d| store::paths(&d, &dev.udid))
        .map(|p| (p.lockdown.exists(), p.remote_pairing.exists()));
    match wifi {
        Ok((true, rp)) => out.push(CheckResult::ok(
            "wifi_pairing",
            if rp {
                "Wi-Fi pairing saved (lockdown + RemotePairing)"
            } else {
                "Wi-Fi pairing saved (lockdown only)"
            },
        )),
        Ok((false, _)) => out.push(CheckResult::bad(
            "wifi_pairing",
            Status::Warn,
            "no Wi-Fi pairing saved; the iPhone works only on the cable",
            "save the Wi-Fi pairing while the cable is connected (optional)",
            Some(Action::SaveWifiPairing),
        )),
        Err(e) => out.push(CheckResult::bad(
            "wifi_pairing",
            Status::Warn,
            problem(&e),
            "set ios.pairing_dir in config.toml",
            None,
        )),
    }
    out
}

/// Run every check for `id` (UDID or prefix; `None` = the only USB device).
pub async fn check(cfg: &IosConfig, id: Option<&str>) -> Vec<CheckResult> {
    let cfg = cfg.clone();
    let id = id.map(str::to_string);
    match offload(move || async move { Ok(run_checks(cfg, id).await) }).await {
        Ok(v) => v,
        Err(e) => vec![CheckResult::bad(
            "usbmuxd",
            Status::Fail,
            problem(&e),
            "internal error: restart agentphone",
            None,
        )],
    }
}

/// Pair with the USB iPhone and save the record in usbmuxd. The iPhone shows
/// "Trust This Computer?". The user taps Trust and enters the passcode.
/// Waits up to 2 minutes. Returns the UDID. UNVERIFIED on a device.
pub async fn pair(id: Option<&str>) -> Result<String> {
    let id = id.map(str::to_string);
    offload(move || async move {
        let dev = select_usb(id.as_deref()).await?;
        let (_, mut mux) = connect::usbmuxd().await?;
        let buid = mux.get_buid().await.map_err(ctx("usbmuxd BUID"))?;
        let provider = connect::usbmuxd_provider(&dev)?;
        let mut lc = Box::pin(LockdownClient::connect(&*provider))
            .await
            .map_err(ctx("lockdown connect"))?;
        let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();
        let host = tunnel::host_name();
        let pf = tokio::time::timeout(
            Duration::from_secs(120),
            Box::pin(lc.pair(host_id, buid, Some(host.as_str()))),
        )
        .await
        .map_err(|_| {
            Error::setup(
                "the iPhone did not answer the Trust prompt in 2 minutes",
                "unlock the iPhone, tap Trust, enter the passcode, and run pair again",
            )
        })?
        .map_err(ctx("pair"))?;
        lc.start_session(&pf)
            .await
            .map_err(ctx("test the new pairing"))?;
        let mut pf = pf;
        pf.udid = Some(dev.udid.clone());
        let bytes = pf.serialize().map_err(ctx("serialize pairing"))?;
        mux.save_pair_record(&dev.udid, bytes)
            .await
            .map_err(ctx("usbmuxd SavePairRecord"))?;
        Ok(dev.udid)
    })
    .await
}

/// Text that tells the user how to finish Developer Mode.
pub const DEVELOPER_MODE_STEPS: &str = "On the iPhone: open Settings > Privacy & Security > Developer Mode and switch it on. The iPhone restarts. After the restart, unlock it and tap Turn On in the alert, then enter the passcode.";

/// Make the Developer Mode switch visible in Settings (iOS 16+) and return
/// the steps the user must do on the iPhone (switch on, restart, confirm).
/// agentphone cannot switch it on when a passcode is set. UNVERIFIED.
pub async fn enable_developer_mode_prompt(id: Option<&str>) -> Result<&'static str> {
    reveal_developer_mode(id).await?;
    Ok(DEVELOPER_MODE_STEPS)
}

/// Make the Developer Mode switch visible in Settings > Privacy & Security.
pub async fn reveal_developer_mode(id: Option<&str>) -> Result<()> {
    let id = id.map(str::to_string);
    offload(move || async move {
        let dev = select_usb(id.as_deref()).await?;
        let p = connect::usbmuxd_provider(&dev)?;
        let mut amfi = Box::pin(AmfiClient::connect(&*p))
            .await
            .map_err(ctx("amfi"))?;
        amfi.reveal_developer_mode_option_in_ui()
            .await
            .map_err(ctx("reveal Developer Mode"))
    })
    .await
}

/// Progress of [`mount_ddi`] and [`download_ddi`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Downloading `file` from the mirror.
    Download {
        file: String,
        done: u64,
        total: Option<u64>,
    },
    /// The image is in the cache.
    Cached {
        dir: PathBuf,
    },
    /// Uploading the image to the iPhone.
    Upload {
        done: u64,
        total: u64,
    },
    /// Asking Apple TSS / the iPhone to mount.
    Mounting,
    Done,
}

/// Progress callback type.
pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// Mirror of the Xcode personalized DDI (iOS 17+). Override with the env var
/// `AGENTPHONE_DDI_MIRROR` (a base URL that ends with `/`).
pub const DDI_MIRROR: &str = "https://raw.githubusercontent.com/doronz88/DeveloperDiskImage/main/PersonalizedImages/Xcode_iOS_DDI_Personalized/";

/// Files of the personalized DDI.
pub const DDI_FILES: &[&str] = &["BuildManifest.plist", "Image.dmg", "Image.dmg.trustcache"];

/// Cache folder of the personalized DDI.
pub fn ddi_cache_dir() -> Result<PathBuf> {
    Ok(store::cache_dir()?
        .join("ddi")
        .join("Xcode_iOS_DDI_Personalized"))
}

fn ddi_complete(dir: &Path) -> bool {
    DDI_FILES.iter().all(|f| {
        std::fs::metadata(dir.join(f))
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    })
}

fn mirror_url() -> String {
    std::env::var("AGENTPHONE_DDI_MIRROR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DDI_MIRROR.to_string())
}

async fn download_into(dir: &Path, base: &str, progress: &ProgressFn) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!("agentphone/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Other(format!("HTTP client: {e}")))?;
    tokio::fs::create_dir_all(dir).await?;
    for f in DDI_FILES {
        let target = dir.join(f);
        if std::fs::metadata(&target)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            continue;
        }
        let url = format!("{base}{f}");
        let resp = client
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| {
                Error::setup(
                    format!("cannot download {url}: {e}"),
                    "check the Internet connection, or set AGENTPHONE_DDI_MIRROR to another mirror",
                )
            })?;
        let total = resp.content_length();
        let part = dir.join(format!("{f}.part"));
        let mut file = tokio::fs::File::create(&part).await?;
        let mut done = 0u64;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::NotConnected(format!("download {f}: {e}")))?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            done += chunk.len() as u64;
            progress(Progress::Download {
                file: f.to_string(),
                done,
                total,
            });
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        tokio::fs::rename(&part, &target).await?;
    }
    Ok(())
}

/// Download the personalized DDI into the cache (once). The image is Apple
/// software: agentphone does not bundle it. Returns the folder.
pub async fn download_ddi(progress: ProgressFn) -> Result<PathBuf> {
    offload(move || async move {
        let dir = ddi_cache_dir()?;
        if !ddi_complete(&dir) {
            download_into(&dir, &mirror_url(), &progress).await?;
        }
        progress(Progress::Cached { dir: dir.clone() });
        Ok(dir)
    })
    .await
}

/// Mount the personalized DDI (iOS 17+) on the USB iPhone. Downloads the
/// image first when it is not in the cache. `image_dir` overrides the folder
/// (it must hold [`DDI_FILES`]). The iPhone must be unlocked. The first mount
/// after each restart needs Internet (Apple TSS signs it). UNVERIFIED.
pub async fn mount_ddi(
    id: Option<&str>,
    image_dir: Option<PathBuf>,
    progress: ProgressFn,
) -> Result<()> {
    let id = id.map(str::to_string);
    offload(move || async move {
        let dev = select_usb(id.as_deref()).await?;
        let p = connect::usbmuxd_provider(&dev)?;
        let li = Box::pin(connect::lockdown_info(&*p)).await?;
        if !li.trusted {
            return Err(Error::setup(
                "this computer is not trusted by the iPhone",
                PAIR_HINT,
            ));
        }
        let v = IosVersion::parse(&li.product_version)
            .ok_or_else(|| Error::protocol("bad ProductVersion"))?;
        if !v.has_core_device_proxy() {
            return Err(Error::setup(
                format!("iOS {v} is not supported"),
                UNSUPPORTED_IOS_HINT,
            ));
        }
        if ddi_mounted(&*p, v).await? {
            progress(Progress::Done);
            return Ok(());
        }
        let dir = match image_dir {
            Some(d) => d,
            None => {
                let d = ddi_cache_dir()?;
                if !ddi_complete(&d) {
                    download_into(&d, &mirror_url(), &progress).await?;
                }
                d
            }
        };
        if !ddi_complete(&dir) {
            return Err(Error::setup(
                format!("{} does not hold the DDI files", dir.display()),
                format!("put {} in that folder", DDI_FILES.join(", ")),
            ));
        }
        let image = tokio::fs::read(dir.join("Image.dmg")).await?;
        let trust_cache = tokio::fs::read(dir.join("Image.dmg.trustcache")).await?;
        let manifest = tokio::fs::read(dir.join("BuildManifest.plist")).await?;
        let ecid = li
            .unique_chip_id
            .ok_or_else(|| Error::protocol("lockdown gave no UniqueChipID"))?;
        let mut m = Box::pin(ImageMounter::connect(&*p))
            .await
            .map_err(ctx("image mounter"))?;
        progress(Progress::Mounting);
        let cb = |((done, total), pf): ((usize, usize), ProgressFn)| async move {
            pf(Progress::Upload {
                done: done as u64,
                total: total as u64,
            });
        };
        Box::pin(m.mount_personalized_with_callback(
            &*p,
            image,
            trust_cache,
            &manifest,
            None,
            ecid,
            cb,
            progress.clone(),
        ))
        .await
        .map_err(|e| match e {
            IdeviceError::DeviceLocked => {
                Error::setup("the iPhone is locked", "unlock the iPhone and mount again")
            }
            IdeviceError::DeveloperModeNotEnabled => {
                Error::setup("Developer Mode is off", DEVMODE_HINT)
            }
            e => crate::errors::map("mount DDI (TSS needs Internet)", e),
        })?;
        progress(Progress::Done);
        Ok(())
    })
    .await
}

/// What [`save_wifi_pairing`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiPairing {
    pub udid: String,
    pub dir: PathBuf,
    /// Lockdown pairing file saved (always true on success).
    pub lockdown: bool,
    /// RemotePairing file saved. False when the iPhone refused it; the
    /// lockdown pairing still works over Wi-Fi.
    pub remote_pairing: bool,
    /// Why RemotePairing failed, if it did.
    pub remote_pairing_error: Option<String>,
}

/// Save the pairing for Wi-Fi use (needs the USB cable once):
/// 1. the lockdown pairing record from usbmuxd, with Wi-Fi connections
///    turned on (`EnableWifiConnections`);
/// 2. a RemotePairing file made over `com.apple.dt.remotepairingdeviced.lockdown`
///    (no prompt on the iPhone).
///
/// UNVERIFIED on a device.
pub async fn save_wifi_pairing(cfg: &IosConfig, id: Option<&str>) -> Result<WifiPairing> {
    let cfg = cfg.clone();
    let id = id.map(str::to_string);
    offload(move || async move {
        let dir = store::pairing_dir(&cfg)?;
        let dev = select_usb(id.as_deref()).await?;
        let paths = store::paths(&dir, &dev.udid)?;
        let (_, mut mux) = connect::usbmuxd().await?;
        let pf = mux
            .get_pair_record(&dev.udid)
            .await
            .map_err(|e| Error::setup(format!("no pair record in usbmuxd ({e})"), PAIR_HINT))?;
        let wifi_mac = pf.wifi_mac_address.clone();
        let bytes = pf.serialize().map_err(ctx("serialize pairing"))?;
        store::write_secret(&paths.lockdown, &bytes)?;

        let p = connect::usbmuxd_provider(&dev)?;
        let li = Box::pin(connect::lockdown_info(&*p)).await?;
        match connect::lockdown_session(&*p).await {
            Ok(mut lc) => {
                if let Err(e) = lc
                    .set_value(
                        "EnableWifiConnections",
                        true.into(),
                        Some("com.apple.mobile.wireless_lockdown"),
                    )
                    .await
                {
                    tracing::warn!("EnableWifiConnections: {e}");
                }
            }
            Err(e) => tracing::warn!("lockdown session: {e}"),
        }

        let host = tunnel::host_name();
        let rp: Result<()> = async {
            let svc = Box::pin(RemotePairingLockdownService::connect(&*p))
                .await
                .map_err(ctx("remotepairingdeviced"))?;
            let mut client = svc
                .into_client(&host)
                .map_err(ctx("RemotePairing client"))?;
            let mut rpf = RpPairingFile::generate(&host);
            // Over the trusted USB link there is no PIN prompt.
            Box::pin(client.connect(&mut rpf, async || String::new()))
                .await
                .map_err(ctx("RemotePairing pair"))?;
            store::write_secret(&paths.remote_pairing, &rpf.to_bytes())?;
            Ok(())
        }
        .await;

        let rec = DeviceRecord {
            udid: dev.udid.clone(),
            name: li.name.clone(),
            product_type: li.product_type.clone(),
            os_version: li.product_version.clone(),
            wifi_mac,
            last_ip: match dev.connection_type {
                Connection::Network(ip) => Some(ip),
                _ => store::load_record(&dir, &dev.udid).and_then(|r| r.last_ip),
            },
        };
        store::save_record(&dir, &rec)?;
        Ok(WifiPairing {
            udid: dev.udid,
            dir,
            lockdown: true,
            remote_pairing: rp.is_ok(),
            remote_pairing_error: rp.err().map(|e| e.to_string()),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problem_drops_fix_text() {
        let e = Error::setup("no tunnel", "run setup");
        assert_eq!(problem(&e), "no tunnel");
        assert!(!problem(&Error::Other("x".into())).contains("fix:"));
    }

    #[test]
    fn skip_fills_all_names() {
        let mut v = vec![CheckResult::ok("usbmuxd", "x")];
        skip_rest(&mut v, "test");
        assert_eq!(v.len(), CHECK_NAMES.len());
        assert!(v.iter().skip(1).all(|c| c.status == Status::Skip));
        let names: Vec<_> = v.iter().map(|c| c.name).collect();
        assert_eq!(names, CHECK_NAMES);
    }

    #[test]
    fn ddi_cache_detection() {
        let dir = std::env::temp_dir().join(format!("ap-ios-ddi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!ddi_complete(&dir));
        for f in DDI_FILES {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        assert!(ddi_complete(&dir));
        let _ = std::fs::remove_dir_all(dir);
        assert!(DDI_MIRROR.ends_with('/'));
    }

    #[tokio::test]
    async fn download_from_local_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for _ in 0..DDI_FILES.len() {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 2048];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = format!("data for {path}");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        let dir = std::env::temp_dir().join(format!("ap-ios-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let pf: ProgressFn = Arc::new(move |p| s2.lock().unwrap().push(p));
        download_into(&dir, &format!("http://{addr}/ddi/"), &pf)
            .await
            .unwrap();
        assert!(ddi_complete(&dir));
        let img = std::fs::read_to_string(dir.join("Image.dmg")).unwrap();
        assert_eq!(img, "data for /ddi/Image.dmg");
        assert!(seen.lock().unwrap().iter().any(|p| matches!(
            p,
            Progress::Download { file, .. } if file == "Image.dmg"
        )));
        let _ = std::fs::remove_dir_all(dir);
    }
}
