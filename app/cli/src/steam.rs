//! `mini-launcher steam list`: the installed Steam games Mini Launcher sees,
//! and the non-Steam games added to Steam (shortcuts).

use std::io::Write;

use anyhow::Result;
use mini_launcher_core::steam::{self, SteamGame, SteamKind};

pub fn list(out: &mut impl Write) -> Result<()> {
    write_list(out, &steam::installed_games()?)
}

fn write_list(out: &mut impl Write, games: &[SteamGame]) -> Result<()> {
    let shortcuts = games
        .iter()
        .filter(|g| g.kind == SteamKind::Shortcut)
        .count();
    for g in games {
        match g.kind {
            SteamKind::App => writeln!(out, "{:>10}  {}", g.appid, g.name)?,
            SteamKind::Shortcut => writeln!(out, "{:>10}  {} (shortcut)", g.appid, g.name)?,
        }
    }
    let apps = games.len() - shortcuts;
    let plural = |n| if n == 1 { "" } else { "s" };
    writeln!(
        out,
        "{apps} installed game{}, {shortcuts} shortcut{}",
        plural(apps),
        plural(shortcuts)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_apps_and_shortcuts() {
        let game = |appid, name: &str, kind| SteamGame {
            appid,
            name: name.into(),
            installdir: String::new(),
            library: Default::default(),
            kind,
        };
        let games = [
            game(3_237_998_081, "Doom", SteamKind::Shortcut),
            game(620, "Portal 2", SteamKind::App),
        ];
        let mut out = Vec::new();
        write_list(&mut out, &games).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "3237998081  Doom (shortcut)\n       620  Portal 2\n1 installed game, 1 shortcut\n"
        );
    }
}
