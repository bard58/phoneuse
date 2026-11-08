//! Terminal output helpers: colours, icons, friendly errors.

use std::io::IsTerminal;

use console::style;

/// Print a green "ok" line.
pub fn ok(msg: impl std::fmt::Display) {
    println!("{} {msg}", style("✔").green().bold());
}

/// Print a yellow warning line to stderr.
pub fn warn(msg: impl std::fmt::Display) {
    eprintln!("{} {msg}", style("!").yellow().bold());
}

/// Print a command the user can copy.
pub fn command(cmd: impl std::fmt::Display) {
    println!("    {}", style(cmd).cyan());
}

/// True when both stdin and stdout are terminals (prompts can be shown).
pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// "Press Enter to close this window", then wait for Enter.
pub fn wait_for_enter() {
    eprintln!("\nPress Enter to close this window.");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// Print JSON (pretty) to stdout.
pub fn json(v: &impl serde::Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

/// The problem and the fix hint for any error, without Debug noise.
pub fn explain(err: &anyhow::Error) -> (String, Option<String>) {
    for cause in err.chain() {
        if let Some(e) = cause.downcast_ref::<ap_core::Error>() {
            return core_error(e);
        }
        if let Some(e) = cause.downcast_ref::<ap_llm::Error>() {
            return llm_error(e);
        }
    }
    let mut problem = err.to_string();
    for cause in err.chain().skip(1) {
        let c = cause.to_string();
        if !problem.contains(&c) {
            problem.push_str(": ");
            problem.push_str(&c);
        }
    }
    (problem, None)
}

fn core_error(e: &ap_core::Error) -> (String, Option<String>) {
    use ap_core::Error as E;
    match e {
        E::Setup { problem, hint } => (problem.clone(), Some(hint.clone())),
        E::DeviceNotFound(id) => (
            format!("no phone with id \"{id}\" is connected"),
            Some("run `agentphone devices` to see connected phones".into()),
        ),
        E::NotConnected(m) => (
            format!("the phone is not connected: {m}"),
            Some("check the cable, unlock the phone, then run `agentphone doctor`".into()),
        ),
        E::Unsupported(what) => (
            format!("this phone or backend cannot do this: {what}"),
            None,
        ),
        other => (other.to_string(), None),
    }
}

fn llm_error(e: &ap_llm::Error) -> (String, Option<String>) {
    use ap_llm::Error as E;
    match e {
        E::Config(m) if m.contains("API key") => (m.clone(), Some(api_key_fix_line())),
        E::Api { status: 401, .. } => (
            e.to_string(),
            Some(
                "the API key is wrong or expired: set a valid key, then open a new terminal".into(),
            ),
        ),
        other => (other.to_string(), None),
    }
}

fn api_key_fix_line() -> String {
    if cfg!(windows) {
        "run `setx ANTHROPIC_API_KEY \"your-key\"` (or the variable your provider uses), then open a new terminal".into()
    } else {
        "run `export ANTHROPIC_API_KEY=\"your-key\"` (or the variable your provider uses)".into()
    }
}

/// Print an error the friendly way: the problem, then "Fix: ...".
pub fn print_error(err: &anyhow::Error) {
    let (problem, hint) = explain(err);
    eprintln!("{} {problem}", style("Error:").red().bold());
    if let Some(h) = hint {
        eprintln!("  {} {h}", style("Fix:").cyan().bold());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_setup_errors() {
        let e = anyhow::Error::new(ap_core::Error::setup(
            "usbmuxd not running",
            "install Apple Devices",
        ))
        .context("cannot list devices");
        let (p, h) = explain(&e);
        assert_eq!(p, "usbmuxd not running");
        assert_eq!(h.as_deref(), Some("install Apple Devices"));
        let (p, h) = explain(&anyhow::anyhow!("plain"));
        assert_eq!((p.as_str(), h), ("plain", None));
        let (_, h) = explain(&anyhow::Error::new(ap_core::Error::DeviceNotFound(
            "x".into(),
        )));
        assert!(h.unwrap().contains("agentphone devices"));
    }
}
