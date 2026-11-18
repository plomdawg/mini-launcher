//! `mini-launcher battlenet list`: the installed Battle.net games Mini
//! Launcher sees, with their launch codes. `--verbose` also shows what
//! detection looked at, for when a game is missing.

use std::io::Write;

use anyhow::Result;
use mini_launcher_core::battlenet;

pub fn list(out: &mut impl Write, verbose: bool) -> Result<()> {
    match battlenet::client_exe() {
        Ok(exe) => writeln!(out, "Battle.net: {}", exe.display())?,
        Err(e) => writeln!(out, "{e}")?,
    }
    let scan = battlenet::scan();
    if verbose {
        if let Some(e) = &scan.product_db_error {
            writeln!(out, "cannot read product.db: {e}")?;
        }
        writeln!(out, "Installs Battle.net knows about:")?;
        for i in &scan.installs {
            writeln!(
                out,
                "  [{}] {}  {}",
                i.source,
                i.product,
                i.location.display()
            )?;
        }
        writeln!(out, "WoW flavor folders with a program in them:")?;
        let mut seen = Vec::new();
        for i in &scan.installs {
            if seen.contains(&&i.location) {
                continue;
            }
            seen.push(&i.location);
            for f in battlenet::flavors_in(&i.location) {
                let product = f.product.as_deref().unwrap_or("(no .flavor.info)");
                writeln!(out, "  {}  {product}", f.dir.display())?;
            }
        }
        writeln!(out, "Also probed {} default folders.", scan.dirs.len())?;
        writeln!(out, "Found:")?;
    }
    for g in &scan.games {
        writeln!(out, "{:>6}  {}  ({})", g.code, g.name, g.location.display())?;
    }
    let plural = if scan.games.len() == 1 { "" } else { "s" };
    writeln!(out, "{} installed game{plural}", scan.games.len())?;
    Ok(())
}
