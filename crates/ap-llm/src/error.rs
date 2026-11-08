//! Errors from LLM providers.

use std::time::Duration;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The API answered with an error status.
    #[error("{provider} API error {status} ({kind}): {message}")]
    Api {
        provider: &'static str,
        status: u16,
        /// Provider error type, e.g. `rate_limit_error`, `overloaded_error`.
        kind: String,
        message: String,
        /// Request id from the response headers, for support tickets.
        request_id: Option<String>,
    },

    /// Connection failed, reset or TLS error.
    #[error("network error: {0}")]
    Network(String),

    #[error("request timed out after {0:?}")]
    Timeout(Duration),

    /// The response body did not match the expected shape.
    #[error("cannot decode response: {0}")]
    Decode(String),

    /// Bad options (missing key, unknown provider, ...).
    #[error("LLM config error: {0}")]
    Config(String),

    /// The request is invalid before sending (e.g. empty messages).
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl Error {
    /// True for errors a retry can fix: 408, 409, 429, 5xx, 529 and network
    /// errors (connect timeouts included).
    ///
    /// False for quota and billing errors (a retry cannot fix them) and for
    /// [`Error::Timeout`]: the request reached the server, which may still
    /// finish and bill it, so a resend can bill the same long call again.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Api { kind, .. } if is_quota_kind(kind) => false,
            Error::Api { status, kind, .. } => {
                retryable_status(*status)
                    || kind == "overloaded_error"
                    || kind == "rate_limit_error"
            }
            Error::Network(_) => true,
            _ => false,
        }
    }

    /// HTTP status, if this is an API error.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// Error kinds (OpenAI `type` or `code`) that no retry can fix.
pub(crate) fn is_quota_kind(kind: &str) -> bool {
    matches!(
        kind,
        "insufficient_quota" | "billing_hard_limit_reached" | "billing_not_active"
    )
}

pub(crate) fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500..=599)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16, kind: &str) -> Error {
        Error::Api {
            provider: "openai",
            status,
            kind: kind.into(),
            message: String::new(),
            request_id: None,
        }
    }

    #[test]
    fn retry_rules() {
        assert!(api(429, "rate_limit_error").is_retryable());
        assert!(api(529, "overloaded_error").is_retryable());
        assert!(!api(429, "insufficient_quota").is_retryable());
        assert!(!api(429, "billing_hard_limit_reached").is_retryable());
        assert!(!api(400, "invalid_request_error").is_retryable());
        assert!(Error::Network("reset".into()).is_retryable());
        assert!(!Error::Timeout(Duration::from_secs(600)).is_retryable());
    }
}
