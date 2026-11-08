//! `agentphone`: control iPhones and Android phones from Windows, macOS and Linux.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "agentphone", version, about = "Let an AI agent use your phone")]
struct Cli {
    /// Device id (UDID or serial). Default: config or the only connected device.
    #[arg(long, short, global = true, env = "AGENTPHONE_DEVICE")]
    device: Option<String>,

    /// More logging (-v, -vv).
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List connected devices.
    Devices,
    /// Check the setup and print how to fix problems.
    Doctor,
    /// Save a screenshot.
    Screenshot {
        #[arg(default_value = "screenshot.png")]
        output: String,
    },
    /// Tap at normalized coordinates (0..1).
    Tap { x: f32, y: f32 },
    /// Swipe between two normalized points.
    Swipe {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        #[arg(long, default_value_t = 300)]
        duration_ms: u64,
    },
    /// Type text into the focused field.
    Type { text: String },
    /// Press a key, e.g. enter or cmd+space.
    Key { key: String },
    /// Press a button: home, lock, volume-up, volume-down, siri, app-switcher.
    Button { button: String },
    /// Open an app by bundle id or package name.
    Open { app: String },
    /// Run an agent task in plain language.
    Run { task: String },
    /// Start the MCP stdio server.
    Mcp,
    /// Show or edit configuration.
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Print the config file path.
    Path,
    /// Print the current configuration.
    Show,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .init();

    let name = match &cli.command {
        Command::Devices => "devices",
        Command::Doctor => "doctor",
        Command::Screenshot { .. } => "screenshot",
        Command::Tap { .. } => "tap",
        Command::Swipe { .. } => "swipe",
        Command::Type { .. } => "type",
        Command::Key { .. } => "key",
        Command::Button { .. } => "button",
        Command::Open { .. } => "open",
        Command::Run { .. } => "run",
        Command::Mcp => "mcp",
        Command::Config { .. } => "config",
    };
    println!("agentphone {name}: not yet implemented");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }
}
