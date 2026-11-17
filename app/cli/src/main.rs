use clap::Parser;

/// Launch games by inserting NFC-tagged carts into a reader.
#[derive(Debug, Parser)]
#[command(name = "nfc-launcher", version, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
    println!("nfc-launcher {}", nfc_launcher_core::version());
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
