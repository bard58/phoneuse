//! `agentphone config ...` and config file helpers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ap_core::Config;
use toml::Value;

use crate::cli::ConfigAction;
use crate::ui;

/// Where the config file lives (`--config` / `AGENTPHONE_CONFIG`, else the platform default).
pub fn config_path(over: Option<&Path>) -> Result<PathBuf> {
    match over {
        Some(p) => Ok(p.to_path_buf()),
        None => Ok(Config::default_path()?),
    }
}

/// Load the config (defaults when the file is missing).
pub fn load(path: &Path) -> Result<Config> {
    Ok(Config::load_from(path)?)
}

/// Save the config.
pub fn save(cfg: &Config, path: &Path) -> Result<()> {
    cfg.save_to(path)
        .with_context(|| format!("cannot write {}", path.display()))
}

pub fn run(action: Option<ConfigAction>, path: &Path, json: bool) -> Result<()> {
    match action.unwrap_or(ConfigAction::Show) {
        ConfigAction::Show => {
            let cfg = load(path)?;
            if json {
                ui::json(&cfg)?;
            } else {
                if !path.exists() {
                    ui::warn(format!(
                        "{} does not exist yet; these are the defaults. Run `agentphone setup`.",
                        path.display()
                    ));
                }
                print!("{}", cfg.to_toml()?);
            }
        }
        ConfigAction::Path => println!("{}", path.display()),
        ConfigAction::Set { key, value } => {
            let cfg = load(path)?;
            let new = set_value(&cfg, &key, &value)?;
            save(&new, path)?;
            ui::ok(format!("{key} = {}", display_value(&new, &key)?));
        }
        ConfigAction::Edit => edit(path)?,
    }
    Ok(())
}

/// Every settable key ("llm.model", "default_device", ...), including optional ones.
pub fn known_keys() -> Vec<String> {
    let mut full = Config {
        default_device: Some(String::new()),
        ..Config::default()
    };
    full.llm.base_url = Some(String::new());
    full.ios.wda_url = Some(String::new());
    full.ios.pairing_dir = Some(PathBuf::new());
    full.android.adb_path = Some(PathBuf::new());
    let v = Value::try_from(&full).unwrap_or(Value::Table(Default::default()));
    let mut out = Vec::new();
    collect_keys(&v, "", &mut out);
    out
}

fn collect_keys(v: &Value, prefix: &str, out: &mut Vec<String>) {
    if let Value::Table(t) = v {
        for (k, child) in t {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            if child.is_table() {
                collect_keys(child, &key, out);
            } else {
                out.push(key);
            }
        }
    }
}

/// Return a copy of `cfg` with `key` set to `raw`. The value is parsed to the
/// key's type (bool, number, list = comma separated). `none` or an empty
/// string clears an optional key.
pub fn set_value(cfg: &Config, key: &str, raw: &str) -> Result<Config> {
    let keys = known_keys();
    if !keys.iter().any(|k| k == key) {
        let lower = key.to_ascii_lowercase();
        if lower.contains("api_key") && !lower.ends_with("_env") {
            bail!(
                "API keys are never stored in the config file. Set the environment variable instead (for example ANTHROPIC_API_KEY)"
            );
        }
        bail!("unknown key \"{key}\". Known keys: {}", keys.join(", "));
    }
    let mut root = Value::try_from(cfg)?;
    let parts: Vec<&str> = key.split('.').collect();
    let (last, parents) = parts.split_last().expect("key is not empty");
    let mut table = root.as_table_mut().expect("config is a table");
    for p in parents {
        table = table
            .entry(p.to_string())
            .or_insert_with(|| Value::Table(Default::default()))
            .as_table_mut()
            .with_context(|| format!("{p} is not a section"))?;
    }
    let clear = raw.is_empty() || raw.eq_ignore_ascii_case("none");
    let existing = table.get(*last).cloned();
    match existing {
        None if clear => {}
        None => {
            table.insert(last.to_string(), Value::String(raw.to_string()));
        }
        Some(old) => {
            let v = match old {
                Value::Boolean(_) => Value::Boolean(parse_bool(raw)?),
                Value::Integer(_) => Value::Integer(
                    raw.trim()
                        .parse()
                        .with_context(|| format!("{key} needs a whole number, not \"{raw}\""))?,
                ),
                Value::Float(_) => Value::Float(
                    raw.trim()
                        .parse()
                        .with_context(|| format!("{key} needs a number, not \"{raw}\""))?,
                ),
                // A list cannot be "none": go back to the default list.
                Value::Array(_) if clear => {
                    table.remove(*last);
                    return finish(root, key);
                }
                Value::Array(_) => Value::Array(
                    raw.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| Value::String(s.to_string()))
                        .collect(),
                ),
                _ if clear => {
                    table.remove(*last);
                    return finish(root, key);
                }
                _ => Value::String(raw.to_string()),
            };
            table.insert(last.to_string(), v);
        }
    }
    finish(root, key)
}

fn finish(root: Value, key: &str) -> Result<Config> {
    root.try_into::<Config>()
        .map_err(|e| anyhow::anyhow!("invalid value for {key}: {e}"))
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => bail!("use true or false, not \"{raw}\""),
    }
}

/// The value of `key` as TOML text (`none` when unset).
pub fn display_value(cfg: &Config, key: &str) -> Result<String> {
    let root = Value::try_from(cfg)?;
    let mut cur = &root;
    for p in key.split('.') {
        match cur.get(p) {
            Some(v) => cur = v,
            None => return Ok("none".into()),
        }
    }
    Ok(cur.to_string())
}

fn edit(path: &Path) -> Result<()> {
    if !path.exists() {
        save(&load(path)?, path)?;
    }
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("notepad");
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(path)
        .status()
        .with_context(|| format!("cannot start the editor \"{editor}\" (set VISUAL or EDITOR)"))?;
    if !status.success() {
        bail!("the editor exited with {status}");
    }
    match Config::load_from(path) {
        Ok(_) => ui::ok(format!("saved {}", path.display())),
        Err(e) => ui::warn(format!(
            "the file has an error and will not load: {e}. Run `agentphone config edit` again"
        )),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_include_optional_fields() {
        let k = known_keys();
        for want in [
            "default_device",
            "llm.model",
            "llm.base_url",
            "safety.max_steps",
            "android.adb_path",
            "ios.backend_order",
        ] {
            assert!(k.iter().any(|x| x == want), "{want} missing from {k:?}");
        }
    }

    #[test]
    fn set_typed_values() {
        let c = Config::default();
        let c = set_value(&c, "llm.model", "claude-sonnet-5-5").unwrap();
        assert_eq!(c.llm.model, "claude-sonnet-5-5");
        let c = set_value(&c, "safety.max_steps", "12").unwrap();
        assert_eq!(c.safety.max_steps, 12);
        let c = set_value(&c, "safety.confirm_irreversible", "no").unwrap();
        assert!(!c.safety.confirm_irreversible);
        let c = set_value(&c, "default_device", "00008110-XYZ").unwrap();
        assert_eq!(c.default_device.as_deref(), Some("00008110-XYZ"));
        let c = set_value(&c, "default_device", "none").unwrap();
        assert_eq!(c.default_device, None);
        let c = set_value(&c, "ios.backend_order", "hid, wda").unwrap();
        assert_eq!(c.ios.backend_order, vec!["hid", "wda"]);
        let c = set_value(&c, "ios.backend_order", "none").unwrap();
        assert_eq!(c.ios.backend_order, Config::default().ios.backend_order);
        let c = set_value(&c, "ios.backend_order", "wda").unwrap();
        let c = set_value(&c, "ios.backend_order", "").unwrap();
        assert_eq!(c.ios.backend_order, Config::default().ios.backend_order);
        let c = set_value(&c, "android.adb_path", "D:\\Android\\platform-tools").unwrap();
        assert!(c.android.adb_path.is_some());
        assert_eq!(display_value(&c, "safety.max_steps").unwrap(), "12");
        assert_eq!(display_value(&c, "llm.base_url").unwrap(), "none");
    }

    #[test]
    fn set_rejects_bad_values() {
        let c = Config::default();
        assert!(set_value(&c, "safety.max_steps", "many").is_err());
        assert!(set_value(&c, "safety.allow_unlock", "maybe").is_err());
        assert!(set_value(&c, "nope", "1").is_err());
        let e = set_value(&c, "llm.api_key", "sk-123").unwrap_err();
        assert!(e.to_string().contains("environment variable"));
        assert!(set_value(&c, "android.adb_port", "70000").is_err());
    }

    #[test]
    fn set_show_round_trip_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        run(
            Some(ConfigAction::Set {
                key: "llm.model".into(),
                value: "claude-haiku-5-5".into(),
            }),
            &path,
            false,
        )
        .unwrap();
        let c = load(&path).unwrap();
        assert_eq!(c.llm.model, "claude-haiku-5-5");
        assert_eq!(c.safety, ap_core::SafetyConfig::default());
    }
}
