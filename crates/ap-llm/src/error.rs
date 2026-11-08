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
    /// True for errors a retry can fix: 408, 409, 429, 5xx, 529, network, timeout.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Api { status, kind, .. } => {
                retryable_status(*status)
                    || kind == "overloaded_error"
                    || kind == "rate_limit_error"
            }
            Error::Network(_) | Error::Timeout(_) => true,
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

pub(crate) fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500..=599)
}
