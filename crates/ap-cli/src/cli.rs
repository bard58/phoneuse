//! Command-line definitions (clap derive).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

const COORDS_HELP: &str = "\
COORDINATES:
  X and Y are fractions of the screen from the top-left corner:
  0 0 is the top-left corner, 0.5 0.5 is the centre, 1 1 is the bottom-right corner.
  The grid from `agentphone screenshot --grid` shows these fractions.
  Add --px to give pixel positions in the saved screenshot instead.";

/// Let an AI agent use your phone (iPhone with iOS 17.4+, or Android).
#[derive(Debug, Parser)]
#[command(
    name = "agentphone",
    version,
    about = "Let an AI agent use your phone (iPhone with iOS 17.4+, or Android)",
    after_help = "Run `agentphone setup` first. Run `agentphone doctor` when something does not work."
)]
pub struct Cli {
    /// Device id (iPhone UDID or Android serial). Default: `agentphone use`, or the only connected phone.
    #[arg(long, short, global = true, env = "AGENTPHONE_DEVICE")]
    pub device: Option<String>,

    /// Config file to use instead of the default one.
    #[arg(long, global = true, env = "AGENTPHONE_CONFIG", value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,

    /// More logging on stderr (-v info, -vv debug, -vvv trace).
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Step-by-step setup: phone, AI model, Claude Code. Safe to run again.
    Setup {
        /// Only set up one part.
        #[arg(value_enum)]
        part: Option<SetupPart>,
    },
    /// Check the setup and print how to fix each problem.
    Doctor,
    /// Test the phone step by step and write a report for the developer.
    #[command(
        after_help = "The steps: setup check, phone connection, screenshot, then an input test.
The input test runs only after you answer y. It presses Home, swipes one time
on the home screen, and presses Home again. It does not type and does not tap.
Exit code 0: every step that ran passed. Exit code 1: a step failed or you pressed Ctrl+C."
    )]
    Selftest {
        /// Folder for the screenshots and the report.
        #[arg(long, value_name = "DIR", default_value = "selftest")]
        out: PathBuf,
        /// Run the input test without the question.
        #[arg(long)]
        yes: bool,
    },
    /// List connected phones.
    Devices,
    /// Use this phone by default.
    Use {
        /// Device id from `agentphone devices`.
        id: String,
    },
    /// Show details of the selected phone.
    Info,
    /// Save a screenshot as PNG.
    #[command(after_help = "Example: agentphone screenshot -o home.png --grid")]
    Screenshot {
        /// Output file.
        #[arg(short, long, default_value = "screenshot.png")]
        output: PathBuf,
        /// Draw a labelled coordinate grid (0.1 steps) on the image.
        #[arg(long)]
        grid: bool,
    },
    /// Tap once.
    #[command(after_help = COORDS_HELP)]
    Tap {
        #[command(flatten)]
        at: PointArgs,
        /// Tap twice.
        #[arg(long)]
        double: bool,
    },
    /// Find text on the screen (OCR) and tap it.
    #[command(name = "tap-on", after_help = "Example: agentphone tap-on \"Wi-Fi\"")]
    TapOn {
        /// Text to find, for example a button label.
        text: String,
        /// Only accept an exact match.
        #[arg(long)]
        exact: bool,
    },
    /// Touch and hold.
    #[command(name = "long-press", after_help = COORDS_HELP)]
    LongPress {
        #[command(flatten)]
        at: PointArgs,
        /// Hold time in milliseconds (1 to 30000).
        #[arg(long, default_value_t = 800, value_parser = clap::value_parser!(u64).range(1..=30000))]
        ms: u64,
    },
    /// Swipe from one point to another.
    #[command(after_help = COORDS_HELP)]
    Swipe {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        /// Duration in milliseconds (1 to 30000).
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=30000))]
        ms: u64,
        /// The numbers are screenshot pixels, not fractions.
        #[arg(long)]
        px: bool,
    },
    /// Scroll the content. `scroll down` shows what is further down.
    Scroll {
        #[arg(value_enum)]
        direction: ScrollDir,
        /// How far, as a fraction of the screen (0.1 .. 1).
        #[arg(long, default_value_t = 0.5)]
        amount: f32,
        /// Where to scroll, as fractions (default: centre).
        #[arg(long, num_args = 2, value_names = ["X", "Y"])]
        at: Option<Vec<f32>>,
    },
    /// Type text into the focused field.
    Type { text: String },
    /// Press a key or shortcut, e.g. enter, backspace, cmd+space, ctrl+shift+left.
    Key { key: String },
    /// Press a hardware or system button.
    Button {
        #[arg(value_enum)]
        button: ButtonArg,
    },
    /// Open an app by bundle id (iOS), package name (Android) or name ("Settings").
    Open { app: String },
    /// List installed apps.
    Apps,
    /// Print the accessibility (UI) tree, if the phone backend has one.
    #[command(name = "ui-tree")]
    UiTree,
    /// Let the AI agent do a task on the phone.
    #[command(
        after_help = "The agent asks you before it sends, buys, deletes or posts anything.\nPress Ctrl+C at any time to stop it and take over the phone."
    )]
    Run(RunArgs),
    /// Run the MCP server (for Claude Code and other MCP clients). Uses stdio by default.
    Mcp {
        /// Serve streamable HTTP (path /mcp) on this local address instead. Default 127.0.0.1:8778.
        #[arg(long, value_name = "ADDR", num_args = 0..=1, default_missing_value = crate::mcp::DEFAULT_HTTP_ADDR)]
        http: Option<std::net::SocketAddr>,
    },
    /// Show or change the configuration.
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Print a shell completion script.
    #[command(
        after_help = "PowerShell: agentphone completions powershell | Out-String | Invoke-Expression"
    )]
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// A point given as two numbers.
#[derive(Debug, Clone, Args)]
pub struct PointArgs {
    /// Horizontal position (0 = left, 1 = right).
    pub x: f32,
    /// Vertical position (0 = top, 1 = bottom).
    pub y: f32,
    /// The numbers are screenshot pixels, not fractions.
    #[arg(long)]
    pub px: bool,
}

#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    /// The task in plain language, e.g. "turn on Wi-Fi".
    pub task: String,
    /// Stop after this many AI steps.
    #[arg(long)]
    pub max_steps: Option<u32>,
    /// AI model id, e.g. claude-opus-5-5.
    #[arg(long)]
    pub model: Option<String>,
    /// AI provider: anthropic or openai (OpenAI-compatible).
    #[arg(long)]
    pub provider: Option<String>,
    /// Also ask Jev (TypeSafe) for quick screen checks. Sends screenshots to typesafe.ai.
    #[cfg(feature = "jev")]
    #[arg(long)]
    pub jev: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupPart {
    /// iPhone or iPad.
    Ios,
    /// Android phone.
    Android,
    /// AI provider and model.
    Llm,
    /// Register with Claude Code.
    Mcp,
    /// Download the text recognition models (builds with OCR only).
    Ocr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ButtonArg {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
    AppSwitcher,
    Siri,
    Back,
}

impl ButtonArg {
    pub fn to_core(self) -> ap_core::Button {
        use ap_core::Button as B;
        match self {
            ButtonArg::Home => B::Home,
            ButtonArg::Lock => B::Lock,
            ButtonArg::VolumeUp => B::VolumeUp,
            ButtonArg::VolumeDown => B::VolumeDown,
            ButtonArg::AppSwitcher => B::AppSwitcher,
            ButtonArg::Siri => B::Siri,
            ButtonArg::Back => B::Back,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the current configuration (default).
    Show,
    /// Print the config file path.
    Path,
    /// Change one value, e.g. `config set llm.model claude-sonnet-5-5`.
    #[command(
        after_help = "Use `none` to clear an optional value, e.g. `config set default_device none`.\nFor a list (e.g. ios.backend_order), `none` restores the default list.\nAPI keys are never stored here: set them as environment variables."
    )]
    Set { key: String, value: String },
    /// Open the config file in a text editor.
    Edit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("agentphone").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_commands() {
        assert!(parse(&[]).command.is_none());
        match parse(&["tap", "0.5", "0.25"]).command {
            Some(Command::Tap { at, double }) => {
                assert_eq!((at.x, at.y, at.px, double), (0.5, 0.25, false, false));
            }
            c => panic!("{c:?}"),
        }
        match parse(&["--device", "abc", "tap", "--px", "300", "600"]) {
            Cli {
                device: Some(d),
                command: Some(Command::Tap { at, .. }),
                ..
            } => assert!(d == "abc" && at.px && at.x == 300.0),
            c => panic!("{c:?}"),
        }
        match parse(&["swipe", "0.5", "0.8", "0.5", "0.2", "--ms", "500"]).command {
            Some(Command::Swipe { ms, y2, .. }) => assert_eq!((ms, y2), (500, 0.2)),
            c => panic!("{c:?}"),
        }
        match parse(&["scroll", "down", "--at", "0.5", "0.6"]).command {
            Some(Command::Scroll { direction, at, .. }) => {
                assert_eq!(direction, ScrollDir::Down);
                assert_eq!(at, Some(vec![0.5, 0.6]));
            }
            c => panic!("{c:?}"),
        }
        match parse(&["button", "volume-up"]).command {
            Some(Command::Button { button }) => {
                assert_eq!(button.to_core(), ap_core::Button::VolumeUp)
            }
            c => panic!("{c:?}"),
        }
        match parse(&["button", "app-switcher"]).command {
            Some(Command::Button { button }) => assert_eq!(button, ButtonArg::AppSwitcher),
            c => panic!("{c:?}"),
        }
        match parse(&["run", "turn on wifi", "--max-steps", "5", "--model", "m"]).command {
            Some(Command::Run(r)) => {
                assert_eq!(r.task, "turn on wifi");
                assert_eq!(r.max_steps, Some(5));
            }
            c => panic!("{c:?}"),
        }
        match parse(&["mcp", "--http", "127.0.0.1:8765"]).command {
            Some(Command::Mcp { http: Some(a) }) => assert_eq!(a.port(), 8765),
            c => panic!("{c:?}"),
        }
        match parse(&["mcp", "--http"]).command {
            Some(Command::Mcp { http: Some(a) }) => assert_eq!(a.to_string(), "127.0.0.1:8778"),
            c => panic!("{c:?}"),
        }
        assert!(matches!(
            parse(&["mcp"]).command,
            Some(Command::Mcp { http: None })
        ));
        assert!(parse(&["devices", "--json"]).json);
        match parse(&["selftest"]).command {
            Some(Command::Selftest { out, yes }) => {
                assert_eq!((out, yes), (PathBuf::from("selftest"), false));
            }
            c => panic!("{c:?}"),
        }
        match parse(&["selftest", "--yes", "--out", "x"]).command {
            Some(Command::Selftest { out, yes }) => {
                assert!(yes && out == std::path::Path::new("x"))
            }
            c => panic!("{c:?}"),
        }
        assert!(matches!(
            parse(&["config", "set", "llm.model", "x"]).command,
            Some(Command::Config {
                action: Some(ConfigAction::Set { .. })
            })
        ));
        assert!(matches!(
            parse(&["setup", "android"]).command,
            Some(Command::Setup {
                part: Some(SetupPart::Android)
            })
        ));
    }

    #[test]
    fn rejects_bad_input() {
        let bad = |a: &[&str]| {
            Cli::try_parse_from(std::iter::once("agentphone").chain(a.iter().copied())).is_err()
        };
        assert!(bad(&["tap", "0.5"]));
        assert!(bad(&["button", "eject"]));
        assert!(bad(&["scroll", "sideways"]));
        assert!(bad(&["run", "x", "--yes-to-confirmations"]));
        assert!(bad(&["mcp", "--http", "not-an-address"]));
    }
}
