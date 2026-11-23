//! Turning a tag into a [`LaunchPlan`], and carrying the plan out.
//!
//! [`resolve`] is pure: it applies the resolution order from
//! `docs/tag-format.md`. [`execute`] is the thin side-effecting layer. Neither
//! ever sees raw tag text: Steam URIs are rebuilt from a `u32` by
//! [`steam_launch_uri`], and executables/args come only from the local config.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::battlenet::{self, Code};
use crate::config::Config;
use crate::payload::{steam_launch_uri, Payload, Slug};

/// What to launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchPlan {
    /// Open `steam://rungameid/{appid}` with the desktop opener.
    Steam { appid: u32 },
    /// Start `Battle.net.exe --exec="launch <code>"` (Windows only), sent
    /// again once the client is up if it wasn't running.
    BattleNet { code: Code },
    /// Start a configured executable directly (no shell).
    Exe {
        path: PathBuf,
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    /// Start a configured emulator on a ROM, resolved when the config is
    /// loaded: `args` is the emulator's argv with `{rom}` already replaced by
    /// `rom` as one argument. Spawned like [`LaunchPlan::Exe`] (no shell).
    Emulator {
        /// The `[[emulators]]` entry this came from.
        emulator: Slug,
        rom: PathBuf,
        path: PathBuf,
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
}

impl fmt::Display for LaunchPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchPlan::Steam { appid } => write!(f, "open {}", steam_launch_uri(*appid)),
            LaunchPlan::BattleNet { code } => write!(f, "launch Battle.net game {code}"),
            LaunchPlan::Exe { path, args, cwd } => write_run(f, path, args, cwd),
            LaunchPlan::Emulator {
                emulator,
                path,
                args,
                cwd,
                ..
            } => {
                write!(f, "emulator {emulator}: ")?;
                write_run(f, path, args, cwd)
            }
        }
    }
}

fn write_run(
    f: &mut fmt::Formatter<'_>,
    path: &Path,
    args: &[String],
    cwd: &Option<PathBuf>,
) -> fmt::Result {
    write!(f, "run {:?} with args {args:?}", path.display().to_string())?;
    match cwd {
        Some(cwd) => write!(f, " in {:?}", cwd.display().to_string()),
        None => Ok(()),
    }
}

/// Why a plan was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// The tag UID is mapped to this config entry.
    Uid(Slug),
    /// The tag says `steam://<appid>`.
    TagSteam,
    /// The tag says `id:<slug>` and the config has that entry.
    TagId(Slug),
}

/// Outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Launch {
        plan: LaunchPlan,
        via: Via,
    },
    /// The tag says `id:<slug>` but the config has no such entry.
    UnknownId(Slug),
    /// Nothing usable on the tag and no UID mapping: report the UID so the
    /// user can map it.
    Unmapped,
}

/// Applies the resolution order: local UID mapping, then the tag payload.
pub fn resolve(config: &Config, uid: &[u8], payload: &Payload) -> Resolution {
    if let Some(game) = config.by_uid(uid) {
        return Resolution::Launch {
            plan: game.plan.clone(),
            via: Via::Uid(game.id.clone()),
        };
    }
    match payload {
        Payload::Steam(appid) => Resolution::Launch {
            plan: LaunchPlan::Steam { appid: *appid },
            via: Via::TagSteam,
        },
        Payload::Id(slug) => match config.by_id(slug) {
            Some(game) => Resolution::Launch {
                plan: game.plan.clone(),
                via: Via::TagId(slug.clone()),
            },
            None => Resolution::UnknownId(slug.clone()),
        },
        Payload::Unusable => Resolution::Unmapped,
    }
}

/// What [`execute`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Dry run: nothing was started; the caller should log the plan.
    DryRun,
    Started,
}

/// Carries out `plan`, or does nothing when `dry_run` is set.
pub fn execute(plan: &LaunchPlan, dry_run: bool) -> std::io::Result<Outcome> {
    if dry_run {
        return Ok(Outcome::DryRun);
    }
    match plan {
        // `open` uses ShellExecuteW on Windows and xdg-open (or another desktop
        // opener) on Linux — never the `steam` binary, which Flatpak Steam
        // lacks. The URI is rebuilt from the number, never taken from the tag.
        LaunchPlan::Steam { appid } => open::that_detached(steam_launch_uri(*appid))?,
        // The code is validated (letters, digits, `_`), so the raw argument
        // can't break out of its quotes.
        LaunchPlan::BattleNet { code } => {
            let exe = battlenet::client_exe().map_err(std::io::Error::other)?;
            battlenet::client::launch(exe, code)?
        }
        LaunchPlan::Exe { path, args, cwd }
        | LaunchPlan::Emulator {
            path, args, cwd, ..
        } => spawn(path, args, cwd.as_deref())?,
    }
    Ok(Outcome::Started)
}

/// Starts `path` directly (no shell), each of `args` as one argument.
fn spawn(path: &Path, args: &[String], cwd: Option<&Path>) -> std::io::Result<()> {
    let mut cmd = Command::new(path);
    cmd.args(args).stdin(Stdio::null());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    detach(&mut cmd);
    let mut child = cmd.spawn()?;
    // Reap the child when it exits so it doesn't linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Starts `path` with one argument passed verbatim, as Battle.net expects
/// `--exec="launch <code>"` with the quotes inside the argument.
#[cfg(windows)]
pub(crate) fn spawn_raw(path: &Path, arg: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(path);
    cmd.raw_arg(arg).stdin(Stdio::null());
    detach(&mut cmd);
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn spawn_raw(_: &Path, _: &str) -> std::io::Result<()> {
    Err(std::io::Error::other(
        battlenet::BattleNetError::Unsupported,
    ))
}

/// Keeps the launcher's Ctrl-C away from the game (killing games loses
/// saves): Ctrl-C is delivered to the whole console process group on Windows
/// and to the terminal's foreground process group on Unix.
#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

/// See the Unix version. `CREATE_NEW_PROCESS_GROUP` alone: GUI games never
/// see the console anyway, and console games keep sharing (and showing in)
/// the launcher's console instead of being hidden (`DETACHED_PROCESS`,
/// `CREATE_NO_WINDOW`) or popping up a second window (`CREATE_NEW_CONSOLE`).
#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use LaunchPlan::Steam;
    use Payload::{Id, Unusable};
    use Resolution::{UnknownId, Unmapped};

    const UID: &[u8] = &[4, 1, 2, 3, 4, 5, 6];
    const OTHER: &[u8] = &[4, 9, 9, 9, 9, 9, 9];

    fn s(s: &str) -> Slug {
        Slug::new(s).unwrap()
    }

    fn go(plan: LaunchPlan, via: Via) -> Resolution {
        Resolution::Launch { plan, via }
    }

    #[test]
    fn precedence() {
        let cfg = Config::parse(
            r#"
            [[games]]
            id = "mapped"
            name = "Mapped by UID"
            uid = "04:01:02:03:04:05:06"
            exe = "/games/mapped"
            [[games]]
            id = "lib"
            name = "Library entry"
            steam_appid = 70
            "#,
        )
        .unwrap();
        let empty = Config::default();
        let mapped = LaunchPlan::Exe {
            path: "/games/mapped".into(),
            args: vec![],
            cwd: None,
        };
        let by_uid = go(mapped, Via::Uid(s("mapped")));
        let cases = [
            // A local UID mapping overrides whatever the tag says.
            (&cfg, UID, Payload::Steam(440), by_uid.clone()),
            (&cfg, UID, Id(s("lib")), by_uid.clone()),
            (&cfg, UID, Unusable, by_uid),
            // Otherwise the tag decides.
            (
                &cfg,
                OTHER,
                Payload::Steam(440),
                go(Steam { appid: 440 }, Via::TagSteam),
            ),
            (
                &cfg,
                OTHER,
                Id(s("lib")),
                go(Steam { appid: 70 }, Via::TagId(s("lib"))),
            ),
            (&cfg, OTHER, Id(s("nope")), UnknownId(s("nope"))),
            (&cfg, OTHER, Unusable, Unmapped),
            (&cfg, &[], Unusable, Unmapped),
            // Zero config: Steam tags still work.
            (
                &empty,
                UID,
                Payload::Steam(440),
                go(Steam { appid: 440 }, Via::TagSteam),
            ),
            (&empty, UID, Id(s("lib")), UnknownId(s("lib"))),
            // Hostile tag text parses to Unusable, so it can't select a plan.
            (
                &cfg,
                OTHER,
                Payload::parse("steam://440;calc.exe"),
                Unmapped,
            ),
            (&cfg, OTHER, Payload::parse("id:../lib"), Unmapped),
        ];
        for (cfg, uid, payload, want) in cases {
            assert_eq!(resolve(cfg, uid, &payload), want, "{uid:?} {payload:?}");
        }
    }

    #[test]
    fn plan_display_and_dry_run() {
        let steam = Steam { appid: 440 };
        assert_eq!(steam.to_string(), "open steam://rungameid/440");
        let exe = LaunchPlan::Exe {
            path: "/g/run".into(),
            args: vec!["a b".into()],
            cwd: Some("/g".into()),
        };
        assert_eq!(exe.to_string(), r#"run "/g/run" with args ["a b"] in "/g""#);
        let bnet = LaunchPlan::BattleNet {
            code: Code::new("WoWF").unwrap(),
        };
        assert_eq!(bnet.to_string(), "launch Battle.net game WoWF");
        // Dry run never touches the system, even for a path that doesn't exist.
        assert_eq!(execute(&bnet, true).unwrap(), Outcome::DryRun);
        assert_eq!(execute(&steam, true).unwrap(), Outcome::DryRun);
        assert_eq!(execute(&exe, true).unwrap(), Outcome::DryRun);
    }

    #[test]
    fn emulator_entry_resolves_by_tag_id() {
        let cfg = Config::parse(
            r#"
            [[emulators]]
            id = "eden"
            name = "Eden"
            exe = "/emu/eden"
            args = ["-g", "{rom}"]
            [[games]]
            id = "mk8"
            name = "MK8"
            emulator = "eden"
            rom = "/roms/mario kart.nsp"
            "#,
        )
        .unwrap();
        let Resolution::Launch { plan, via } = resolve(&cfg, OTHER, &Id(s("mk8"))) else {
            panic!("not launched");
        };
        assert_eq!(via, Via::TagId(s("mk8")));
        assert_eq!(
            plan.to_string(),
            r#"emulator eden: run "/emu/eden" with args ["-g", "/roms/mario kart.nsp"]"#
        );
        assert_eq!(execute(&plan, true).unwrap(), Outcome::DryRun);
    }
}
