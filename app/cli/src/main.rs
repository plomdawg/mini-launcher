use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Args, Parser, Subcommand};
use mini_launcher_cli::{logging, port};

mod autostart;
mod battlenet;
mod diagnose;
mod run;
mod scan;
mod steam;
mod write;

/// Launch games by inserting NFC-tagged carts into a reader.
#[derive(Debug, Parser)]
#[command(name = "mini-launcher", version, about)]
#[command(subcommand_required = true, arg_required_else_help = true)]
struct Cli {
    /// More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
    /// this, for the log file too.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List serial ports, talk to the reader and show where the config lives.
    Diagnose {
        #[command(flatten)]
        port: PortArgs,
        /// Also list non-USB serial ports.
        #[arg(long)]
        all: bool,
    },
    /// Print UID, raw pages, NDEF record and payload of each tag placed on the reader.
    Scan {
        #[command(flatten)]
        port: PortArgs,
    },
    /// Write `steam://<appid>` or `id:<slug>` to the next tag placed on the
    /// reader, then read it back to verify. Never locks the tag.
    Write {
        /// The payload, validated like a tag's: `steam://<appid>` or `id:<slug>`.
        #[arg(value_name = "PAYLOAD", value_parser = write::parse)]
        payload: mini_launcher_core::payload::Payload,
        #[command(flatten)]
        port: PortArgs,
        /// Give up if no tag is placed within this many seconds.
        #[arg(long, value_name = "SECS", default_value_t = 60)]
        timeout: u64,
    },
    /// Launch the game for each cart inserted into the reader.
    Run {
        #[command(flatten)]
        port: PortArgs,
        /// Read tag events from stdin instead of a reader, one per poll:
        /// `insert <uid-hex> [payload]` or `remove`. Exits at end of input.
        #[arg(long, conflicts_with = "port")]
        simulate: bool,
        /// Log what would be launched without starting anything.
        #[arg(long)]
        dry_run: bool,
        /// Use this config file instead of the default location.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// Start `run` when you log in (Windows only for now).
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
    },
    /// Installed Steam games and non-Steam shortcuts.
    Steam {
        #[command(subcommand)]
        action: SteamAction,
    },
    /// Installed Battle.net games (Windows only).
    #[command(name = "battlenet")]
    BattleNet {
        #[command(subcommand)]
        action: BattleNetAction,
    },
}

#[derive(Debug, Subcommand)]
enum SteamAction {
    /// List installed games and non-Steam shortcuts (appid and name),
    /// sorted by name.
    List,
}

#[derive(Debug, Subcommand)]
enum BattleNetAction {
    /// List installed games (launch code, name and folder), sorted by name.
    /// With -v, also what detection looked at (product.db, uninstall
    /// entries, WoW flavor folders).
    List,
}

#[derive(Debug, Subcommand)]
enum AutostartAction {
    /// Start `run` at login (per user, no admin rights needed).
    Enable {
        /// Pass this reader port to `run` instead of auto-detecting it.
        #[arg(short, long, value_name = "NAME")]
        port: Option<String>,
    },
    /// Stop starting at login.
    Disable,
    /// Show whether `run` starts at login.
    Status,
}

/// Reader port selection, shared by every command that opens the reader.
#[derive(Debug, Args)]
struct PortArgs {
    /// Serial port of the reader (e.g. COM7 or /dev/ttyUSB0). Auto-detected
    /// when exactly one CH340 adapter is present.
    #[arg(short, long, value_name = "NAME")]
    port: Option<String>,
}

/// Set by Ctrl-C; long-running commands poll it and return cleanly.
static STOP: AtomicBool = AtomicBool::new(false);

fn handle_ctrl_c() {
    let result = ctrlc::set_handler(|| {
        // A second Ctrl-C exits right away, in case a command is stuck.
        if STOP.swap(true, Ordering::Relaxed) {
            std::process::exit(130);
        }
    });
    if let Err(e) = result {
        tracing::warn!("cannot install Ctrl-C handler: {e}");
    }
}

/// Whether `e` came from writing to a closed stdout pipe.
fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Only `run` is long-lived (and, as a tray app, has no console to read).
    let is_run = matches!(cli.command, Command::Run { .. });
    let log_dir = is_run.then(mini_launcher_core::logs::dir).flatten();
    // Held until exit so buffered file lines get flushed.
    let _log_guard = logging::init(cli.verbose, log_dir.as_deref());
    if is_run && log_dir.is_none() {
        tracing::warn!("no data directory available on this system; logging to stderr only");
    }
    tracing::debug!("mini-launcher {}", mini_launcher_core::version());

    let mut out = std::io::stdout().lock();
    let result = match cli.command {
        Command::Diagnose { port, all } => diagnose::run(&mut out, port.port.as_deref(), all),
        Command::Scan { port } => {
            handle_ctrl_c();
            scan::run(&mut out, port.port.as_deref(), &STOP)
        }
        Command::Write {
            payload,
            port,
            timeout,
        } => {
            handle_ctrl_c();
            let timeout = std::time::Duration::from_secs(timeout);
            write::run(&mut out, &payload, port.port.as_deref(), timeout, &STOP)
        }
        Command::Run {
            port,
            simulate,
            dry_run,
            config,
        } => {
            handle_ctrl_c();
            let opts = run::Options {
                port: port.port,
                simulate,
                dry_run,
                config,
            };
            run::run(&mut out, opts, &STOP)
        }
        Command::Autostart { action } => {
            let action = match action {
                AutostartAction::Enable { port } => autostart::Action::Enable(port),
                AutostartAction::Disable => autostart::Action::Disable,
                AutostartAction::Status => autostart::Action::Status,
            };
            autostart::run(&mut out, action)
        }
        Command::Steam {
            action: SteamAction::List,
        } => steam::list(&mut out),
        Command::BattleNet {
            action: BattleNetAction::List,
        } => battlenet::list(&mut out, cli.verbose > 0),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // Output piped into something that quit early (`| head`): not an error.
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn diagnose_takes_optional_port_and_global_verbose() {
        let cli =
            Cli::try_parse_from(["mini-launcher", "diagnose", "-vv", "--port", "COM7"]).unwrap();
        assert_eq!(cli.verbose, 2);
        let Command::Diagnose { port, all } = cli.command else {
            panic!("not diagnose")
        };
        assert_eq!(port.port.as_deref(), Some("COM7"));
        assert!(!all);

        let cli = Cli::try_parse_from(["mini-launcher", "diagnose", "--all"]).unwrap();
        let Command::Diagnose { port, all } = cli.command else {
            panic!("not diagnose")
        };
        assert_eq!(port.port, None);
        assert!(all);
    }

    #[test]
    fn run_simulate_conflicts_with_port() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["mini-launcher", "run"], args].concat());
        assert!(parse(&["--simulate", "--port", "COM7"]).is_err());
        let Command::Run {
            simulate, dry_run, ..
        } = parse(&["--simulate", "--dry-run"]).unwrap().command
        else {
            panic!("not run")
        };
        assert!(simulate && dry_run);
        assert!(parse(&["-p", "COM7"]).is_ok());
    }

    #[test]
    fn write_validates_its_payload() {
        let parse =
            |args: &[&str]| Cli::try_parse_from([&["mini-launcher", "write"], args].concat());
        let Command::Write {
            payload,
            port,
            timeout,
        } = parse(&["steam://440", "--port", "COM7"]).unwrap().command
        else {
            panic!("not write")
        };
        assert_eq!(payload, mini_launcher_core::payload::Payload::Steam(440));
        assert_eq!((port.port.as_deref(), timeout), (Some("COM7"), 60));
        assert!(parse(&["id:my-game", "--timeout", "5"]).is_ok());
        for bad in [&["id:../x"][..], &["steam://440 && calc"], &[]] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn broken_pipe_is_detected_through_context() {
        let e: anyhow::Error = std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
        assert!(is_broken_pipe(&e.context("writing report")));
        assert!(!is_broken_pipe(&anyhow::anyhow!("no reply")));
    }

    #[test]
    fn autostart_takes_an_action() {
        let cli =
            Cli::try_parse_from(["mini-launcher", "autostart", "enable", "-p", "COM7"]).unwrap();
        let Command::Autostart {
            action: AutostartAction::Enable { port },
        } = cli.command
        else {
            panic!("not autostart enable")
        };
        assert_eq!(port.as_deref(), Some("COM7"));
        assert!(Cli::try_parse_from(["mini-launcher", "autostart", "status"]).is_ok());
        assert!(Cli::try_parse_from(["mini-launcher", "autostart"]).is_err());
    }

    #[test]
    fn steam_list_parses() {
        let cli = Cli::try_parse_from(["mini-launcher", "steam", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Steam {
                action: SteamAction::List
            }
        ));
        assert!(Cli::try_parse_from(["mini-launcher", "steam"]).is_err());
        let cli = Cli::try_parse_from(["mini-launcher", "battlenet", "list", "-v"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::BattleNet {
                action: BattleNetAction::List
            }
        ));
        assert_eq!(cli.verbose, 1);
    }

    #[test]
    fn no_subcommand_shows_help() {
        let err = Cli::try_parse_from(["mini-launcher"]).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}
