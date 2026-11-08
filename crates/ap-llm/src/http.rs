//! Shared HTTP plumbing: client construction, JSON POST with retry and backoff.

use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::error::{Error, Result, is_quota_kind};

/// Longest server `retry-after` the client waits for. A longer request gives
/// up at once instead of sleeping less than the server asked.
pub(crate) const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Retry settings for 408/409/429/5xx/529 and network errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// First backoff delay; doubles each retry.
    pub base_delay: Duration,
    /// Upper bound for one computed backoff delay. A server `retry-after` is
    /// honored up to the larger of this and 60 s; past that the call gives up.
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

    /// Delay before the next attempt, or `None` when the server asked for a
    /// wait longer than the client accepts.
    fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        if let Some(d) = retry_after {
            return (d <= self.max_delay.max(MAX_RETRY_AFTER)).then_some(d);
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
        Some((exp + jitter).min(self.max_delay.max(exp)))
    }
}

/// Whole-request timeout for one call: the configured floor, raised for
/// large `max_tokens` (about 30 output tokens per second) so a long
/// non-streaming generation is not cut off while the server still bills it.
pub(crate) fn request_timeout(floor: Duration, max_tokens: u32) -> Duration {
    floor.max(Duration::from_secs(u64::from(max_tokens) / 30))
}

/// Parse `retry-after-ms` (milliseconds) or `retry-after` (seconds).
/// Values are clamped before conversion, so huge values cannot panic.
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let num = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
    };
    if let Some(ms) = num("retry-after-ms") {
        return Duration::try_from_secs_f64(ms.min(3_600_000.0) / 1000.0).ok();
    }
    num("retry-after").and_then(|s| Duration::try_from_secs_f64(s.min(3600.0)).ok())
}

/// Build a reqwest client with rustls (ring) and a connect timeout.
/// The whole-request timeout is set per request (see `PostJson::timeout`).
pub(crate) fn build_client(connect_timeout: Duration) -> Result<reqwest::Client> {
    crate::install_crypto_provider();
    reqwest::Client::builder()
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

/// A failed attempt: the error, the server's `retry-after`, and its
/// `x-should-retry` answer when it sent one.
struct Failure {
    err: Error,
    retry_after: Option<Duration>,
    should_retry: Option<bool>,
}

impl From<Error> for Failure {
    fn from(err: Error) -> Self {
        Self {
            err,
            retry_after: None,
            should_retry: None,
        }
    }
}

impl Failure {
    fn retryable(&self) -> bool {
        if let Error::Api { kind, .. } = &self.err
            && is_quota_kind(kind)
        {
            return false;
        }
        self.should_retry.unwrap_or_else(|| self.err.is_retryable())
    }
}

pub(crate) struct PostJson<'a> {
    pub client: &'a reqwest::Client,
    pub url: &'a str,
    pub headers: &'a HeaderMap,
    pub body: &'a Value,
    pub policy: &'a RetryPolicy,
    /// Whole-request timeout for each attempt.
    pub timeout: Duration,
    pub provider: &'static str,
    pub parse_error: ErrorParser,
}

impl PostJson<'_> {
    /// POST with retries. Returns the body of the first 2xx response.
    pub async fn send(&self) -> Result<RawResponse> {
        let mut attempt = 0u32;
        loop {
            let f = match self.once().await {
                Ok(r) => return Ok(r),
                Err(f) => f,
            };
            if !f.retryable() || attempt >= self.policy.max_retries {
                return Err(f.err);
            }
            let Some(delay) = self.policy.backoff(attempt, f.retry_after) else {
                tracing::warn!(
                    provider = self.provider,
                    retry_after = ?f.retry_after,
                    "server asked for a longer wait than the client accepts; not retrying"
                );
                return Err(f.err);
            };
            let err = f.err;
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

    async fn once(&self) -> std::result::Result<RawResponse, Failure> {
        let resp = self
            .client
            .post(self.url)
            .headers(self.headers.clone())
            .timeout(self.timeout)
            .json(self.body)
            .send()
            .await
            .map_err(|e| Failure::from(self.map_reqwest(e)))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp
            .text()
            .await
            .map_err(|e| Failure::from(self.map_reqwest(e)))?;
        if (200..300).contains(&status) {
            return Ok(RawResponse { body });
        }
        let (kind, message) = (self.parse_error)(status, &body);
        let request_id = ["request-id", "x-request-id"]
            .iter()
            .find_map(|h| headers.get(*h))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let should_retry = headers
            .get("x-should-retry")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| match s.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });
        Err(Failure {
            err: Error::Api {
                provider: self.provider,
                status,
                kind,
                message,
                request_id,
            },
            retry_after: parse_retry_after(&headers),
            should_retry,
        })
    }

    fn map_reqwest(&self, e: reqwest::Error) -> Error {
        if e.is_connect() {
            // Nothing reached the server (connect timeouts included): safe to retry.
            Error::Network(e.to_string())
        } else if e.is_timeout() {
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
        let d0 = p.backoff(0, None).unwrap();
        let d3 = p.backoff(3, None).unwrap();
        assert!(d0 >= Duration::from_millis(100) && d0 <= Duration::from_millis(125));
        assert!(d3 >= Duration::from_millis(800) && d3 <= Duration::from_millis(1000));
        assert_eq!(p.backoff(10, None), Some(Duration::from_millis(1000)));
    }

    #[test]
    fn backoff_honors_retry_after_up_to_bound() {
        let p = RetryPolicy::default();
        // Not shortened to max_delay (30 s).
        assert_eq!(
            p.backoff(0, Some(Duration::from_secs(45))),
            Some(Duration::from_secs(45))
        );
        assert_eq!(p.backoff(0, Some(MAX_RETRY_AFTER)), Some(MAX_RETRY_AFTER));
        // Longer than the client accepts: give up.
        assert_eq!(p.backoff(0, Some(Duration::from_secs(61))), None);
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn retry_after_parsing_never_panics() {
        assert_eq!(
            parse_retry_after(&headers(&[("retry-after", "1e20")])),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(
            parse_retry_after(&headers(&[("retry-after", "2.5")])),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(
            parse_retry_after(&headers(&[
                ("retry-after-ms", "1500"),
                ("retry-after", "9")
            ])),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(parse_retry_after(&headers(&[("retry-after", "NaN")])), None);
        assert_eq!(parse_retry_after(&headers(&[("retry-after", "-1")])), None);
        assert_eq!(parse_retry_after(&HeaderMap::new()), None);
    }

    #[test]
    fn request_timeout_scales_with_max_tokens() {
        let floor = Duration::from_secs(600);
        assert_eq!(request_timeout(floor, 16_000), floor);
        assert_eq!(request_timeout(floor, 64_000), Duration::from_secs(2133));
    }
}
