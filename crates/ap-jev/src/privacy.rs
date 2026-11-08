//! Privacy guard for Jev calls.
//!
//! Every [`ScreenJudge`](crate::ScreenJudge) call sends screen content to
//! TypeSafe. The guard refuses the call when:
//! - the user did not give consent for this run, or
//! - the foreground app matches the blocklist (banking, messages, password
//!   managers, Photos, Health, Wallet, ...), or
//! - the foreground app is unknown and `block_unknown_app` is set.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ap_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// Default blocklist. Entries match app ids or names, case-insensitively,
/// as substrings ("bank" matches "com.example.MyBank").
pub const DEFAULT_BLOCKLIST: &[&str] = &[
    // Banking and payments
    "bank",
    "paypal",
    "venmo",
    "revolut",
    "wise",
    "cash app",
    "com.squareup.cash",
    "coinbase",
    "binance",
    // Android bank and payment package ids that contain no word above.
    "com.chase.sig.android",
    "com.wf.wellsfargomobile",
    "com.infonow.bofa",
    "com.citi.citimobile",
    "com.konylabs.capitalone",
    "com.google.android.apps.nbu.paisa.user",
    // Wallet ("wallet" also matches com.google.android.apps.walletnfcrel)
    "wallet",
    "com.apple.passbook",
    // Messages and mail
    "messages",
    "messaging",
    "com.apple.mobilesms",
    "com.google.android.apps.messaging",
    "com.samsung.android.messaging",
    "com.android.mms",
    "whatsapp",
    "signal",
    "org.thoughtcrime.securesms",
    "telegram",
    "messenger",
    "com.facebook.orca",
    "com.apple.mobilemail",
    "gmail",
    "com.google.android.gm",
    "com.microsoft.office.outlook",
    // Password managers and authenticators
    "password",
    "1password",
    "bitwarden",
    "lastpass",
    "dashlane",
    "keepass",
    "keychain",
    "authenticator",
    // Photos
    "photos",
    "com.apple.mobileslideshow",
    "gallery",
    // Health
    "health",
    "com.apple.health",
];

/// Privacy settings, editable through the agentphone config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    /// App ids or names that never go to Jev.
    pub blocklist: Vec<String>,
    /// Entries removed from the blocklist (for example "gmail").
    pub allow: Vec<String>,
    /// Refuse when the foreground app is not known. On by default: a screen
    /// from an unknown app is never sent.
    pub block_unknown_app: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            blocklist: DEFAULT_BLOCKLIST.iter().map(|s| s.to_string()).collect(),
            allow: Vec::new(),
            block_unknown_app: true,
        }
    }
}

/// Run-time privacy guard: config, a consent flag and the current app.
#[derive(Debug, Default)]
pub struct Privacy {
    config: PrivacyConfig,
    consent: AtomicBool,
    foreground_app: RwLock<Option<String>>,
}

impl Privacy {
    /// A guard without consent. Call [`Privacy::grant_consent`] after the
    /// user agrees for this run.
    pub fn new(config: PrivacyConfig) -> Self {
        Self {
            config,
            consent: AtomicBool::new(false),
            foreground_app: RwLock::new(None),
        }
    }

    pub fn config(&self) -> &PrivacyConfig {
        &self.config
    }

    /// Record that the user agrees to send screens to TypeSafe for this run.
    pub fn grant_consent(&self) {
        self.consent.store(true, Ordering::SeqCst);
    }

    pub fn revoke_consent(&self) {
        self.consent.store(false, Ordering::SeqCst);
    }

    pub fn has_consent(&self) -> bool {
        self.consent.load(Ordering::SeqCst)
    }

    /// Set the app on screen (bundle id, package name or display name).
    pub fn set_foreground_app(&self, app: Option<String>) {
        if let Ok(mut g) = self.foreground_app.write() {
            *g = app;
        }
    }

    pub fn foreground_app(&self) -> Option<String> {
        self.foreground_app.read().ok().and_then(|g| g.clone())
    }

    /// The blocklist entry that `app` matches, if any.
    pub fn blocked_by(&self, app: &str) -> Option<&str> {
        let app = app.to_lowercase();
        self.config
            .blocklist
            .iter()
            .filter(|e| !e.trim().is_empty())
            .filter(|e| !self.config.allow.iter().any(|a| a.eq_ignore_ascii_case(e)))
            .find(|e| app.contains(&e.to_lowercase()))
            .map(String::as_str)
    }

    /// Check consent and the current foreground app.
    pub fn check(&self) -> Result<()> {
        self.check_app(self.foreground_app().as_deref())
    }

    /// Check consent and the given app.
    pub fn check_app(&self, app: Option<&str>) -> Result<()> {
        if !self.has_consent() {
            return Err(Error::Refused(
                "Jev sends screenshots to TypeSafe and the user has not given consent for this run"
                    .into(),
            ));
        }
        match app {
            Some(a) => match self.blocked_by(a) {
                Some(entry) => Err(Error::Refused(format!(
                    "app '{a}' matches the Jev privacy blocklist entry '{entry}'"
                ))),
                None => Ok(()),
            },
            None if self.config.block_unknown_app => Err(Error::Refused(
                "the foreground app is unknown and block_unknown_app is set".into(),
            )),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(r: Result<()>) -> bool {
        matches!(r, Err(Error::Refused(_)))
    }

    #[test]
    fn unknown_app_blocked_by_default() {
        let p = Privacy::new(PrivacyConfig::default());
        p.grant_consent();
        assert!(PrivacyConfig::default().block_unknown_app);
        assert!(refused(p.check()));
        p.set_foreground_app(Some("com.apple.Preferences".into()));
        assert!(p.check().is_ok());
    }

    #[test]
    fn needs_consent() {
        let p = Privacy::new(PrivacyConfig::default());
        p.set_foreground_app(Some("com.apple.Preferences".into()));
        assert!(refused(p.check()));
        p.grant_consent();
        assert!(p.check().is_ok());
        p.revoke_consent();
        assert!(refused(p.check()));
    }

    #[test]
    fn blocklist_matches_ids_and_names() {
        let p = Privacy::new(PrivacyConfig::default());
        p.grant_consent();
        for app in [
            "com.apple.MobileSMS",
            "Messages",
            "com.chase.MyBank",
            "1Password 8",
            "com.apple.mobileslideshow",
            "Photos",
            "com.apple.Health",
            "Wallet",
            "com.google.android.apps.messaging",
        ] {
            assert!(refused(p.check_app(Some(app))), "{app} should be blocked");
        }
        for app in ["com.apple.Preferences", "Settings", "com.android.chrome"] {
            assert!(p.check_app(Some(app)).is_ok(), "{app} should pass");
        }
        p.set_foreground_app(Some("WhatsApp".into()));
        assert!(refused(p.check()));
        // Real Android package ids (dumpsys gives only the id, not the label).
        for app in [
            "com.google.android.gm",
            "org.thoughtcrime.securesms",
            "com.facebook.orca",
            "com.samsung.android.messaging",
            "com.android.mms",
            "com.microsoft.office.outlook",
            "com.google.android.apps.walletnfcrel",
            "com.google.android.apps.nbu.paisa.user",
            "com.google.android.apps.photos",
            "com.chase.sig.android",
            "com.wf.wellsfargomobile",
            "com.whatsapp",
            "org.telegram.messenger",
            "com.google.android.apps.authenticator2",
        ] {
            assert!(refused(p.check_app(Some(app))), "{app} should be blocked");
        }
        for app in ["com.android.settings", "com.google.android.youtube"] {
            assert!(p.check_app(Some(app)).is_ok(), "{app} should pass");
        }
        p.set_foreground_app(Some("Settings".into()));
        assert!(p.check().is_ok());
        // Unknown app: refused by default.
        p.set_foreground_app(None);
        assert!(refused(p.check()));
    }

    #[test]
    fn config_is_editable() {
        let cfg = PrivacyConfig {
            blocklist: vec!["com.example.secret".into(), "gmail".into()],
            allow: vec!["GMAIL".into()],
            block_unknown_app: true,
        };
        let p = Privacy::new(cfg);
        p.grant_consent();
        assert!(refused(p.check_app(Some("com.example.secret"))));
        assert!(p.check_app(Some("Gmail")).is_ok());
        assert!(p.check_app(Some("Messages")).is_ok());
        assert!(refused(p.check_app(None)));

        let parsed: PrivacyConfig = serde_json::from_str(r#"{"block_unknown_app": true}"#).unwrap();
        assert!(parsed.block_unknown_app);
        assert_eq!(parsed.blocklist.len(), DEFAULT_BLOCKLIST.len());
    }
}
