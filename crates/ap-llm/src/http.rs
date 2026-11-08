//! Shared HTTP plumbing: client construction, JSON POST with retry and backoff.

use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::error::{Error, Result, retryable_status};

/// Retry settings for 408/409/429/5xx/529 and network errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// First backoff delay; doubles each retry.
    pub base_delay: Duration,
    /// Upper bound for one delay (also caps `retry-after`).
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 4,
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// No retries (tests, callers with their own retry).
    pub const NONE: RetryPolicy = RetryPolicy {
        max_retries: 0,
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
    };

    fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(d) = retry_after {
            return d.min(self.max_delay);
        }
        let exp = self
            .base_delay
            .saturating_mul(1u32 << attempt.min(16))
            .min(self.max_delay);
        // Up to 25% jitter, without a rand dependency.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let jitter = exp.mul_f64(f64::from(nanos % 1000) / 4000.0);
        (exp + jitter).min(self.max_delay.max(exp))
    }
}

/// Build a reqwest client with rustls (ring) and timeouts.
pub(crate) fn build_client(
    timeout: Duration,
    connect_timeout: Duration,
) -> Result<reqwest::Client> {
    crate::install_crypto_provider();
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .user_agent(concat!("agentphone/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Config(format!("cannot build HTTP client: {e}")))
}

pub(crate) struct RawResponse {
    pub body: String,
}

/// Parses an error body into (kind, message).
pub(crate) type ErrorParser = fn(u16, &str) -> (String, String);

pub(crate) struct PostJson<'a> {
    pub client: &'a reqwest::Client,
    pub url: &'a str,
    pub headers: &'a HeaderMap,
    pub body: &'a Value,
    pub policy: &'a RetryPolicy,
    pub timeout: Duration,
    pub provider: &'static str,
    pub parse_error: ErrorParser,
}

impl PostJson<'_> {
    /// POST with retries. Returns the body of the first 2xx response.
    pub async fn send(&self) -> Result<RawResponse> {
        let mut attempt = 0u32;
        loop {
            let (err, retry_after) = match self.once().await {
                Ok(r) => return Ok(r),
                Err(e) => e,
            };
            if !err.is_retryable() || attempt >= self.policy.max_retries {
                return Err(err);
            }
            let delay = self.policy.backoff(attempt, retry_after);
            tracing::warn!(
                provider = self.provider,
                attempt = attempt + 1,
                ?delay,
                error = %err,
                "LLM request failed, retrying"
            );
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    async fn once(&self) -> std::result::Result<RawResponse, (Error, Option<Duration>)> {
        let resp = self
            .client
            .post(self.url)
            .headers(self.headers.clone())
            .json(self.body)
            .send()
            .await
            .map_err(|e| (self.map_reqwest(e), None))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.text().await.map_err(|e| (self.map_reqwest(e), None))?;
        if (200..300).contains(&status) {
            return Ok(RawResponse { body });
        }
        let (kind, message) = (self.parse_error)(status, &body);
        let request_id = ["request-id", "x-request-id"]
            .iter()
            .find_map(|h| headers.get(*h))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let retry_after = if retryable_status(status) {
            headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(Duration::from_secs_f64)
        } else {
            None
        };
        Err((
            Error::Api {
                provider: self.provider,
                status,
                kind,
                message,
                request_id,
            },
            retry_after,
        ))
    }

    fn map_reqwest(&self, e: reqwest::Error) -> Error {
        if e.is_timeout() {
            Error::Timeout(self.timeout)
        } else if e.is_decode() {
            Error::Decode(e.to_string())
        } else {
            Error::Network(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy {
            max_retries: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(1000),
        };
        let d0 = p.backoff(0, None);
        let d3 = p.backoff(3, None);
        assert!(d0 >= Duration::from_millis(100) && d0 <= Duration::from_millis(125));
        assert!(d3 >= Duration::from_millis(800) && d3 <= Duration::from_millis(1000));
        assert_eq!(p.backoff(10, None), Duration::from_millis(1000));
        assert_eq!(
            p.backoff(0, Some(Duration::from_secs(60))),
            Duration::from_millis(1000)
        );
    }
}
