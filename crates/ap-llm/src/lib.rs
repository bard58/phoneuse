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

/// Default base URL for a provider name when `llm.base_url` is not set.
///
/// Ollama and LM Studio default to their local servers, so a missing
/// `base_url` never sends screenshots to a cloud API.
pub fn default_base_url(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => anthropic::DEFAULT_BASE_URL,
        "ollama" => "http://localhost:11434/v1",
        "lmstudio" | "lm-studio" | "lm_studio" => "http://localhost:1234/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        _ => openai::DEFAULT_BASE_URL,
    }
}

/// The base URL the provider will use: `llm.base_url` when set and not
/// blank, else [`default_base_url`].
pub fn resolved_base_url(cfg: &ap_core::LlmConfig) -> String {
    cfg.base_url
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_base_url(&cfg.provider).to_string())
}

/// True when the config needs an API key in the environment to run.
/// Always true for Anthropic. False for OpenAI-compatible providers that
/// need no key (Ollama, LM Studio) or that point at a local server.
/// [`provider_from_config`] uses the same rule.
pub fn key_required(cfg: &ap_core::LlmConfig) -> bool {
    if cfg.is_anthropic() {
        return true;
    }
    !cfg.key_optional() && !openai::is_local(&resolved_base_url(cfg))
}

/// Build the provider named in the config. "anthropic" (or "claude") uses
/// the Messages API; every other name uses the OpenAI wire format.
/// Reads the API key from the env var the config names. A missing key is
/// allowed when [`key_required`] is false.
pub fn provider_from_config(cfg: &ap_core::LlmConfig) -> Result<Arc<dyn Provider>> {
    if cfg.provider.trim().is_empty() {
        return Err(Error::Config(
            "llm.provider is empty (use \"anthropic\" or an OpenAI-compatible name such as \"openai\")"
                .into(),
        ));
    }
    let base = resolved_base_url(cfg);
    if cfg.is_anthropic() {
        let key = cfg.api_key().map_err(|e| Error::Config(e.to_string()))?;
        let mut opts = AnthropicOptions::new(key);
        opts.model = cfg.model.clone();
        opts.base_url = base;
        return Ok(Arc::new(AnthropicProvider::new(opts)?));
    }
    // Ollama / LM Studio return Ok("") when no key is set.
    let key = cfg.api_key().ok().filter(|k| !k.is_empty());
    if key.is_none() && key_required(cfg) {
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

#[cfg(test)]
mod config_tests {
    use super::*;

    fn cfg(provider: &str, base_url: Option<&str>) -> ap_core::LlmConfig {
        ap_core::LlmConfig {
            provider: provider.into(),
            model: "m".into(),
            base_url: base_url.map(str::to_string),
            // Env vars that no test environment sets.
            anthropic_api_key_env: "AP_LLM_TEST_NO_SUCH_KEY_A".into(),
            openai_api_key_env: "AP_LLM_TEST_NO_SUCH_KEY_O".into(),
            ..Default::default()
        }
    }

    #[test]
    fn local_providers_default_to_localhost() {
        assert_eq!(
            resolved_base_url(&cfg("ollama", None)),
            "http://localhost:11434/v1"
        );
        assert_eq!(
            resolved_base_url(&cfg(" Ollama ", Some("  "))),
            "http://localhost:11434/v1"
        );
        for p in ["lmstudio", "lm-studio", "lm_studio"] {
            assert_eq!(resolved_base_url(&cfg(p, None)), "http://localhost:1234/v1");
        }
        assert_eq!(
            resolved_base_url(&cfg("openrouter", None)),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            resolved_base_url(&cfg("ollama", Some("http://10.0.0.5:11434/v1"))),
            "http://10.0.0.5:11434/v1"
        );
    }

    #[test]
    fn provider_names_are_trimmed_and_open_ended() {
        // Local / key-optional names build without a key.
        for p in ["ollama", "lm-studio", "lm_studio", " lmstudio "] {
            let pr = provider_from_config(&cfg(p, None)).unwrap();
            assert_eq!(pr.name(), "openai", "provider {p}");
        }
        // Any other name is OpenAI-compatible; a local base_url needs no key.
        let pr = provider_from_config(&cfg("together", Some("http://127.0.0.1:9/v1"))).unwrap();
        assert_eq!(pr.name(), "openai");
        // A remote OpenAI-compatible host still needs a key.
        let e = provider_from_config(&cfg("together", None)).err().unwrap();
        assert!(matches!(e, Error::Config(_)), "{e:?}");
        // " anthropic" with a stray space is Anthropic (and needs a key).
        let e = provider_from_config(&cfg(" anthropic", None))
            .err()
            .unwrap();
        assert!(e.to_string().contains("API key"), "{e}");
        assert!(provider_from_config(&cfg("  ", None)).is_err());
    }

    #[test]
    fn key_required_rule() {
        assert!(key_required(&cfg("anthropic", None)));
        assert!(key_required(&cfg("openai", None)));
        assert!(!key_required(&cfg(
            "openai",
            Some("http://localhost:11434/v1")
        )));
        assert!(!key_required(&cfg("ollama", None)));
        assert!(!key_required(&cfg("lm-studio", None)));
    }
}
