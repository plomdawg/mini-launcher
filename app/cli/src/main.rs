use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod diagnose;
mod port;
mod run;
mod scan;

/// Launch games by inserting NFC-tagged carts into a reader.
#[derive(Debug, Parser)]
#[command(name = "mini-launcher", version, about)]
#[command(subcommand_required = true, arg_required_else_help = true)]
struct Cli {
    /// More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides.
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
}

/// Reader port selection, shared by every command that opens the reader.
#[derive(Debug, Args)]
struct PortArgs {
    /// Serial port of the reader (e.g. COM7 or /dev/ttyUSB0). Auto-detected
    /// when exactly one CH340 adapter is present.
    #[arg(short, long, value_name = "NAME")]
    port: Option<String>,
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        // Old Windows consoles and redirected output show raw escape codes.
        .with_ansi(std::io::stderr().is_terminal())
        .init();
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
    init_logging(cli.verbose);
    tracing::debug!("mini-launcher {}", mini_launcher_core::version());

    let mut out = std::io::stdout().lock();
    let result = match cli.command {
        Command::Diagnose { port, all } => diagnose::run(&mut out, port.port.as_deref(), all),
        Command::Scan { port } => {
            handle_ctrl_c();
            scan::run(&mut out, port.port.as_deref(), &STOP)
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
    fn broken_pipe_is_detected_through_context() {
        let e: anyhow::Error = std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
        assert!(is_broken_pipe(&e.context("writing report")));
        assert!(!is_broken_pipe(&anyhow::anyhow!("no reply")));
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
