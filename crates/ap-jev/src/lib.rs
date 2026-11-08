//! Typed screen judgments from Jev, the TypeSafe System One model.
//!
//! Jev does not write text. It answers typed questions about a state
//! (JSON plus images) with probabilities. This crate uses it for fast,
//! cheap checks that would otherwise need a full LLM call:
//!
//! - [`JevClient`]: low-level HTTP client for `POST /v1/systemone`.
//!   It applies no privacy rules.
//! - [`ScreenJudge`]: high-level checks on a phone [`Frame`](ap_core::Frame):
//!   [`ScreenJudge::judge`], [`ScreenJudge::is_irreversible`],
//!   [`ScreenJudge::pick_target`] and [`ScreenJudge::locate`].
//!   Every call goes through the [`Privacy`] guard first.
//! - [`image`]: resizes frames into the size range the API accepts.
//!
//! Privacy: a call sends the screenshot to a third-party service.
//! [`ScreenJudge`] refuses with [`Error::Refused`](ap_core::Error::Refused)
//! until the user gives consent for the run, and for blocked apps.

pub mod client;
pub mod image;
pub mod judge;
pub mod privacy;
pub mod types;

pub use client::{HttpTransport, JevClient, JevOptions, RetryPolicy, Transport};
pub use image::{ImageAsset, prepare_frame};
pub use judge::{
    JudgeConfig, PickInput, ScreenJudge, ScreenJudgment, ScreenProbabilities, TargetPick,
};
pub use privacy::{Privacy, PrivacyConfig};
pub use types::{
    Answer, Answers, Asset, BoxAnswer, BoxCoords, Choice, ImageFormat, Noul, NoulCriteria,
    Question, Score, SystemOneRequest, SystemOneResponse, Usage,
};

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Default model alias.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// Env var that holds the API key.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// Install the rustls `ring` crypto provider as the process default.
/// The workspace builds reqwest with `rustls-no-provider`.
/// Safe to call many times. A provider that another crate installed is kept.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            // Err means another thread installed one first. That is fine.
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
    });
}
