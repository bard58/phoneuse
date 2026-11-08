//! Low-level HTTP client for `POST /v1/systemone`.
//!
//! This layer applies no privacy rules. Use [`ScreenJudge`](crate::ScreenJudge)
//! for screen content.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ap_core::{Error, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::types::{Answers, Asset, Question, SystemOneRequest, SystemOneResponse};
use crate::{API_KEY_ENV, DEFAULT_BASE_URL, DEFAULT_MODEL};

/// Retry settings for 429, 529, 5xx and network errors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(400),
            max_delay: Duration::from_secs(8),
        }
    }
}

impl RetryPolicy {
    /// Delay before retry number `attempt` (0-based). A `Retry-After`
    /// value from the server wins, capped at `max_delay`.
    pub fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(d) = retry_after {
            return d.min(self.max_delay);
        }
        let exp = self.base_delay.saturating_mul(1u32 << attempt.min(16));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let jitter = exp.mul_f64(f64::from(nanos % 1000) / 4000.0);
        (exp + jitter).min(self.max_delay)
    }
}

/// True for statuses worth retrying: 429, 529 and other 5xx.
pub fn retryable_status(status: u16) -> bool {
    status == 429 || status == 529 || (500..600).contains(&status)
}

/// Client options.
#[derive(Debug, Clone)]
pub struct JevOptions {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub retry: RetryPolicy,
}

impl JevOptions {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: DEFAULT_MODEL.into(),
            base_url: DEFAULT_BASE_URL.into(),
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            retry: RetryPolicy::default(),
        }
    }

    /// Read `TYPESAFE_API_KEY`, and optional `TYPESAFE_MODEL` and
    /// `TYPESAFE_BASE_URL`.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var(API_KEY_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                Error::setup(
                    "Jev is enabled but no TypeSafe API key is set",
                    format!(
                        "set the environment variable {API_KEY_ENV} to a key from https://typesafe.ai, or turn Jev off"
                    ),
                )
            })?;
        let mut opts = Self::new(key.trim());
        if let Some(m) = env_nonempty("TYPESAFE_MODEL") {
            opts.model = m;
        }
        if let Some(b) = env_nonempty("TYPESAFE_BASE_URL") {
            opts.base_url = b;
        }
        Ok(opts)
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Sends one request. Replace it in tests to avoid the network.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, request: &SystemOneRequest) -> Result<SystemOneResponse>;
}

/// reqwest transport with retries.
pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    api_key: String,
    retry: RetryPolicy,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl HttpTransport {
    pub fn new(opts: &JevOptions) -> Result<Self> {
        crate::install_crypto_provider();
        let client = reqwest::Client::builder()
            .timeout(opts.timeout)
            .connect_timeout(opts.connect_timeout)
            .user_agent(concat!("agentphone-jev/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Config(format!("cannot build HTTP client: {e}")))?;
        Ok(Self {
            client,
            url: format!("{}/v1/systemone", opts.base_url.trim_end_matches('/')),
            api_key: opts.api_key.clone(),
            retry: opts.retry,
        })
    }

    /// One attempt. `Err((error, retryable, retry_after))`.
    async fn once(
        &self,
        request: &SystemOneRequest,
    ) -> std::result::Result<SystemOneResponse, (Error, bool, Option<Duration>)> {
        let mut req = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(request);
        if request.needs_images_header() {
            req = req.header("TypeSafe-Danger-Zone-Images", "true");
        }
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
            .and_then(|s| Duration::try_from_secs_f64(s.min(3600.0)).ok());
        let body = resp.text().await.map_err(map_reqwest)?;
        if (200..300).contains(&status) {
            return serde_json::from_str(&body).map_err(|e| {
                (
                    Error::protocol(format!("cannot parse Jev response: {e}")),
                    false,
                    None,
                )
            });
        }
        Err((
            status_error(status, &body),
            retryable_status(status),
            retry_after,
        ))
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send(&self, request: &SystemOneRequest) -> Result<SystemOneResponse> {
        let mut attempt = 0u32;
        loop {
            let (err, retryable, retry_after) = match self.once(request).await {
                Ok(r) => return Ok(r),
                Err(e) => e,
            };
            if !retryable || attempt >= self.retry.max_retries {
                return Err(err);
            }
            let delay = self.retry.backoff(attempt, retry_after);
            tracing::warn!(attempt = attempt + 1, ?delay, error = %err, "Jev request failed, retrying");
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }
}

fn map_reqwest(e: reqwest::Error) -> (Error, bool, Option<Duration>) {
    let retryable = e.is_timeout() || e.is_connect() || e.is_request();
    (
        Error::Other(format!("Jev request failed: {e}")),
        retryable,
        None,
    )
}

/// Map an HTTP error status and body to an [`Error`].
pub fn status_error(status: u16, body: &str) -> Error {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("detail").cloned())
        .map(|d| match d {
            Value::String(s) => s,
            other => other.to_string(),
        })
        .unwrap_or_else(|| body.chars().take(500).collect());
    match status {
        401 => Error::setup(
            "TypeSafe rejected the API key (401)",
            format!("check {API_KEY_ENV}; create a new key at https://typesafe.ai if needed"),
        ),
        400 if detail.contains("image") || detail.contains("Danger") => Error::setup(
            format!("TypeSafe refused the image request (400): {detail}"),
            "use a model that accepts images (Jev 1.14+), for example set TYPESAFE_MODEL",
        ),
        _ => Error::protocol(format!("TypeSafe HTTP {status}: {detail}")),
    }
}

/// Jev client.
#[derive(Clone)]
pub struct JevClient {
    model: String,
    transport: Arc<dyn Transport>,
}

impl std::fmt::Debug for JevClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JevClient")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl JevClient {
    /// Client from env vars. Missing `TYPESAFE_API_KEY` gives
    /// [`Error::Setup`] with a hint.
    pub fn from_env() -> Result<Self> {
        Self::new(JevOptions::from_env()?)
    }

    pub fn new(opts: JevOptions) -> Result<Self> {
        Ok(Self {
            model: opts.model.clone(),
            transport: Arc::new(HttpTransport::new(&opts)?),
        })
    }

    /// Client with a custom transport (tests, proxies).
    pub fn with_transport(model: impl Into<String>, transport: Arc<dyn Transport>) -> Self {
        Self {
            model: model.into(),
            transport,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Build the request body without sending it.
    pub fn build_request(
        &self,
        state: Value,
        assets: BTreeMap<String, Asset>,
        questions: BTreeMap<String, Question>,
    ) -> SystemOneRequest {
        SystemOneRequest {
            model: self.model.clone(),
            state,
            assets,
            questions,
        }
    }

    /// Ask all `questions` about `state` in one request.
    pub async fn ask(
        &self,
        state: Value,
        assets: BTreeMap<String, Asset>,
        questions: BTreeMap<String, Question>,
    ) -> Result<Answers> {
        if questions.is_empty() {
            return Err(Error::InvalidArgument(
                "Jev needs at least one question".into(),
            ));
        }
        let request = self.build_request(state, assets, questions);
        let response = self.transport.send(&request).await?;
        for name in request.questions.keys() {
            if !response.answers.contains_key(name) {
                return Err(Error::protocol(format!("Jev did not answer '{name}'")));
            }
        }
        tracing::debug!(
            model = %response.model,
            input_tokens = response.usage.input_tokens,
            "Jev answered"
        );
        Ok(response.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_is_setup_error() {
        // Changing env vars needs `unsafe` in edition 2024 (denied here),
        // so this only runs when the key is not set on this machine.
        if std::env::var(API_KEY_ENV).is_err() {
            let err = JevClient::from_env().unwrap_err();
            assert!(err.hint().unwrap().contains(API_KEY_ENV));
        }
    }

    #[test]
    fn retry_rules() {
        for s in [429, 500, 502, 503, 529] {
            assert!(retryable_status(s), "{s}");
        }
        for s in [400, 401, 413, 415, 422] {
            assert!(!retryable_status(s), "{s}");
        }
        let p = RetryPolicy::default();
        assert_eq!(p.backoff(0, Some(Duration::from_secs(60))), p.max_delay);
        assert!(p.backoff(0, None) >= p.base_delay);
        assert!(p.backoff(10, None) <= p.max_delay);
    }

    #[test]
    fn error_mapping() {
        assert!(status_error(401, "").hint().is_some());
        let e = status_error(
            422,
            r#"{"detail":[{"loc":["body","questions"],"msg":"field required","type":"missing"}]}"#,
        );
        assert!(e.to_string().contains("field required"));
        let e = status_error(400, r#"{"detail":"images are not enabled"}"#);
        assert!(e.hint().is_some());
        assert!(status_error(529, "overloaded").to_string().contains("529"));
    }
}
