//! `agentphone setup`: the step-by-step wizard.
//!
//! Plain words, one step at a time. Every step can be skipped. Nothing is
//! installed or registered without asking first. API keys are never stored.

use std::path::Path;

use anyhow::{Result, bail};
use ap_core::Config;
use console::style;
use dialoguer::{Confirm, Input, Select};

use crate::cli::SetupPart;
use crate::doctor::{Check, Status};
use crate::{config_cmd, devices, doctor, ios, ui};

/// Microsoft Store page of the Apple Devices app.
const APPLE_DEVICES_STORE: &str = "ms-windows-store://pdp/?productid=9NP83LWLPZ9K";

fn theme() -> dialoguer::theme::ColorfulTheme {
    dialoguer::theme::ColorfulTheme::default()
}

fn confirm(q: &str, default: bool) -> bool {
    Confirm::with_theme(&theme())
        .with_prompt(q)
        .default(default)
        .interact()
        .unwrap_or(false)
}

fn select(q: &str, items: &[&str], default: usize) -> Option<usize> {
    Select::with_theme(&theme())
        .with_prompt(q)
        .items(items)
        .default(default)
        .interact_opt()
        .ok()
        .flatten()
}

fn input(q: &str, default: Option<&str>) -> String {
    let t = theme();
    let mut i = Input::<String>::with_theme(&t)
        .with_prompt(q)
        .allow_empty(true);
    if let Some(d) = default {
        i = i.default(d.to_string());
    }
    i.interact_text().unwrap_or_default().trim().to_string()
}

/// Like [`input`], but `None` when the prompt fails (Ctrl+C, no terminal).
fn required_input(q: &str, default: Option<&str>) -> Option<String> {
    let t = theme();
    let mut i = Input::<String>::with_theme(&t)
        .with_prompt(q)
        .allow_empty(true);
    if let Some(d) = default {
        i = i.default(d.to_string());
    }
    i.interact_text().ok().map(|s| s.trim().to_string())
}

/// The model to suggest for an OpenAI-compatible provider: the current one,
/// unless it is a Claude id (an OpenAI-compatible server does not know it).
pub fn openai_model_default(current: &str) -> Option<String> {
    let m = current.trim();
    (!m.is_empty() && !m.starts_with("claude-")).then(|| m.to_string())
}

fn say(text: &str) {
    for line in text.lines() {
        println!("  {line}");
    }
}

fn step(n: &str) {
    println!("\n{} {}", style("▶").cyan().bold(), style(n).bold());
}

fn show_checks(checks: &[Check]) {
    for c in checks {
        let icon = match c.status {
            Status::Ok => style("✔").green().bold(),
            Status::Warn => style("!").yellow().bold(),
            Status::Fail => style("✘").red().bold(),
        };
        if c.detail.is_empty() {
            println!("  {icon} {}", c.name);
        } else {
            println!("  {icon} {}: {}", c.name, c.detail);
        }
    }
}

/// Ask "check again / skip". `true` = check again.
fn again_or_skip(what: &str) -> bool {
    matches!(
        select(
            "When you are ready:",
            &["I did it, check again", &format!("Skip {what} for now")],
            0
        ),
        Some(0)
    )
}

/// Run the wizard (all parts, or one).
pub async fn run(cfg_path: &Path, part: Option<SetupPart>, wanted: Option<&str>) -> Result<()> {
    if !ui::interactive() {
        bail!(
            "setup asks questions, so it needs an interactive terminal. Run `agentphone doctor` to see what is missing"
        );
    }
    let mut cfg = match config_cmd::load(cfg_path) {
        Ok(c) => c,
        Err(e) => {
            ui::warn(format!(
                "the config file has an error ({e}); starting from defaults"
            ));
            Config::default()
        }
    };

    match part {
        Some(SetupPart::Ios) => ios_flow(&mut cfg, wanted).await,
        Some(SetupPart::Android) => android_flow(&cfg).await,
        Some(SetupPart::Llm) => llm_flow(&mut cfg),
        Some(SetupPart::Mcp) => mcp_flow(cfg_path),
        Some(SetupPart::Ocr) => ocr_flow().await?,
        None => {
            println!(
                "{}\n",
                style("Welcome to agentphone. This takes about 5 minutes.").bold()
            );
            say("agentphone lets an AI agent see and use your phone from this computer.");
            say("You can stop and run `agentphone setup` again at any time.");
            step("Step 1: Your phone");
            let choice = select(
                "Which phone do you want to use?",
                &[
                    "iPhone (iOS 17.4 or newer)",
                    "Android phone",
                    "Both",
                    "Skip this step",
                ],
                0,
            );
            if matches!(choice, Some(0 | 2)) {
                ios_flow(&mut cfg, wanted).await;
            }
            if matches!(choice, Some(1 | 2)) {
                android_flow(&cfg).await;
            }
            choose_default(&mut cfg).await;
            step("Step 2: The AI model");
            llm_flow(&mut cfg);
        }
    }

    config_cmd::save(&cfg, cfg_path)?;
    ui::ok(format!("saved settings to {}", cfg_path.display()));

    if part.is_none() {
        step("Step 3: Claude Code (optional)");
        mcp_flow(cfg_path);
        if ap_vision::OCR_ENABLED {
            step("Step 4: Text recognition (optional)");
            if confirm(
                "Download the text recognition models now (about 12 MB)?",
                true,
            ) {
                ocr_flow().await?;
            }
        }
        step("All set");
        say("Try these:");
        ui::command("agentphone doctor");
        ui::command("agentphone screenshot -o screen.png");
        ui::command("agentphone run \"open Settings and turn on Wi-Fi\"");
    }
    Ok(())
}

// ---------------------------------------------------------------- iPhone

/// Plain instructions for an iPhone check id.
pub fn ios_instructions(id: &str) -> Option<&'static str> {
    Some(match id {
        "usbmuxd" if cfg!(windows) => {
            "Windows needs Apple's USB driver.\n\
             1. Install the free \"Apple Devices\" app from the Microsoft Store (iTunes also works).\n\
             2. Open it once, then close it."
        }
        "usbmuxd" => {
            "This computer needs the usbmuxd service.\n\
             Linux: install it (for example `sudo apt install usbmuxd`) and plug in the iPhone."
        }
        "device" => {
            "1. Connect the iPhone to this computer with a USB cable (a data cable, like the one in the box).\n\
             2. Unlock the iPhone."
        }
        "paired" => {
            "1. Unlock the iPhone.\n\
             2. The iPhone asks \"Trust This Computer?\". Tap Trust and enter your passcode.\n\
             3. No question shown? Unplug the cable and plug it in again."
        }
        "ios_version" => {
            "agentphone needs iOS 17.4 or newer.\n\
             On the iPhone open Settings > General > Software Update and install the update."
        }
        "developer_mode" => {
            "Turn on Developer Mode (one time):\n\
             1. On the iPhone open Settings > Privacy & Security.\n\
             2. Scroll to the bottom and tap Developer Mode. Turn it on.\n\
             3. Tap Restart. The iPhone restarts.\n\
             4. After the restart, unlock it, tap Turn On, and enter your passcode.\n\
             Do not see Developer Mode? It shows up after the iPhone trusts this computer:\n\
             unplug, plug in again, and look again."
        }
        "ddi" => {
            "The iPhone needs Apple's developer disk image to accept touches from this computer.\n\
             agentphone downloads it from Apple and installs it on the iPhone (about 20 MB).\n\
             You do this again after each restart of the iPhone."
        }
        _ => return None,
    })
}

/// The iPhone to set up: `--device`, else the saved default iPhone.
async fn ios_target(
    cfg: &Config,
    wanted: Option<&str>,
) -> (Option<String>, Vec<ap_core::DeviceInfo>) {
    let found = devices::discover(&[ios::provider(cfg)]).await.devices;
    (ios::target(cfg, wanted, &found), found)
}

async fn ios_flow(cfg: &mut Config, wanted: Option<&str>) {
    step("iPhone");
    let mut chosen;
    loop {
        let (udid, found) = ios_target(cfg, wanted).await;
        chosen = udid;
        let wanted = chosen.as_deref();
        let checks = ios::checks(cfg, wanted, &found).await;
        show_checks(&checks);
        let Some(bad) = checks.iter().find(|c| c.status == Status::Fail) else {
            if checks.iter().any(|c| c.status == Status::Warn) {
                ui::warn("some iPhone checks have warnings; see above");
            } else {
                ui::ok("the iPhone is ready");
            }
            break;
        };
        println!();
        // Several iPhones: the generic "connect by USB" text is wrong here.
        let several = bad.id == "device" && wanted.is_none() && found.len() > 1;
        match ios_instructions(&bad.id).filter(|_| !several) {
            Some(t) => say(t),
            None => say(&format!(
                "{}: {}",
                bad.name,
                bad.fix.as_deref().unwrap_or(&bad.detail)
            )),
        }
        match bad.id.as_str() {
            "usbmuxd" if cfg!(windows) => {
                if confirm("Open the Apple Devices page in the Microsoft Store?", true) {
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", "", APPLE_DEVICES_STORE])
                        .status();
                }
            }
            "paired" => {
                if confirm("Ask the iPhone to trust this computer now?", true) {
                    say("Look at the iPhone and tap Trust…");
                    match ios::pair(wanted).await {
                        Ok(m) => {
                            ui::ok(m);
                            continue;
                        }
                        Err(e) => ui::print_error(&e.into()),
                    }
                }
            }
            "developer_mode" => {
                if confirm("Show the Developer Mode switch on the iPhone now?", true) {
                    match ios::reveal_developer_mode(wanted).await {
                        Ok(_) => ui::ok("done: now follow the steps above on the iPhone"),
                        Err(e) => ui::print_error(&e.into()),
                    }
                }
            }
            "ddi" => {
                let mounted =
                    confirm("Download and install it now?", true) && mount_ddi(wanted).await;
                if mounted {
                    continue;
                }
            }
            _ => {}
        }
        if !again_or_skip("the iPhone") {
            return;
        }
    }
    if confirm(
        "Also use the iPhone over Wi-Fi when the cable is unplugged? (same network)",
        false,
    ) {
        match ios::save_wifi_pairing(cfg, chosen.as_deref()).await {
            Ok(m) => ui::ok(m),

            Err(e) => ui::print_error(&e.into()),
        }
    }
}

async fn mount_ddi(wanted: Option<&str>) -> bool {
    let bar = indicatif::ProgressBar::new(0);
    bar.set_style(
        indicatif::ProgressStyle::with_template(
            "  {spinner} {msg} [{bar:30}] {bytes}/{total_bytes}",
        )
        .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
        .progress_chars("=> "),
    );
    bar.set_message("developer disk image");
    let b = bar.clone();
    let progress: ios::DdiProgress = std::sync::Arc::new(move |phase, done, total| {
        b.set_message(phase.to_string());
        if let Some(t) = total {
            b.set_length(t);
        }
        b.set_position(done);
    });
    let r = ios::mount_ddi(wanted, progress).await;
    bar.finish_and_clear();
    match r {
        Ok(()) => {
            ui::ok("developer disk image installed");
            true
        }
        Err(e) => {
            ui::print_error(&e.into());
            false
        }
    }
}

// ---------------------------------------------------------------- Android

async fn android_flow(cfg: &Config) {
    step("Android phone");
    loop {
        if let Err(e) = ap_android::locate_adb(&cfg.android) {
            say("This computer needs Google's free Android tools (platform-tools).");
            if cfg!(windows) {
                say("Open a terminal and run:");
                ui::command("winget install Google.PlatformTools");
                say("Then come back here.");
            } else {
                say(e.hint().unwrap_or(ap_android::locate::INSTALL_HINT));
            }
            if !again_or_skip("Android") {
                return;
            }
            continue;
        }
        let provider = ap_android::AndroidProvider::new(cfg.android.clone());
        let checks: Vec<Check> = doctor::android_checks(cfg).await;
        show_checks(&checks);
        let entries = provider.entries().await.unwrap_or_default();
        use ap_android::DeviceState as S;
        if entries.iter().any(|e| e.state == S::Device) {
            ui::ok("the Android phone is ready");
            break;
        }
        println!();
        if entries
            .iter()
            .any(|e| matches!(e.state, S::Unauthorized | S::Authorizing))
        {
            say("1. Unlock the phone.\n\
                 2. It asks \"Allow USB debugging?\". Tick \"Always allow from this computer\" and tap Allow.\n\
                 3. No question shown? Unplug the cable and plug it in again.");
        } else {
            say("Turn on USB debugging (one time):\n\
                 1. On the phone open Settings > About phone.\n\
                 2. Tap \"Build number\" 7 times. (Samsung: Software information > Build number.)\n\
                 3. Go back to Settings > System > Developer options and turn on USB debugging.\n\
                 4. Connect the phone with a USB cable and unlock it.");
        }
        match select(
            "When you are ready:",
            &[
                "I did it, check again",
                "Connect over Wi-Fi instead",
                "Skip Android for now",
            ],
            0,
        ) {
            Some(0) => continue,
            Some(1) => {
                if android_wifi(&provider).await {
                    break;
                }
            }
            _ => return,
        }
    }
    if confirm(
        "Also use this phone over Wi-Fi (Android 11 or newer, same network)?",
        false,
    ) {
        android_wifi(&ap_android::AndroidProvider::new(cfg.android.clone())).await;
    }
}

/// adb pair + connect. `true` when connected.
async fn android_wifi(p: &ap_android::AndroidProvider) -> bool {
    say(
        "On the phone open Developer options > Wireless debugging and turn it on.\n\
         Tap \"Pair device with pairing code\". The phone shows an address and a 6-digit code.",
    );
    let addr = input("Pairing address (IP:port shown under the code)", None);
    let code = input("6-digit pairing code", None);
    if addr.is_empty() || code.is_empty() {
        ui::warn("skipped Wi-Fi pairing");
        return false;
    }
    match p.pair(&addr, &code).await {
        Ok(m) => ui::ok(m.trim()),
        Err(e) => {
            ui::print_error(&e.into());
            return false;
        }
    }
    say("Now look at the Wireless debugging screen: it shows \"IP address & Port\".");
    let target = input("IP address & port", None);
    if target.is_empty() {
        return false;
    }
    match p.connect_wifi(&target).await {
        Ok(m) => {
            ui::ok(m);
            true
        }
        Err(e) => {
            ui::print_error(&e.into());
            false
        }
    }
}

// ---------------------------------------------------------------- Default phone

async fn choose_default(cfg: &mut Config) {
    let found = devices::discover(&devices::providers(cfg)).await;
    match found.devices.as_slice() {
        [] => ui::warn("no phone is connected right now; you can finish setup and connect later"),
        [one] => {
            ui::ok(format!("found {} ({})", one.name, one.platform));
            if let Some(saved) = cfg.default_device.clone().filter(|s| *s != one.id) {
                // The saved phone may only be unplugged now: keep it unless asked.
                if confirm(
                    &format!(
                        "The default phone {saved} is not connected now. Use {} by default instead?",
                        one.name
                    ),
                    false,
                ) {
                    cfg.default_device = Some(one.id.clone());
                } else {
                    ui::ok(format!("{saved} stays the default phone"));
                }
            }
        }
        many => {
            let labels: Vec<String> = many
                .iter()
                .map(|d| format!("{} ({} {}, {})", d.name, d.platform, d.os_version, d.id))
                .collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            if let Some(i) = select("Which phone should agentphone use by default?", &refs, 0) {
                cfg.default_device = Some(many[i].id.clone());
            }
        }
    }
}

// ---------------------------------------------------------------- AI model

fn llm_flow(cfg: &mut Config) {
    say("`agentphone run` uses an AI model to decide what to tap.");
    say("Claude Code and other MCP clients use their own model, so you can skip this.");
    let choice = select(
        "Which AI provider?",
        &[
            "Anthropic Claude (recommended)",
            "OpenAI or OpenAI-compatible (OpenRouter, Ollama, LM Studio)",
            "Skip",
        ],
        if cfg.llm.provider.eq_ignore_ascii_case("openai") {
            1
        } else {
            0
        },
    );
    match choice {
        Some(0) => {
            cfg.llm.provider = "anthropic".into();
            cfg.llm.base_url = None;
            let models = [
                (
                    ap_llm::models::CLAUDE_OPUS_5_5,
                    "Claude Opus 5.5: best results (default)",
                ),
                (
                    ap_llm::models::CLAUDE_SONNET_5_5,
                    "Claude Sonnet 5.5: faster, cheaper",
                ),
                (
                    ap_llm::models::CLAUDE_HAIKU_5_5,
                    "Claude Haiku 5.5: fastest, cheapest",
                ),
            ];
            let labels: Vec<&str> = models.iter().map(|m| m.1).collect();
            let cur = models
                .iter()
                .position(|m| m.0 == cfg.llm.model)
                .unwrap_or(0);
            if let Some(i) = select("Which model?", &labels, cur) {
                cfg.llm.model = models[i].0.into();
            }
        }
        Some(1) => {
            let before = cfg.clone();
            cfg.llm.provider = "openai".into();
            let base = input(
                "API address",
                Some(
                    cfg.llm
                        .base_url
                        .as_deref()
                        .unwrap_or(ap_llm::openai::DEFAULT_BASE_URL),
                ),
            );
            cfg.llm.base_url =
                (!base.is_empty() && base != ap_llm::openai::DEFAULT_BASE_URL).then_some(base);
            let cur = openai_model_default(&cfg.llm.model);
            // A Claude model id does not work with an OpenAI-compatible server,
            // so the model name is required here.
            loop {
                match required_input("Model name (must accept images)", cur.as_deref()) {
                    Some(m) if !m.is_empty() => {
                        cfg.llm.model = m;
                        break;
                    }
                    Some(_) => ui::warn(
                        "enter the model name of your provider, for example the name shown by `ollama list`",
                    ),
                    None => {
                        ui::warn("no model name given; run `agentphone setup llm` again later");
                        *cfg = before;
                        return;
                    }
                }
            }
        }
        _ => return,
    }
    explain_api_key(cfg);
}

fn explain_api_key(cfg: &Config) {
    let var = cfg.llm.api_key_env().to_string();
    let local = cfg
        .llm
        .base_url
        .as_deref()
        .is_some_and(ap_llm::openai::is_local);
    if cfg.llm.api_key().is_ok() {
        ui::ok(format!(
            "found your API key in the environment variable {var}"
        ));
        return;
    }
    if local {
        ui::ok("a local server needs no API key");
        return;
    }
    say(&format!(
        "\nagentphone reads the API key from the environment variable {var}.\n\
         The key is never saved in the agentphone config file."
    ));
    if cfg!(windows) {
        say("Run this in a terminal (paste your key between the quotes):");
        ui::command(format!("setx {var} \"paste-your-key-here\""));
        say("Then close this terminal and open a new one, so it sees the key.");
    } else {
        say("Add this line to your shell profile (~/.bashrc, ~/.zshrc):");
        ui::command(format!("export {var}=\"paste-your-key-here\""));
    }
    if var == "ANTHROPIC_API_KEY" {
        say("Get a key at https://console.anthropic.com/settings/keys");
    }
}

// ---------------------------------------------------------------- Claude Code

/// The `claude mcp add` arguments for this executable.
pub fn mcp_add_args(exe: &Path, cfg_path: Option<&Path>) -> Vec<String> {
    let mut a = vec![
        "mcp".to_string(),
        "add".into(),
        "--scope".into(),
        "user".into(),
    ];
    if let Some(p) = cfg_path {
        a.push("-e".into());
        a.push(format!("AGENTPHONE_CONFIG={}", p.display()));
    }
    a.extend([
        "agentphone".into(),
        "--".into(),
        exe.display().to_string(),
        "mcp".into(),
    ]);
    a
}

fn mcp_flow(cfg_path: &Path) {
    let exe = std::env::current_exe().unwrap_or_else(|_| "agentphone".into());
    let custom = Config::default_path()
        .ok()
        .filter(|d| d != cfg_path)
        .map(|_| cfg_path);
    let args = mcp_add_args(&exe, custom);
    let shown = format!(
        "claude {}",
        args.iter()
            .map(|a| if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.clone()
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    say("Claude Code can use your phone through agentphone (MCP server).");
    let Ok(claude) = which::which("claude") else {
        say("Claude Code is not installed here. After you install it, run:");
        ui::command(&shown);
        return;
    };
    say("This runs:");
    ui::command(&shown);
    if !confirm("Add agentphone to Claude Code now?", true) {
        return;
    }
    match std::process::Command::new(claude).args(&args).output() {
        Ok(out) if out.status.success() => {
            ui::ok("added. Start a new Claude Code session to use it")
        }
        Ok(out) => {
            let msg = String::from_utf8_lossy(&out.stderr);
            let msg2 = String::from_utf8_lossy(&out.stdout);
            let text = format!("{} {}", msg.trim(), msg2.trim());
            if text.contains("already exists") {
                ui::ok("agentphone is already in Claude Code");
            } else {
                ui::warn(format!("claude mcp add failed: {}", text.trim()));
            }
        }
        Err(e) => ui::warn(format!("cannot run claude: {e}")),
    }
}

// ---------------------------------------------------------------- OCR

#[cfg(feature = "ocr")]
async fn ocr_flow() -> Result<()> {
    use ap_vision::models;
    let dir = models::default_model_dir()?;
    let list = models::default_models();
    let bar = indicatif::ProgressBar::new(0);
    bar.set_style(
        indicatif::ProgressStyle::with_template("  {msg} [{bar:30}] {bytes}/{total_bytes}")
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    models::download_models(&dir, &list, |p| {
        bar.set_message(p.file.clone());
        if let Some(t) = p.total {
            bar.set_length(t);
        }
        bar.set_position(p.downloaded);
    })
    .await?;
    bar.finish_and_clear();
    ui::ok(format!("text recognition models are in {}", dir.display()));
    Ok(())
}

#[cfg(not(feature = "ocr"))]
async fn ocr_flow() -> Result<()> {
    Err(ap_vision::ocr_unavailable().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_cover_ios_steps() {
        for id in [
            "usbmuxd",
            "device",
            "paired",
            "ios_version",
            "developer_mode",
            "ddi",
        ] {
            assert!(ios_instructions(id).is_some(), "{id}");
        }
        assert!(
            ios_instructions("developer_mode")
                .unwrap()
                .contains("Restart")
        );
        assert!(ios_instructions("x").is_none());
    }

    #[test]
    fn openai_model_never_keeps_a_claude_id() {
        assert_eq!(openai_model_default(ap_llm::models::CLAUDE_OPUS_5_5), None);
        assert_eq!(openai_model_default(""), None);
        assert_eq!(openai_model_default("llava").as_deref(), Some("llava"));
    }

    #[test]
    fn mcp_command() {
        let a = mcp_add_args(Path::new("C:\\Tools\\agentphone.exe"), None);
        assert_eq!(
            a.join(" "),
            "mcp add --scope user agentphone -- C:\\Tools\\agentphone.exe mcp"
        );
        let a = mcp_add_args(Path::new("ap"), Some(Path::new("c.toml")));
        assert!(a.contains(&"AGENTPHONE_CONFIG=c.toml".to_string()));
    }
}
