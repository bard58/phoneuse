//! `agentphone doctor`: every setup check with an ok / warn / fail icon and one fix line.

use std::path::Path;

use ap_core::Config;
use console::style;
use serde::Serialize;

use crate::{devices, ios};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

/// One check result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    /// Section: "Config", "AI model", "Android", "iPhone", "Phones", "OCR".
    pub group: &'static str,
    /// Stable id (the wizard uses it to pick instructions).
    pub id: String,
    pub name: String,
    pub status: Status,
    pub detail: String,
    /// One line: what to do. Shown for warn and fail.
    pub fix: Option<String>,
}

impl Check {
    pub fn new(
        group: &'static str,
        id: &str,
        name: &str,
        status: Status,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            group,
            id: id.into(),
            name: name.into(),
            status,
            detail: detail.into(),
            fix: None,
        }
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// True when any check failed (doctor exits non-zero).
pub fn any_failed(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.status == Status::Fail)
}

fn icon(s: Status, color: bool) -> String {
    let (txt, styled) = match s {
        Status::Ok => ("✔", style("✔").green().bold()),
        Status::Warn => ("!", style("!").yellow().bold()),
        Status::Fail => ("✘", style("✘").red().bold()),
    };
    if color {
        styled.force_styling(true).to_string()
    } else {
        txt.to_string()
    }
}

/// Human report, grouped by section. `color` = ANSI colours.
pub fn render(checks: &[Check], color: bool) -> String {
    let mut out = String::new();
    let mut group = "";
    for c in checks {
        if c.group != group {
            group = c.group;
            if !out.is_empty() {
                out.push('\n');
            }
            if color {
                out.push_str(&style(group).bold().force_styling(true).to_string());
            } else {
                out.push_str(group);
            }
            out.push('\n');
        }
        out.push_str(&format!("  {} {}", icon(c.status, color), c.name));
        if !c.detail.is_empty() {
            out.push_str(&format!(": {}", c.detail));
        }
        out.push('\n');
        if c.status != Status::Ok
            && let Some(f) = &c.fix
        {
            let label = if color {
                style("Fix:").cyan().force_styling(true).to_string()
            } else {
                "Fix:".to_string()
            };
            out.push_str(&format!("      {label} {f}\n"));
        }
    }
    let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
    out.push('\n');
    out.push_str(&match (fails, warns) {
        (0, 0) => "Everything looks good.".to_string(),
        (0, w) => format!("Ready, with {w} warning(s)."),
        (f, _) => format!("{f} problem(s) to fix. Run `agentphone setup` for step-by-step help."),
    });
    out.push('\n');
    out
}

/// Config file and AI provider checks.
pub fn config_checks(cfg_path: &Path, cfg: Result<&Config, &anyhow::Error>) -> Vec<Check> {
    let mut v = Vec::new();
    match cfg {
        Err(e) => v.push(
            Check::new("Config", "config", "Config file", Status::Fail, e.to_string())
                .with_fix("run `agentphone config edit` to fix it, or delete the file and run `agentphone setup`"),
        ),
        Ok(_) if !cfg_path.exists() => v.push(
            Check::new(
                "Config",
                "config",
                "Config file",
                Status::Warn,
                format!("{} not found, using defaults", cfg_path.display()),
            )
            .with_fix("run `agentphone setup`"),
        ),
        Ok(_) => v.push(Check::new(
            "Config",
            "config",
            "Config file",
            Status::Ok,
            cfg_path.display().to_string(),
        )),
    }
    if let Ok(cfg) = cfg {
        let llm = &cfg.llm;
        // Same rule as `ap_llm::provider_from_config`: any non-empty name;
        // every name but Anthropic's uses the OpenAI wire format.
        if llm.provider.trim().is_empty() {
            v.push(
                Check::new(
                    "AI model",
                    "llm_provider",
                    "Provider",
                    Status::Fail,
                    "no provider is set".to_string(),
                )
                .with_fix("run `agentphone config set llm.provider anthropic` (or openai)"),
            );
        } else if !llm.is_anthropic() && llm.model.starts_with("claude-") {
            // An OpenAI-compatible server does not know Anthropic model ids.
            v.push(
                Check::new(
                    "AI model",
                    "llm_provider",
                    "Provider",
                    Status::Warn,
                    format!(
                        "{} / {}: this is a Claude model id, but the provider is not Anthropic",
                        llm.provider, llm.model
                    ),
                )
                .with_fix("run `agentphone setup llm` and enter the model name of your provider"),
            );
        } else {
            v.push(Check::new(
                "AI model",
                "llm_provider",
                "Provider",
                Status::Ok,
                format!("{} / {}", llm.provider, llm.model),
            ));
        }
        let var = llm.api_key_env();
        if std::env::var(var).is_ok_and(|k| !k.trim().is_empty()) {
            v.push(Check::new(
                "AI model",
                "llm_key",
                "API key",
                Status::Ok,
                format!("{var} is set"),
            ));
        } else if !ap_llm::key_required(llm) {
            v.push(Check::new(
                "AI model",
                "llm_key",
                "API key",
                Status::Ok,
                "local server, no key needed",
            ));
        } else {
            // Only `agentphone run` needs a key; MCP clients bring their own model.
            v.push(
                Check::new(
                    "AI model",
                    "llm_key",
                    "API key",
                    Status::Warn,
                    format!("{var} is not set (needed for `agentphone run`, not for MCP)"),
                )
                .with_fix(set_env_command(var, "your-key")),
            );
        }
    }
    v
}

/// The command that sets an environment variable for new terminals.
pub fn set_env_command(var: &str, value: &str) -> String {
    if cfg!(windows) {
        format!("run `setx {var} \"{value}\"`, then open a new terminal")
    } else {
        format!("add `export {var}=\"{value}\"` to your shell profile, then open a new terminal")
    }
}

/// Android (adb) checks.
pub async fn android_checks(cfg: &Config) -> Vec<Check> {
    let mut v = Vec::new();
    let g = "Android";
    match ap_android::locate_adb(&cfg.android) {
        Ok(p) => v.push(Check::new(
            g,
            "adb",
            "adb (platform-tools)",
            Status::Ok,
            p.display().to_string(),
        )),
        Err(e) => {
            // Not fatal: the user may only have an iPhone.
            v.push(
                Check::new(
                    g,
                    "adb",
                    "adb (platform-tools)",
                    Status::Warn,
                    "not found (only needed for Android)",
                )
                .with_fix(
                    e.hint()
                        .unwrap_or(ap_android::locate::INSTALL_HINT)
                        .to_string(),
                ),
            );
            return v;
        }
    }
    let p = ap_android::AndroidProvider::new(cfg.android.clone());
    match p.entries().await {
        Err(e) => v.push(
            Check::new(g, "adb_server", "adb server", Status::Fail, e.to_string())
                .with_fix(e.hint().unwrap_or("run `adb start-server`").to_string()),
        ),
        Ok(entries) => {
            v.push(Check::new(
                g,
                "adb_server",
                "adb server",
                Status::Ok,
                "running",
            ));
            for e in entries {
                use ap_android::DeviceState as S;
                let name = format!("Phone {}", e.serial);
                let model = e.model.clone().unwrap_or_default().replace('_', " ");
                let c = match e.state {
                    S::Device => Check::new(g, "android_device", &name, Status::Ok, format!("{model} ready")),
                    S::Unauthorized | S::Authorizing => {
                        Check::new(g, "android_unauthorized", &name, Status::Fail, "not allowed yet")
                            .with_fix(ap_android::provider::UNAUTHORIZED_HINT)
                    }
                    S::Offline => Check::new(g, "android_offline", &name, Status::Fail, "offline")
                        .with_fix("unlock the phone and replug the cable, or run `adb reconnect`"),
                    S::NoPermissions => {
                        Check::new(g, "android_permissions", &name, Status::Fail, "no USB permission")
                            .with_fix("install the Google USB driver (Windows) or fix udev rules (Linux), then replug")
                    }
                    S::Other(s) => Check::new(g, "android_other", &name, Status::Warn, format!("state '{s}'"))
                        .with_fix("boot the phone normally and enable USB debugging"),
                };
                v.push(c);
            }
        }
    }
    v
}

/// The install command for a build with OCR (the same as in docs/mcp.md).
pub const OCR_INSTALL: &str = "cargo install --git https://github.com/bard58/phoneuse --branch rust-rewrite ap-cli --features ocr";

/// OCR (tap-on) check.
pub fn ocr_checks() -> Vec<Check> {
    let g = "Text recognition";
    if !ap_vision::OCR_ENABLED {
        return vec![
            Check::new(
                g,
                "ocr",
                "OCR for tap-on",
                Status::Warn,
                "not in this build (optional)",
            )
            .with_fix(format!("install a build with OCR: `{OCR_INSTALL}`")),
        ];
    }
    match ap_vision::default_recognizer() {
        Ok(r) => vec![Check::new(
            g,
            "ocr",
            "OCR for tap-on",
            Status::Ok,
            r.name().to_string(),
        )],
        Err(e) => vec![
            Check::new(g, "ocr", "OCR for tap-on", Status::Warn, e.to_string())
                .with_fix(e.hint().unwrap_or("run `agentphone setup ocr`").to_string()),
        ],
    }
}

/// Run every check.
pub async fn gather(cfg_path: &Path, wanted: Option<&str>) -> Vec<Check> {
    gather_with_devices(cfg_path, wanted).await.0
}

/// Run every check. Also return the phones that discovery found, so that a
/// caller can remove their ids and names from a report.
pub async fn gather_with_devices(
    cfg_path: &Path,
    wanted: Option<&str>,
) -> (Vec<Check>, Vec<ap_core::DeviceInfo>) {
    let loaded = crate::config_cmd::load(cfg_path);
    let mut checks = config_checks(cfg_path, loaded.as_ref());
    let cfg = loaded.unwrap_or_default();
    let found = devices::discover(&devices::providers(&cfg)).await;
    let iphone = ios::target(&cfg, wanted, &found.devices);
    checks.extend(ios::checks(&cfg, iphone.as_deref(), &found.devices).await);
    checks.extend(android_checks(&cfg).await);
    let g = "Phones";
    if found.devices.is_empty() {
        checks.push(
            Check::new(g, "devices", "Connected phones", Status::Fail, "none found")
                .with_fix("plug in the phone, unlock it, and fix the problems above"),
        );
    } else {
        for d in &found.devices {
            checks.push(Check::new(
                g,
                "device",
                &d.name,
                Status::Ok,
                format!(
                    "{} {} via {} ({})",
                    d.platform, d.os_version, d.backend, d.id
                ),
            ));
        }
        let default = wanted.or(cfg.default_device.as_deref());
        if let Some(id) = default
            && !found.devices.iter().any(|d| d.id == id)
        {
            checks.push(
                Check::new(
                    g,
                    "default_device",
                    "Default phone",
                    Status::Warn,
                    format!("{id} is not connected"),
                )
                .with_fix("connect it, or choose another with `agentphone use <id>`"),
            );
        } else if default.is_none() && found.devices.len() > 1 {
            checks.push(
                Check::new(
                    g,
                    "default_device",
                    "Default phone",
                    Status::Warn,
                    "several phones, none chosen",
                )
                .with_fix("run `agentphone use <id>`"),
            );
        }
    }
    checks.extend(ocr_checks());
    relax_unused_platforms(&mut checks, &found.devices);
    (checks, found.devices)
}

/// When a phone works, problems on the other platform are only warnings
/// (an Android user does not need the Apple driver, and the other way round).
pub fn relax_unused_platforms(checks: &mut [Check], found: &[ap_core::DeviceInfo]) {
    if found.is_empty() {
        return;
    }
    for (group, platform) in [
        ("iPhone", ap_core::Platform::Ios),
        ("Android", ap_core::Platform::Android),
    ] {
        if found.iter().any(|d| d.platform == platform) {
            continue;
        }
        for c in checks
            .iter_mut()
            .filter(|c| c.group == group && c.status == Status::Fail)
        {
            c.status = Status::Warn;
            c.detail.push_str(&format!(" (only needed for {platform})"));
        }
    }
}

/// `agentphone doctor`. Returns false when a check failed.
pub async fn run(cfg_path: &Path, wanted: Option<&str>, json: bool) -> anyhow::Result<bool> {
    let checks = gather(cfg_path, wanted).await;
    if json {
        crate::ui::json(&serde_json::json!({
            "ok": !any_failed(&checks),
            "checks": checks,
        }))?;
    } else {
        print!("{}", render(&checks, console::colors_enabled()));
    }
    Ok(!any_failed(&checks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Check> {
        vec![
            Check::new(
                "Config",
                "config",
                "Config file",
                Status::Ok,
                "C:\\x\\config.toml",
            ),
            Check::new(
                "iPhone",
                "usbmuxd",
                "Apple Mobile Device service",
                Status::Fail,
                "not running",
            )
            .with_fix("install the Apple Devices app"),
            Check::new(
                "iPhone",
                "developer_mode",
                "Developer Mode",
                Status::Warn,
                "unknown",
            )
            .with_fix("Settings > Privacy & Security > Developer Mode"),
            Check::new("Android", "adb", "adb", Status::Ok, "found").with_fix("never shown"),
        ]
    }

    #[test]
    fn render_plain() {
        let out = render(&sample(), false);
        let expected = "\
Config
  ✔ Config file: C:\\x\\config.toml

iPhone
  ✘ Apple Mobile Device service: not running
      Fix: install the Apple Devices app
  ! Developer Mode: unknown
      Fix: Settings > Privacy & Security > Developer Mode

Android
  ✔ adb: found

1 problem(s) to fix. Run `agentphone setup` for step-by-step help.
";
        assert_eq!(out, expected);
        assert!(!out.contains("never shown"));
        assert!(any_failed(&sample()));
    }

    #[test]
    fn render_summaries_and_color() {
        let ok = vec![Check::new("Config", "c", "Config file", Status::Ok, "")];
        assert!(render(&ok, false).ends_with("Everything looks good.\n"));
        assert!(!any_failed(&ok));
        let warn = vec![Check::new("Config", "c", "Config file", Status::Warn, "")];
        assert!(render(&warn, false).contains("Ready, with 1 warning(s)."));
        let colored = render(&sample(), true);
        assert!(colored.contains("\u{1b}["));
    }

    #[test]
    fn unused_platform_is_a_warning() {
        let mut checks = sample();
        relax_unused_platforms(&mut checks, &[]);
        assert!(any_failed(&checks));
        let android = ap_core::DeviceInfo {
            id: "R5CX".into(),
            name: "Pixel".into(),
            platform: ap_core::Platform::Android,
            os_version: "15".into(),
            model: "Pixel 8".into(),
            screen_points: Default::default(),
            screen_pixels: Default::default(),
            backend: "adb".into(),
        };
        relax_unused_platforms(&mut checks, &[android]);
        assert!(!any_failed(&checks));
        assert!(checks[1].detail.ends_with("(only needed for iOS)"));
    }

    #[test]
    fn json_shape() {
        let v = serde_json::to_value(&sample()[1]).unwrap();
        assert_eq!(v["status"], "fail");
        assert_eq!(v["fix"], "install the Apple Devices app");
    }

    #[test]
    fn config_checks_missing_key() {
        let mut cfg = Config::default();
        cfg.llm.anthropic_api_key_env = "AGENTPHONE_TEST_SURELY_UNSET_KEY".into();
        let checks = config_checks(Path::new("Z:\\does\\not\\exist.toml"), Ok(&cfg));
        assert_eq!(checks[0].status, Status::Warn);
        let key = checks.iter().find(|c| c.id == "llm_key").unwrap();
        assert_eq!(key.status, Status::Warn);
        assert!(
            key.fix
                .as_ref()
                .unwrap()
                .contains("AGENTPHONE_TEST_SURELY_UNSET_KEY")
        );
        // Same rule as `ap_llm::provider_from_config`: only an empty name fails.
        cfg.llm.provider = "  ".into();
        let checks = config_checks(Path::new("x"), Ok(&cfg));
        assert!(any_failed(&checks));
        cfg.llm.provider = "groq".into();
        let checks = config_checks(Path::new("x"), Ok(&cfg));
        let p = checks.iter().find(|c| c.id == "llm_provider").unwrap();
        assert_ne!(p.status, Status::Fail, "{p:?}");
    }

    #[test]
    fn local_server_needs_no_key() {
        let mut cfg = Config::default();
        cfg.llm.provider = "openai".into();
        cfg.llm.model = "llava".into();
        cfg.llm.openai_api_key_env = "AGENTPHONE_TEST_SURELY_UNSET_KEY".into();
        cfg.llm.base_url = Some("http://localhost:11434/v1".into());
        let checks = config_checks(Path::new("x"), Ok(&cfg));
        let key = checks.iter().find(|c| c.id == "llm_key").unwrap();
        assert_eq!(key.status, Status::Ok, "{key:?}");
        cfg.llm.base_url = None;
        let checks = config_checks(Path::new("x"), Ok(&cfg));
        let key = checks.iter().find(|c| c.id == "llm_key").unwrap();
        assert_eq!(key.status, Status::Warn);
    }

    #[test]
    fn claude_model_with_openai_is_a_warning() {
        let mut cfg = Config::default();
        cfg.llm.provider = "openai".into();
        let checks = config_checks(Path::new("x"), Ok(&cfg));
        let p = checks.iter().find(|c| c.id == "llm_provider").unwrap();
        assert_eq!(p.status, Status::Warn);
        assert!(p.fix.as_deref().unwrap().contains("setup llm"));
    }

    #[test]
    fn ocr_fix_works_without_a_checkout() {
        assert!(OCR_INSTALL.contains("--git") && !OCR_INSTALL.contains("--path"));
        if !ap_vision::OCR_ENABLED {
            let c = &ocr_checks()[0];
            assert!(c.fix.as_deref().unwrap().contains(OCR_INSTALL));
        }
    }
}
