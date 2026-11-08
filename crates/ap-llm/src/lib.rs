//! LLM clients for agentphone.
//!
//! - [`Provider`]: one async call, `complete(Request) -> Response`, over a
//!   provider-neutral message model ([`types`]).
//! - [`AnthropicProvider`]: Claude Messages API (default model `claude-opus-5-5`),
//!   prompt caching on tools + system, retry with backoff, timeouts.
//! - [`OpenAiCompatibleProvider`]: `/chat/completions` with tools and
//!   `image_url` data URIs (OpenAI, OpenRouter, Ollama, LM Studio).
//! - [`pricing`]: per-model prices for a cost estimate.
//!
//! TLS: the workspace builds reqwest with `rustls-no-provider`. Every client
//! constructor here calls [`install_crypto_provider`], which installs the
//! rustls `ring` provider as the process default once.

pub mod anthropic;
pub mod error;
mod http;
pub mod openai;
pub mod pricing;
pub mod types;

#[cfg(test)]
pub(crate) mod test_server;

use std::sync::Arc;

pub use anthropic::{AnthropicOptions, AnthropicProvider};
pub use ap_core;
pub use error::{Error, Result};
pub use http::RetryPolicy;
pub use openai::{OpenAiCompatibleProvider, OpenAiOptions};
pub use types::{
    ContentBlock, Effort, ImageMediaType, Message, Request, Response, Role, StopReason, ToolDef,
    Usage,
};

/// Claude model ids this crate knows (verified against the claude-api skill, 2026-11-08).
pub mod models {
    /// Default: Claude Opus 5.5.
    pub const CLAUDE_OPUS_5_5: &str = "claude-opus-5-5";
    /// Claude Sonnet 5.5.
    pub const CLAUDE_SONNET_5_5: &str = "claude-sonnet-5-5";
    /// Claude Haiku 5.5.
    pub const CLAUDE_HAIKU_5_5: &str = "claude-haiku-5-5";
    /// The ids above, default first.
    pub const KNOWN: [&str; 3] = [CLAUDE_OPUS_5_5, CLAUDE_SONNET_5_5, CLAUDE_HAIKU_5_5];

    pub fn is_known(id: &str) -> bool {
        KNOWN.contains(&id)
    }
}

/// One LLM backend. Implementations must be cheap to share (`Arc<dyn Provider>`).
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Short name for logs ("anthropic", "openai").
    fn name(&self) -> &str;

    /// Model used when [`Request::model`] is `None`.
    fn default_model(&self) -> &str;

    /// Send one request and wait for the full response.
    async fn complete(&self, request: Request) -> Result<Response>;
}

/// Install the rustls `ring` crypto provider as the process default.
/// Safe to call many times; a provider installed earlier (by any crate) is kept.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            // Err means another thread installed one first: that is fine.
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
    });
}

/// Build the provider named in the config ("anthropic" or "openai").
/// Reads the API key from the env var the config names. For "openai" a
/// missing key is allowed when `base_url` points at a local server.
pub fn provider_from_config(cfg: &ap_core::LlmConfig) -> Result<Arc<dyn Provider>> {
    match cfg.provider.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => {
            let key = cfg.api_key().map_err(|e| Error::Config(e.to_string()))?;
            let mut opts = AnthropicOptions::new(key);
            opts.model = cfg.model.clone();
            if let Some(b) = &cfg.base_url {
                opts.base_url = b.clone();
            }
            Ok(Arc::new(AnthropicProvider::new(opts)?))
        }
        "openai" | "openai-compatible" | "openrouter" | "ollama" | "lmstudio" => {
            // Ollama / LM Studio return Ok("") when no key is set.
            let key = cfg.api_key().ok().filter(|k| !k.is_empty());
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| openai::DEFAULT_BASE_URL.to_string());
            if key.is_none() && !openai::is_local(&base) && !cfg.key_optional() {
                return Err(Error::Config(format!(
                    "no API key: set the environment variable {}",
                    cfg.api_key_env()
                )));
            }
            let mut opts = OpenAiOptions::new(cfg.model.clone());
            opts.api_key = key;
            opts.base_url = base;
            Ok(Arc::new(OpenAiCompatibleProvider::new(opts)?))
        }
        other => Err(Error::Config(format!(
            "unknown LLM provider '{other}' (use \"anthropic\" or \"openai\")"
        ))),
    }
}
