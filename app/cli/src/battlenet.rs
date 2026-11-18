//! `mini-launcher battlenet list`: the installed Battle.net games Mini
//! Launcher sees, with their launch codes.

use std::io::Write;

use anyhow::Result;
use mini_launcher_core::battlenet;

pub fn list(out: &mut impl Write) -> Result<()> {
    match battlenet::client_exe() {
        Ok(exe) => writeln!(out, "Battle.net: {}", exe.display())?,
        Err(e) => writeln!(out, "{e}")?,
    }
    let games = battlenet::installed_games();
    for g in &games {
        writeln!(out, "{:>6}  {}  ({})", g.code, g.name, g.location.display())?;
    }
    let plural = if games.len() == 1 { "" } else { "s" };
    writeln!(out, "{} installed game{plural}", games.len())?;
    Ok(())
}
