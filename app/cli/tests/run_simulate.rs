//! `mini-launcher run --simulate --dry-run`: stdin script → launch log lines.

use std::io::Write;
use std::process::{Command, Stdio};

const CONFIG: &str = r#"
[[games]]
id = "half-life-2"
name = "Half-Life 2"
uid = "04:A1:B2:C3:D4:E5:F6"
steam_appid = 220

[[games]]
id = "my-game"
name = "My Game"
exe = "game.exe"
args = ["--fullscreen"]
"#;

/// One line per poll (200 ms). Removal takes 5 missed polls.
const SCRIPT: &str = "\
# tag mapped by UID in the config
insert 04a1b2c3d4e5f6
remove
remove
remove
remove
remove
# back within the re-seat window: no relaunch
insert 04a1b2c3d4e5f6
# swap straight to a tag that carries its own steam appid
insert deadbeef steam://440
# blank, unmapped tag: never launches
insert 11223344
# a malformed line is skipped with a warning
bogus
insert 55667788 id:my-game
remove
";

#[test]
fn simulate_dry_run_logs_launches() {
    let dir = std::env::temp_dir().join(format!("mini-launcher-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(&config, CONFIG).unwrap();
    let log_dir = dir.join("logs");

    let mut child = Command::new(env!("CARGO_BIN_EXE_mini-launcher"))
        .args(["run", "--simulate", "--dry-run", "--config"])
        .arg(&config)
        .env_remove("RUST_LOG")
        .env(LOG_DIR_ENV, &log_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(SCRIPT.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let log = read_logs(&log_dir);
    std::fs::remove_dir_all(&dir).ok();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let events: Vec<&str> = stdout
        .lines()
        .filter(|l| {
            ["launch ", "removed ", "reseated ", "unknown "]
                .iter()
                .any(|p| l.starts_with(p))
        })
        .collect();
    let want = [
        r#"launch 04A1B2C3D4E5F6: open steam://rungameid/220 (uid mapped to "half-life-2") [dry run]"#,
        "removed 04A1B2C3D4E5F6",
        "reseated 04A1B2C3D4E5F6: not relaunching",
        "removed 04A1B2C3D4E5F6",
        "launch DEADBEEF: open steam://rungameid/440 (tag steam:) [dry run]",
        "removed DEADBEEF",
        "unknown tag 11223344: nothing usable on it and its UID is not mapped; \
         not launching. To map it, add to ",
        "removed 11223344",
        r#"launch 55667788: run "game.exe" with args ["--fullscreen"] (tag id:my-game) [dry run]"#,
    ];
    assert_eq!(events.len(), want.len(), "stdout:\n{stdout}");
    for (got, want) in events.iter().zip(want) {
        assert!(got.starts_with(want), "got {got:?}\nwant {want:?}");
    }
    // The hint is a ready-to-paste config entry for that UID.
    assert!(stdout.contains("    uid = \"11223344\"\n"), "{stdout}");
    assert!(stderr.contains(r#"unknown command "bogus""#), "{stderr}");
    // The file log records launches without -v.
    assert!(
        log.contains("launch DEADBEEF: open steam://rungameid/440 (tag steam:) [dry run]"),
        "log:\n{log}"
    );
}

const LOG_DIR_ENV: &str = "MINI_LAUNCHER_LOG_DIR";

/// All log files in `dir`, concatenated.
fn read_logs(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        out += &std::fs::read_to_string(entry.unwrap().path()).unwrap();
    }
    out
}

#[test]
fn explicit_missing_config_is_an_error() {
    let missing = std::env::temp_dir().join(format!(
        "mini-launcher-test-missing-{}/config.toml",
        std::process::id()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_mini-launcher"))
        .args(["run", "--simulate", "--dry-run", "--config"])
        .arg(&missing)
        .env_remove("RUST_LOG")
        .env(LOG_DIR_ENV, missing.parent().unwrap().join("logs"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    std::fs::remove_dir_all(missing.parent().unwrap()).ok();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("cannot load"), "{stderr}");
}
