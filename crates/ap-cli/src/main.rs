//! `agentphone`: let an AI agent use your phone, from Windows, macOS and Linux.

mod actions;
mod cli;
mod config_cmd;
mod devices;
mod doctor;
mod ios;
#[cfg(feature = "jev")]
mod jev;
mod mcp;
mod run;
mod setup;
mod ui;

use std::process::ExitCode;

use anyhow::Result;
use clap::{CommandFactory, Parser};

use cli::{Cli, Command};

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = if verbose == 0 {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level))
    } else {
        tracing_subscriber::EnvFilter::new(level)
    };
    // Logs go to stderr: stdout is for results and the MCP protocol.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(verbose >= 2)
        .try_init();
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    // Every command runs on ap-ios's 32 MiB thread and runtime: the idevice
    // futures overflow the 1 MiB Windows main stack.
    ap_ios::block_on(dispatch(cli))
}

async fn dispatch(cli: Cli) -> ExitCode {
    match execute(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            ui::print_error(&e);
            ExitCode::FAILURE
        }
    }
}

async fn execute(cli: Cli) -> Result<u8> {
    let cfg_path = config_cmd::config_path(cli.config.as_deref())?;
    let wanted = cli.device.as_deref();
    let json = cli.json;

    let Some(command) = cli.command else {
        if !cfg_path.exists() && ui::interactive() {
            setup::run(&cfg_path, None, wanted).await?;
        } else {
            Cli::command().print_help()?;
            println!();
        }
        return Ok(0);
    };

    match command {
        Command::Setup { part } => setup::run(&cfg_path, part, wanted).await?,
        Command::Doctor => {
            if !doctor::run(&cfg_path, wanted, json).await? {
                return Ok(1);
            }
        }
        Command::Config { action } => config_cmd::run(action, &cfg_path, json)?,
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "agentphone",
                &mut std::io::stdout(),
            );
        }
        Command::Devices => {
            let cfg = config_cmd::load(&cfg_path)?;
            let found = devices::discover(&devices::providers(&cfg)).await;
            print_devices(&found, cfg.default_device.as_deref(), json)?;
        }
        Command::Use { id } => {
            let mut cfg = config_cmd::load(&cfg_path)?;
            let found = devices::discover(&devices::providers(&cfg)).await;
            let Some(d) = found.devices.iter().find(|d| d.id == id) else {
                let ids: Vec<&str> = found.devices.iter().map(|d| d.id.as_str()).collect();
                return Err(ap_core::Error::setup(
                    format!("no connected phone has the id \"{id}\""),
                    if ids.is_empty() {
                        "connect the phone, then run `agentphone devices`".to_string()
                    } else {
                        format!("use one of: {}", ids.join(", "))
                    },
                )
                .into());
            };
            cfg.default_device = Some(id.clone());
            config_cmd::save(&cfg, &cfg_path)?;
            ui::ok(format!("agentphone now uses {} ({id}) by default", d.name));
        }
        Command::Mcp { http } => {
            let cfg = config_cmd::load(&cfg_path)?;
            mcp::serve(cfg, cli.device.clone(), http).await?;
        }
        Command::Run(args) => {
            let cfg = config_cmd::load(&cfg_path)?;
            return run::command(&cfg, &args, wanted, json).await;
        }
        other => {
            let cfg = config_cmd::load(&cfg_path)?;
            let device = devices::connect(&cfg, wanted).await?;
            device_command(&*device, other, json).await?;
        }
    }
    Ok(0)
}

async fn device_command(d: &dyn ap_core::Device, command: Command, json: bool) -> Result<()> {
    use actions as a;
    match command {
        Command::Info => a::print_info(d.info(), d.capabilities(), json),
        Command::Screenshot { output, grid } => a::screenshot(d, &output, grid, json).await,
        Command::Tap { at, double } => {
            let p = a::point(d, at.x, at.y, at.px).await?;
            a::tap(d, p, double, json).await
        }
        Command::TapOn { text, exact } => a::tap_on(d, &text, exact, json).await,
        Command::LongPress { at, ms } => {
            let p = a::point(d, at.x, at.y, at.px).await?;
            a::long_press(d, p, ms, json).await
        }
        Command::Swipe {
            x1,
            y1,
            x2,
            y2,
            ms,
            px,
        } => {
            let from = a::point(d, x1, y1, px).await?;
            let to = a::point(d, x2, y2, px).await?;
            a::swipe(d, from, to, ms, json).await
        }
        Command::Scroll {
            direction,
            amount,
            at,
        } => a::scroll(d, direction, amount, at, json).await,
        Command::Type { text } => a::type_text(d, &text, json).await,
        Command::Key { key } => a::key(d, &key, json).await,
        Command::Button { button } => a::button(d, button.to_core(), json).await,
        Command::Open { app } => a::open(d, &app, json).await,
        Command::Apps => a::apps(d, json).await,
        Command::UiTree => a::ui_tree(d, json).await,
        _ => unreachable!("handled in execute"),
    }
}

fn print_devices(found: &devices::Discovery, default: Option<&str>, json: bool) -> Result<()> {
    if json {
        return ui::json(&serde_json::json!({
            "devices": found.devices,
            "default": default,
            "problems": found.problems,
        }));
    }
    if found.devices.is_empty() {
        println!("No phones found.");
    }
    for d in &found.devices {
        let mark = if Some(d.id.as_str()) == default {
            console::style("*").green().bold().to_string()
        } else {
            " ".to_string()
        };
        println!(
            "{mark} {:<40} {} {} {}",
            d.id,
            console::style(&d.name).bold(),
            console::style(format!("{} {}", d.platform, d.os_version)).dim(),
            console::style(format!("[{}]", d.backend)).dim()
        );
    }
    for p in &found.problems {
        ui::warn(format!("{}: {}", p.provider, p.problem));
        if let Some(f) = &p.fix {
            eprintln!("    {} {f}", console::style("Fix:").cyan().bold());
        }
    }
    if found.devices.len() > 1 && default.is_none() {
        eprintln!("Choose one with `agentphone use <id>`.");
    }
    Ok(())
}
