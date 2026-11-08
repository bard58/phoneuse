//! User configuration, stored as TOML in the platform config directory
//! (`directories::ProjectDirs` for "agentphone").
//!
//! Every field has a default, so an empty or partial file is valid.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Default LLM model id.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// Top-level configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Device id to use when none is given. `None` = the only connected device.
    pub default_device: Option<String>,
    pub llm: LlmConfig,
    pub safety: SafetyConfig,
    pub ios: IosConfig,
    pub android: AndroidConfig,
}

/// LLM provider settings. API keys are never stored here, only env var names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// "anthropic" or "openai" (OpenAI-compatible).
    pub provider: String,
    pub model: String,
    /// Optional base URL override (proxies, local OpenAI-compatible servers).
    pub base_url: Option<String>,
    pub anthropic_api_key_env: String,
    pub openai_api_key_env: String,
    pub max_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "anthropic".into(),
            model: DEFAULT_MODEL.into(),
            base_url: None,
            anthropic_api_key_env: "ANTHROPIC_API_KEY".into(),
            openai_api_key_env: "OPENAI_API_KEY".into(),
            max_tokens: 4096,
        }
    }
}

impl LlmConfig {
    /// Env var holding the API key for the configured provider.
    pub fn api_key_env(&self) -> &str {
        if self.provider.eq_ignore_ascii_case("openai") {
            &self.openai_api_key_env
        } else {
            &self.anthropic_api_key_env
        }
    }

    /// Read the API key from the environment.
    pub fn api_key(&self) -> Result<String> {
        let var = self.api_key_env();
        std::env::var(var)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                Error::setup(
                    format!("no API key for provider '{}'", self.provider),
                    format!("set the environment variable {var} to your API key"),
                )
            })
    }
}

/// Safety rules for the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    /// Ask the user before sending, buying, deleting, posting, ...
    pub confirm_irreversible: bool,
    /// Maximum agent steps per task.
    pub max_steps: u32,
    /// Let the agent unlock the phone (owner approval still required).
    pub allow_unlock: bool,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            confirm_irreversible: true,
            max_steps: 50,
            allow_unlock: false,
        }
    }
}

/// iOS backend settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IosConfig {
    /// Backends to try in order: "hid" (CoreDevice HID), "wda" (WebDriverAgent), "dongle".
    pub backend_order: Vec<String>,
    /// URL of an already-running WebDriverAgent, if any.
    pub wda_url: Option<String>,
    /// WDA port on the device.
    pub wda_port: u16,
    /// WDA bundle id prefix to launch through XCTest.
    pub wda_bundle_id: String,
    /// Folder with pairing files; `None` = platform default.
    pub pairing_dir: Option<PathBuf>,
}

impl Default for IosConfig {
    fn default() -> Self {
        Self {
            backend_order: vec!["hid".into(), "wda".into()],
            wda_url: None,
            wda_port: 8100,
            wda_bundle_id: "com.facebook.WebDriverAgentRunner.xctrunner".into(),
            pairing_dir: None,
        }
    }
}

/// Android backend settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AndroidConfig {
    /// Path to `adb`; `None` = search PATH and common SDK folders.
    pub adb_path: Option<PathBuf>,
    pub adb_host: String,
    pub adb_port: u16,
}

impl Default for AndroidConfig {
    fn default() -> Self {
        Self {
            adb_path: None,
            adb_host: "127.0.0.1".into(),
            adb_port: 5037,
        }
    }
}

impl Config {
    /// Platform config directory, e.g. `%APPDATA%\agentphone\config` on Windows.
    pub fn dir() -> Result<PathBuf> {
        directories::ProjectDirs::from("", "", "agentphone")
            .map(|d| d.config_dir().to_path_buf())
            .ok_or_else(|| Error::Config("cannot find the user's home directory".into()))
    }

    /// Default config file path.
    pub fn default_path() -> Result<PathBuf> {
        Ok(Self::dir()?.join("config.toml"))
    }

    /// Parse TOML text.
    pub fn from_toml(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }

    /// Serialize to TOML.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load from `path`; a missing file gives defaults.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                Self::from_toml(&s).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Load from the default path.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_path()?)
    }

    /// Write to `path`, creating parent folders.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_toml()?)?;
        Ok(())
    }

    /// Write to the default path; returns it.
    pub fn save(&self) -> Result<PathBuf> {
        let p = Self::default_path()?;
        self.save_to(&p)?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.llm.model, "claude-opus-5-5");
        assert!(c.safety.confirm_irreversible);
        assert_eq!(c.safety.max_steps, 50);
        assert!(!c.safety.allow_unlock);
        assert_eq!(c.ios.backend_order, vec!["hid", "wda"]);
        assert_eq!(c.llm.api_key_env(), "ANTHROPIC_API_KEY");
    }

    #[test]
    fn partial_toml_and_round_trip() {
        let c =
            Config::from_toml("[safety]\nmax_steps = 7\n[llm]\nprovider = \"openai\"\n").unwrap();
        assert_eq!(c.safety.max_steps, 7);
        assert!(c.safety.confirm_irreversible);
        assert_eq!(c.llm.api_key_env(), "OPENAI_API_KEY");
        let back = Config::from_toml(&c.to_toml().unwrap()).unwrap();
        assert_eq!(back, c);
        assert!(Config::from_toml("safety = 3").is_err());
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("agentphone-test-{}", std::process::id()));
        let path = dir.join("sub").join("config.toml");
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
        let c = Config {
            default_device: Some("abc".into()),
            ..Config::default()
        };
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), c);
        let _ = std::fs::remove_dir_all(dir);
    }
}
