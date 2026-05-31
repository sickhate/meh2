// GPL-3.0-or-later
//! meh — widget system for Wayland / GTK4

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use meh_core::{IpcCmd, IpcResponse, MehPaths};

#[derive(Parser, Debug)]
#[command(
    name = "meh2",
    version,
    author,
    about = "Widget system for Wayland (meh2 fork)"
)]
struct Cli {
    /// Override config directory
    #[arg(short, long, global = true)]
    config: Option<std::path::PathBuf>,

    /// Don't daemonize (stay in foreground)
    #[arg(long, global = true)]
    no_daemonize: bool,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the meh daemon
    #[command(alias = "d")]
    Daemon,

    /// Open a window
    #[command(alias = "o")]
    Open {
        window: String,
        #[arg(long)]
        toggle: bool,
        #[arg(long)]
        monitor: Option<i32>,
    },

    /// Close windows
    #[command(alias = "c")]
    Close { windows: Vec<String> },

    /// Close all windows
    #[command(alias = "ca")]
    CloseAll,

    /// Reload config and CSS
    #[command(alias = "r")]
    Reload,

    /// Update variable values (key=value ...)
    #[command(alias = "u")]
    Update {
        #[arg(required = true, value_parser = parse_var)]
        vars: Vec<(String, String)>,
    },

    /// Show all variable values
    State,

    /// Get a single variable value
    #[command(alias = "g")]
    Get { var: String },

    /// List open windows
    #[command(name = "list-windows", alias = "lw")]
    ListWindows,

    /// Ping the daemon
    Ping,

    /// Kill the daemon
    #[command(alias = "k")]
    Kill,

    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "meh=info,meh_daemon=info,meh_gtk4=info,meh_script_vars=info,meh_core=info",
                )
            }),
        )
        .with_target(false)
        .init();

    let paths = match &cli.config {
        Some(dir) => MehPaths::from_config_dir(dir)?,
        None => MehPaths::default_paths()?,
    };

    match cli.cmd {
        Command::Daemon => {
            meh_daemon::run(paths, !cli.no_daemonize)?;
        }

        Command::Completions { shell } => {
            use clap::CommandFactory;
            clap_complete::generate(shell, &mut Cli::command(), "meh2", &mut std::io::stdout());
        }

        other => {
            let cmd = to_ipc_cmd(other);
            let resp = meh_core::send_ipc_cmd(&paths.socket_file, &cmd)
                .await
                .context("Cannot reach meh daemon. Start it with: meh daemon")?;

            match resp {
                IpcResponse::Ok(msg) => {
                    if !msg.is_empty() {
                        println!("{}", msg);
                    }
                }
                IpcResponse::Err(e) => {
                    eprintln!("meh: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

fn to_ipc_cmd(cmd: Command) -> IpcCmd {
    match cmd {
        Command::Open {
            window,
            toggle,
            monitor,
        } => IpcCmd::Open {
            window,
            toggle,
            monitor,
        },
        Command::Close { windows } => IpcCmd::Close { windows },
        Command::CloseAll => IpcCmd::CloseAll,
        Command::Reload => IpcCmd::Reload,
        Command::Update { vars } => {
            let map: HashMap<String, String> = vars.into_iter().collect();
            IpcCmd::Update { vars: map }
        }
        Command::State => IpcCmd::State,
        Command::Get { var } => IpcCmd::Get { var },
        Command::ListWindows => IpcCmd::ListWindows,
        Command::Ping => IpcCmd::Ping,
        Command::Kill => IpcCmd::Kill,
        Command::Daemon | Command::Completions { .. } => unreachable!("handled before to_ipc_cmd"),
    }
}

fn parse_var(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.trim_matches('"').to_string()))
        .ok_or_else(|| format!("expected `key=value`, got `{}`", s))
}
