//! `agentphone run --jev`: add Jev screen judgments to what the agent sees.
//!
//! Each settled screen is also sent to Jev (TypeSafe). The answers
//! (locked, keyboard, alert, still loading, private data) are added to the
//! observation text. The user must agree once per run.
//!
//! Before each Jev call the observer asks the device for the foreground app
//! (Android: `dumpsys window`; iOS: the app opened by the last `open_app`,
//! else unknown) and gives it to the privacy guard. The guard does not send
//! the screen when the app is unknown or when its id matches the blocklist
//! (`ap_jev::DEFAULT_BLOCKLIST`, substring match). A Jev error or refusal
//! never stops the run.

use anyhow::Result;
use ap_agent::{Observation, Observer, ScreenObserver};
use ap_core::{Device, Frame};
use ap_jev::{JevClient, Privacy, PrivacyConfig, ScreenJudge, ScreenJudgment};

/// The consent question. It states what the code does: see the module docs.
pub const CONSENT_PROMPT: &str = "Jev sends a screenshot of your phone to typesafe.ai at each agent step. A screenshot is not sent when agentphone cannot tell which app is open, or when the app id contains a blocklist word (for example bank, wallet, messages, password, photos, health). Apps with other ids are sent. Allow for this run?";

pub struct JevObserver {
    inner: ScreenObserver,
    judge: ScreenJudge,
}

/// One line for the model.
pub fn judgment_text(j: &ScreenJudgment) -> String {
    let mut notes = Vec::new();
    if j.locked {
        notes.push("the phone is locked");
    }
    if j.alert_visible {
        notes.push("a dialog or alert is open");
    }
    if j.keyboard_visible {
        notes.push("the keyboard is shown");
    }
    if !j.settled {
        notes.push("the screen may still be loading");
    }
    if j.sensitive >= 0.5 {
        notes.push("the screen may show private data: do not copy it anywhere");
    }
    if notes.is_empty() {
        "Quick screen check (Jev): nothing special.".into()
    } else {
        format!("Quick screen check (Jev): {}.", notes.join("; "))
    }
}

impl JevObserver {
    async fn annotate(&self, device: &dyn Device, mut obs: Observation) -> Observation {
        let app = match device.foreground_app().await {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!("foreground app unknown: {e}");
                None
            }
        };
        self.judge.privacy().set_foreground_app(app);
        if let Err(e) = self.judge.privacy().check() {
            tracing::info!("Jev check skipped: {e}");
            return obs;
        }
        let frame = match Frame::from_encoded(&obs.jpeg) {
            Ok(f) => f,
            Err(_) => return obs,
        };
        match self.judge.judge(&frame, None).await {
            Ok(j) => {
                obs.text.push(' ');
                obs.text.push_str(&judgment_text(&j));
            }
            Err(e) => tracing::info!("Jev check skipped: {e}"),
        }
        obs
    }
}

#[async_trait::async_trait]
impl Observer for JevObserver {
    async fn look(&self, device: &dyn Device) -> ap_core::Result<Observation> {
        let obs = self.inner.look(device).await?;
        Ok(self.annotate(device, obs).await)
    }

    async fn after_action(&self, device: &dyn Device) -> ap_core::Result<Observation> {
        // Strips can hold several frames; keep them fast and unjudged.
        self.inner.after_action(device).await
    }
}

/// Build the observer after asking the user for consent.
pub fn observer(json: bool) -> Result<Box<dyn Observer>> {
    let client = JevClient::from_env()?;
    let privacy = Privacy::new(PrivacyConfig::default());
    let agreed = !json
        && crate::ui::interactive()
        && dialoguer::Confirm::new()
            .with_prompt(CONSENT_PROMPT)
            .default(false)
            .interact()
            .unwrap_or(false);
    if !agreed {
        anyhow::bail!("Jev was not allowed for this run. Run again without --jev, or answer yes");
    }
    privacy.grant_consent();
    Ok(Box::new(JevObserver {
        inner: ScreenObserver::default(),
        judge: ScreenJudge::new(client, privacy),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use ap_jev::{SystemOneRequest, SystemOneResponse, Transport};

    struct Counting(Mutex<usize>);

    #[async_trait::async_trait]
    impl Transport for Counting {
        async fn send(&self, _r: &SystemOneRequest) -> ap_core::Result<SystemOneResponse> {
            *self.0.lock().unwrap() += 1;
            Err(ap_core::Error::Other("test transport".into()))
        }
    }

    #[tokio::test]
    async fn unknown_app_is_not_sent() {
        let fake = Arc::new(Counting(Mutex::new(0)));
        let client = JevClient::with_transport("jev-latest", fake.clone());
        let privacy = Privacy::new(PrivacyConfig::default());
        privacy.grant_consent();
        let o = JevObserver {
            inner: ScreenObserver::instant(),
            judge: ScreenJudge::new(client, privacy),
        };
        // The mock device cannot tell the foreground app: nothing goes to Jev.
        let m = ap_core::mock::MockDevice::new();
        let obs = o.look(&m).await.unwrap();
        assert!(!obs.text.contains("Jev"));
        assert_eq!(*fake.0.lock().unwrap(), 0);
        assert_eq!(o.judge.privacy().foreground_app(), None);
    }

    #[test]
    fn consent_prompt_matches_code() {
        assert!(PrivacyConfig::default().block_unknown_app);
        assert!(!CONSENT_PROMPT.contains("never sent"));
        assert!(CONSENT_PROMPT.contains("cannot tell which app"));
    }

    #[test]
    fn text_for_judgment() {
        let j = ScreenJudgment {
            settled: true,
            locked: true,
            keyboard_visible: false,
            alert_visible: true,
            action_irreversible: 0.0,
            sensitive: 0.0,
            probabilities: Default::default(),
            model: "jev".into(),
            usage: Default::default(),
        };
        let t = judgment_text(&j);
        assert!(t.contains("locked") && t.contains("dialog"));
    }
}
