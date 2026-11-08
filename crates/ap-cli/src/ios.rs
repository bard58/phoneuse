//! All iPhone (ap-ios) wiring for the CLI, behind a few narrow functions.

use std::sync::Arc;

use ap_core::{Config, DeviceProvider, Result};
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
        "hid_services" => "Touch and keyboard input",
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

/// iPhone setup checks for `doctor` and the wizard, in setup order.
pub async fn checks(cfg: &Config, udid: Option<&str>) -> Vec<Check> {
    convert(setup::check(&cfg.ios, udid).await)
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

    #[test]
    fn every_ios_check_has_a_name() {
        for id in setup::CHECK_NAMES {
            assert_ne!(check_name(id), "iPhone check", "{id}");
        }
    }
}
