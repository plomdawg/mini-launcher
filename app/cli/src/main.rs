use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod diagnose;
mod port;

/// Launch games by inserting NFC-tagged carts into a reader.
#[derive(Debug, Parser)]
#[command(name = "nfc-launcher", version, about)]
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
        .init();
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
    tracing::debug!("nfc-launcher {}", nfc_launcher_core::version());

    let mut out = std::io::stdout().lock();
    let result = match cli.command {
        Command::Diagnose { port, all } => diagnose::run(&mut out, port.port.as_deref(), all),
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
            Cli::try_parse_from(["nfc-launcher", "diagnose", "-vv", "--port", "COM7"]).unwrap();
        assert_eq!(cli.verbose, 2);
        let Command::Diagnose { port, all } = cli.command;
        assert_eq!(port.port.as_deref(), Some("COM7"));
        assert!(!all);

        let cli = Cli::try_parse_from(["nfc-launcher", "diagnose", "--all"]).unwrap();
        let Command::Diagnose { port, all } = cli.command;
        assert_eq!(port.port, None);
        assert!(all);
    }

    #[test]
    fn broken_pipe_is_detected_through_context() {
        let e: anyhow::Error = std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
        assert!(is_broken_pipe(&e.context("writing report")));
        assert!(!is_broken_pipe(&anyhow::anyhow!("no reply")));
    }

    #[test]
    fn no_subcommand_shows_help() {
        let err = Cli::try_parse_from(["nfc-launcher"]).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}
