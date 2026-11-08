//! App name to bundle id / package name.

use ap_core::{Device, Error, Platform, Result};

const IOS: &[(&str, &str)] = &[
    ("settings", "com.apple.Preferences"),
    ("safari", "com.apple.mobilesafari"),
    ("messages", "com.apple.MobileSMS"),
    ("phone", "com.apple.mobilephone"),
    ("mail", "com.apple.mobilemail"),
    ("photos", "com.apple.mobileslideshow"),
    ("camera", "com.apple.camera"),
    ("maps", "com.apple.Maps"),
    ("calendar", "com.apple.mobilecal"),
    ("notes", "com.apple.mobilenotes"),
    ("reminders", "com.apple.reminders"),
    ("clock", "com.apple.mobiletimer"),
    ("appstore", "com.apple.AppStore"),
    ("music", "com.apple.Music"),
    ("weather", "com.apple.weather"),
    ("contacts", "com.apple.MobileAddressBook"),
    ("files", "com.apple.DocumentsApp"),
    ("facetime", "com.apple.facetime"),
    ("calculator", "com.apple.calculator"),
    ("health", "com.apple.Health"),
    ("wallet", "com.apple.Passbook"),
    ("shortcuts", "com.apple.shortcuts"),
    ("findmy", "com.apple.findmy"),
];

const ANDROID: &[(&str, &str)] = &[
    ("settings", "com.android.settings"),
    ("chrome", "com.android.chrome"),
    ("messages", "com.google.android.apps.messaging"),
    ("phone", "com.google.android.dialer"),
    ("gmail", "com.google.android.gm"),
    ("maps", "com.google.android.apps.maps"),
    ("photos", "com.google.android.apps.photos"),
    ("youtube", "com.google.android.youtube"),
    ("playstore", "com.android.vending"),
    ("calendar", "com.google.android.calendar"),
    ("contacts", "com.google.android.contacts"),
    ("clock", "com.google.android.deskclock"),
    ("calculator", "com.google.android.calculator"),
    ("camera", "com.android.camera2"),
    ("files", "com.google.android.documentsui"),
];

fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A bundle id / package name for `app`: ids pass through ("com.x.y"), known
/// names map through a table, other names match the last part of an installed id.
pub(crate) async fn resolve(device: &dyn Device, app: &str) -> Result<String> {
    let app = app.trim();
    if app.is_empty() {
        return Err(Error::InvalidArgument("app name is empty".into()));
    }
    if app.contains('.') && !app.contains(char::is_whitespace) {
        return Ok(app.to_string());
    }
    let want = squash(app);
    let table = match device.info().platform {
        Platform::Ios => IOS,
        Platform::Android => ANDROID,
    };
    if let Some((_, id)) = table.iter().find(|(n, _)| *n == want) {
        return Ok((*id).to_string());
    }
    let apps = device.list_apps().await.unwrap_or_default();
    let last = |id: &str| squash(id.rsplit('.').next().unwrap_or(id));
    if let Some(id) = apps.iter().find(|id| last(id) == want) {
        return Ok(id.clone());
    }
    let partial: Vec<&String> = apps.iter().filter(|id| last(id).contains(&want)).collect();
    if let [id] = partial.as_slice() {
        return Ok((*id).clone());
    }
    Err(Error::InvalidArgument(format!(
        "no app id known for \"{app}\": pass the iOS bundle id or Android package name (call list_apps to see installed ids)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ap_core::mock::MockDevice;

    #[tokio::test]
    async fn resolves_names() {
        let d = MockDevice::new();
        assert_eq!(
            resolve(&d, "Settings").await.unwrap(),
            "com.apple.Preferences"
        );
        assert_eq!(
            resolve(&d, "App Store").await.unwrap(),
            "com.apple.AppStore"
        );
        assert_eq!(resolve(&d, "com.x.y").await.unwrap(), "com.x.y");
        // From list_apps: last component.
        assert_eq!(
            resolve(&d, "mobilesafari").await.unwrap(),
            "com.apple.mobilesafari"
        );
        assert!(resolve(&d, "Nope").await.is_err());
        assert!(resolve(&d, " ").await.is_err());
    }
}
