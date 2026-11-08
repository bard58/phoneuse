//! iOS version parsing and route selection.
//!
//! Only one software route is built: Route B (CoreDevice HID + DVT screenshot
//! over a CoreDeviceProxy tunnel). It needs iOS 17.4 or later.
//!
//! Connection, screenshots, buttons and keys work from iOS 17.4. Touch input
//! needs iOS 27 or later ([`IosVersion::touch_supported`]): iOS 17.4 to 26.x
//! refuse the screen stream with CoreDevice error 9021 ("Remote control
//! requires iOS 27.0 or later on this device").

use std::fmt;

use ap_core::{Capabilities, Error, IOS_TOUCH_MIN_MAJOR, Result};
use serde::{Deserialize, Serialize};

/// The hint for devices below iOS 17.4.
pub const UNSUPPORTED_IOS_HINT: &str =
    "iOS 17.4 or later is required. Update the iPhone in Settings > General > Software Update.";

/// A parsed `ProductVersion` such as `17.4.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IosVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl IosVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The first version with CoreDeviceProxy over lockdown (USB or Wi-Fi).
    pub const MIN_SUPPORTED: IosVersion = IosVersion::new(17, 4, 0);

    /// The first version with touch input over CoreDevice HID.
    pub const MIN_TOUCH: IosVersion = IosVersion::new(IOS_TOUCH_MIN_MAJOR, 0, 0);

    /// Parse `17`, `17.4` or `17.4.1`. Extra text after a number is ignored
    /// (`26.0b2` gives 26.0).
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.trim().split('.');
        let num = |p: Option<&str>| -> Option<u16> {
            let p = p?;
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        };
        let major = num(parts.next())?;
        let minor = num(parts.next()).unwrap_or(0);
        let patch = num(parts.next()).unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }

    /// CoreDeviceProxy (`com.apple.internal.devicecompute.CoreDeviceProxy`)
    /// exists from iOS 17.4.
    pub fn has_core_device_proxy(self) -> bool {
        self >= Self::MIN_SUPPORTED
    }

    /// Touch input (tap, swipe, pinch) through CoreDevice HID needs iOS 27.
    /// Below 27 the device refuses the screen stream (error 9021) and drops
    /// every touch event.
    pub fn touch_supported(self) -> bool {
        self >= Self::MIN_TOUCH
    }

    /// What Route B can do on this version. Below iOS 27: screen only
    /// (screenshots, buttons, keys, text, app launch), no tap or swipe.
    pub fn capabilities(self) -> Capabilities {
        let touch = self.touch_supported();
        Capabilities {
            tap: touch,
            swipe: touch,
            type_text: true,
            keys: true,
            buttons: true,
            launch_app: true,
            ui_tree: false,
            screenshot: true,
        }
    }

    /// Developer Mode exists from iOS 16.
    pub fn has_developer_mode(self) -> bool {
        self.major >= 16
    }

    /// iOS 17+ uses the personalized DDI (TSS-signed).
    pub fn uses_personalized_ddi(self) -> bool {
        self.major >= 17
    }

    /// The route that applies to this version.
    pub fn route(self) -> Route {
        if self.has_core_device_proxy() {
            Route::CoreDeviceHid
        } else {
            Route::Unsupported
        }
    }
}

impl fmt::Display for IosVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

/// How agentphone controls a device of a given iOS version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// iOS 17.4+: CoreDeviceProxy tunnel, CoreDevice HID input, DVT screenshot.
    CoreDeviceHid,
    /// iOS below 17.4: no software route.
    Unsupported,
}

impl Route {
    pub fn describe(self) -> &'static str {
        match self {
            Route::CoreDeviceHid => {
                "CoreDevice HID + DVT screenshot over a CoreDeviceProxy tunnel (iOS 17.4+, touch input from iOS 27)"
            }
            Route::Unsupported => "not supported (iOS 17.4 or later is required)",
        }
    }
}

/// One backend kind. Only [`BackendKind::Hid`] is built.
///
/// A WebDriverAgent backend can be added later: add a `Wda` variant here,
/// map `"wda"` to it in [`select_backends`], and implement
/// [`crate::backend::IosBackend`] for it. The public API does not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BackendKind {
    /// CoreDevice HID + DVT (Route B).
    Hid,
}

impl BackendKind {
    pub fn name(self) -> &'static str {
        match self {
            BackendKind::Hid => "hid",
        }
    }
}

/// Pick the backends to try, in order, from `ios.backend_order`.
///
/// Errors: [`Error::Setup`] for iOS below 17.4, or when no entry in
/// `backend_order` is a built backend. Unknown and not-built names (such as
/// `"wda"`) are skipped with a warning.
pub fn select_backends(version: IosVersion, backend_order: &[String]) -> Result<Vec<BackendKind>> {
    if !version.has_core_device_proxy() {
        return Err(Error::setup(
            format!("this iPhone runs iOS {version}, which agentphone cannot control"),
            UNSUPPORTED_IOS_HINT,
        ));
    }
    let mut out = Vec::new();
    for name in backend_order {
        let kind = match name.trim().to_ascii_lowercase().as_str() {
            "hid" | "coredevice" | "core_device" => Some(BackendKind::Hid),
            other => {
                tracing::warn!("ios.backend_order: backend '{other}' is not available, skipped");
                None
            }
        };
        match kind {
            Some(k) if !out.contains(&k) => out.push(k),
            _ => {}
        }
    }
    if out.is_empty() {
        return Err(Error::setup(
            "no usable iOS backend in ios.backend_order",
            "add \"hid\" to ios.backend_order in config.toml (the default is [\"hid\"])",
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_versions() {
        assert_eq!(IosVersion::parse("17.4.1"), Some(IosVersion::new(17, 4, 1)));
        assert_eq!(IosVersion::parse("26"), Some(IosVersion::new(26, 0, 0)));
        assert_eq!(IosVersion::parse("26.0b2"), Some(IosVersion::new(26, 0, 0)));
        assert_eq!(IosVersion::parse(""), None);
        assert_eq!(IosVersion::parse("x.1"), None);
        assert_eq!(IosVersion::new(17, 4, 0).to_string(), "17.4");
        assert_eq!(IosVersion::new(15, 8, 3).to_string(), "15.8.3");
    }

    #[test]
    fn routes_per_version() {
        let r = |s: &str| IosVersion::parse(s).unwrap().route();
        assert_eq!(r("15.8"), Route::Unsupported);
        assert_eq!(r("16.7.10"), Route::Unsupported);
        assert_eq!(r("17.0"), Route::Unsupported);
        assert_eq!(r("17.3.1"), Route::Unsupported);
        assert_eq!(r("17.4"), Route::CoreDeviceHid);
        assert_eq!(r("18.6"), Route::CoreDeviceHid);
        assert_eq!(r("26.1"), Route::CoreDeviceHid);
        assert!(!IosVersion::new(15, 0, 0).has_developer_mode());
        assert!(IosVersion::new(16, 0, 0).has_developer_mode());
        assert!(IosVersion::new(17, 0, 0).uses_personalized_ddi());
    }

    #[test]
    fn touch_gate_per_version() {
        let v = |s: &str| IosVersion::parse(s).unwrap();
        for old in ["17.4", "18.7.3", "26.3", "26.7.1"] {
            assert!(!v(old).touch_supported(), "{old}");
            let c = v(old).capabilities();
            assert!(!c.tap && !c.swipe && !c.has_touch(), "{old}");
            assert!(c.screenshot && c.buttons && c.keys && c.type_text, "{old}");
            // Connection and screenshots stay allowed below 27.
            assert_eq!(v(old).route(), Route::CoreDeviceHid);
            assert!(select_backends(v(old), &order(&["hid"])).is_ok());
        }
        for new in ["27.0", "27.0.1", "28.1"] {
            assert!(v(new).touch_supported(), "{new}");
            assert!(v(new).capabilities().has_touch(), "{new}");
        }
    }

    #[test]
    fn backend_selection() {
        let v = IosVersion::new(26, 0, 0);
        assert_eq!(
            select_backends(v, &order(&["hid", "wda"])).unwrap(),
            vec![BackendKind::Hid]
        );
        assert_eq!(
            select_backends(v, &order(&["wda", "HID", "hid"])).unwrap(),
            vec![BackendKind::Hid]
        );
        let e = select_backends(v, &order(&["wda"])).unwrap_err();
        assert!(e.hint().unwrap().contains("backend_order"));
        let e = select_backends(IosVersion::new(17, 3, 0), &order(&["hid"])).unwrap_err();
        assert_eq!(e.hint(), Some(UNSUPPORTED_IOS_HINT));
        assert!(UNSUPPORTED_IOS_HINT.starts_with("iOS 17.4 or later is required."));
    }
}
