//! All iPhone (ap-ios) wiring for the CLI, behind a few narrow functions.

use std::sync::Arc;

use ap_core::{Config, DeviceInfo, DeviceProvider, Platform, Result};
use ap_ios::setup::{self, Progress, Status as IosStatus};

use crate::doctor::{Check, Status};

/// The iPhone device provider.
pub fn provider(cfg: &Config) -> Box<dyn DeviceProvider> {
    Box::new(ap_ios::IosProvider::new(cfg.ios.clone()))
}

/// Human name of an ap-ios check id.
pub fn check_name(id: &str) -> &'static str {
    match id {
        "usbmuxd" => "Apple Mobile Device service",
        "device" => "iPhone connected",
        "paired" => "iPhone trusts this computer",
        "ios_version" => "iOS version",
        "developer_mode" => "Developer Mode",
        "ddi" => "Developer disk image",
        "tunnel" => "Secure connection",
        "hid_services" => "Input services (touch, buttons, keys)",
        "wifi_pairing" => "Wi-Fi pairing",
        _ => "iPhone check",
    }
}

/// Convert ap-ios check results. Skipped checks (an earlier one failed) are left out.
pub fn convert(results: Vec<setup::CheckResult>) -> Vec<Check> {
    results
        .into_iter()
        .filter_map(|c| {
            let status = match c.status {
                IosStatus::Ok => Status::Ok,
                IosStatus::Warn => Status::Warn,
                IosStatus::Fail => Status::Fail,
                IosStatus::Skip => return None,
            };
            Some(Check {
                group: "iPhone",
                id: c.name.to_string(),
                name: check_name(c.name).to_string(),
                status,
                detail: c.detail,
                fix: c.fix_hint,
            })
        })
        .collect()
}

/// The iPhone the checks and setup actions use: `--device`, else the saved
/// default phone when discovery lists it as an iPhone (a saved Android
/// default is not an iPhone id), else none (the only USB iPhone).
pub fn target(cfg: &Config, wanted: Option<&str>, found: &[DeviceInfo]) -> Option<String> {
    if let Some(w) = wanted {
        return Some(w.to_string());
    }
    let d = cfg.default_device.as_deref()?;
    found
        .iter()
        .any(|x| x.platform == Platform::Ios && x.id == d)
        .then(|| d.to_string())
}

/// iPhone setup checks for `doctor` and the wizard, in setup order.
/// `found` is the device discovery result (for Wi-Fi iPhones and the
/// several-iPhones case).
pub async fn checks(cfg: &Config, udid: Option<&str>, found: &[DeviceInfo]) -> Vec<Check> {
    let mut v = convert(setup::check(&cfg.ios, udid).await);
    adjust(&mut v, udid, found);
    v
}

/// Fix the "iPhone connected" check with what discovery knows:
/// - several iPhones and none chosen: say how to choose one;
/// - no USB iPhone, but the iPhone works over Wi-Fi: a warning, not a failure.
pub fn adjust(checks: &mut [Check], udid: Option<&str>, found: &[DeviceInfo]) {
    let Some(c) = checks
        .iter_mut()
        .find(|c| c.id == "device" && c.status == Status::Fail)
    else {
        return;
    };
    let iphones: Vec<&DeviceInfo> = found
        .iter()
        .filter(|d| d.platform == Platform::Ios)
        .filter(|d| udid.is_none_or(|u| d.id == u || d.id.starts_with(u)))
        .collect();
    if udid.is_none() && iphones.len() > 1 {
        let ids: Vec<&str> = iphones.iter().map(|d| d.id.as_str()).collect();
        c.fix = Some(format!(
            "choose one with `agentphone use <id>` or pass `--device <id>` (ids: {})",
            ids.join(", ")
        ));
    } else if let Some(d) = iphones.first() {
        c.status = Status::Warn;
        c.detail = format!(
            "{} works over Wi-Fi, but it is not connected by USB, so the USB checks were skipped",
            d.name
        );
        c.fix = Some(
            "connect the USB cable to run all checks (needed again after each iPhone restart)"
                .into(),
        );
    }
}

/// Pair with the iPhone (the phone shows "Trust This Computer?").
/// Returns a short message.
pub async fn pair(udid: Option<&str>) -> Result<String> {
    setup::pair(udid).await
}

/// Make the Developer Mode switch visible in Settings. Returns the steps
/// the user must then do on the iPhone.
pub async fn reveal_developer_mode(udid: Option<&str>) -> Result<&'static str> {
    setup::enable_developer_mode_prompt(udid).await
}

/// Progress of [`mount_ddi`]: (phase text, bytes done, bytes total).
pub type DdiProgress = Arc<dyn Fn(&str, u64, Option<u64>) + Send + Sync>;

/// Download (if needed) and mount the Developer Disk Image.
pub async fn mount_ddi(udid: Option<&str>, progress: DdiProgress) -> Result<()> {
    let cb: setup::ProgressFn = Arc::new(move |p: Progress| match p {
        Progress::Download { file, done, total } => {
            progress(&format!("downloading {file}"), done, total)
        }
        Progress::Upload { done, total } => progress("copying to the iPhone", done, Some(total)),
        Progress::Mounting => progress("mounting", 0, None),
        Progress::Cached { .. } | Progress::Done => {}
    });
    setup::mount_ddi(udid, None, cb).await
}

/// Save the pairing so the iPhone also works over Wi-Fi. Returns a short message.
pub async fn save_wifi_pairing(cfg: &Config, udid: Option<&str>) -> Result<String> {
    let w = setup::save_wifi_pairing(&cfg.ios, udid).await?;
    let mut msg = format!("saved Wi-Fi pairing for {} in {}", w.udid, w.dir.display());
    if !w.remote_pairing {
        msg.push_str(" (the iPhone refused the newer pairing; the older one still works)");
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, platform: Platform) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: format!("Phone {id}"),
            platform,
            os_version: "18.0".into(),
            model: "iPhone14,5".into(),
            screen_points: Default::default(),
            screen_pixels: Default::default(),
            backend: "hid".into(),
        }
    }

    fn usb_fail(detail: &str) -> Vec<Check> {
        vec![
            Check::new(
                "iPhone",
                "usbmuxd",
                "Apple Mobile Device service",
                Status::Ok,
                "",
            ),
            Check::new("iPhone", "device", "iPhone connected", Status::Fail, detail)
                .with_fix("connect the iPhone with a USB cable"),
        ]
    }

    #[test]
    fn target_uses_saved_iphone_only() {
        let found = [
            info("IPHONE1", Platform::Ios),
            info("R5CX", Platform::Android),
        ];
        let mut cfg = Config::default();
        assert_eq!(target(&cfg, None, &found), None);
        assert_eq!(target(&cfg, Some("X"), &found).as_deref(), Some("X"));
        cfg.default_device = Some("IPHONE1".into());
        assert_eq!(target(&cfg, None, &found).as_deref(), Some("IPHONE1"));
        cfg.default_device = Some("R5CX".into());
        assert_eq!(target(&cfg, None, &found), None);
    }

    #[test]
    fn several_iphones_say_how_to_choose() {
        let found = [info("A", Platform::Ios), info("B", Platform::Ios)];
        let mut c = usb_fail("2 iPhones are connected; give a device id");
        adjust(&mut c, None, &found);
        assert_eq!(c[1].status, Status::Fail);
        let fix = c[1].fix.as_deref().unwrap();
        assert!(
            fix.contains("agentphone use") && fix.contains("A, B"),
            "{fix}"
        );
    }

    #[test]
    fn wifi_iphone_is_a_warning() {
        let found = [info("A", Platform::Ios)];
        let mut c = usb_fail("no iPhone is connected by USB");
        adjust(&mut c, None, &found);
        assert_eq!(c[1].status, Status::Warn);
        assert!(c[1].detail.contains("Wi-Fi"));
        // Another iPhone over Wi-Fi does not hide a missing chosen iPhone.
        let mut c = usb_fail("iPhone Z is not connected by USB");
        adjust(&mut c, Some("Z"), &found);
        assert_eq!(c[1].status, Status::Fail);
        // No iPhone at all: still a failure.
        let mut c = usb_fail("no iPhone is connected by USB");
        adjust(&mut c, None, &[info("R5CX", Platform::Android)]);
        assert_eq!(c[1].status, Status::Fail);
    }

    #[test]
    fn every_ios_check_has_a_name() {
        for id in setup::CHECK_NAMES {
            assert_ne!(check_name(id), "iPhone check", "{id}");
        }
    }
}
