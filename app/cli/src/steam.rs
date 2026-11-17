//! `mini-launcher steam list`: the installed Steam games Mini Launcher sees.

use std::io::Write;

use anyhow::Result;
use mini_launcher_core::steam;

pub fn list(out: &mut impl Write) -> Result<()> {
    let games = steam::installed_games()?;
    for g in &games {
        writeln!(out, "{:>10}  {}", g.appid, g.name)?;
    }
    let plural = if games.len() == 1 { "" } else { "s" };
    writeln!(out, "{} installed game{plural}", games.len())?;
    Ok(())
}
