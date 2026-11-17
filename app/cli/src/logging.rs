//! `tracing` setup: stderr always, plus a daily-rolling file for `run`.

use std::io::IsTerminal;
use std::path::Path;

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

/// How many daily log files to keep.
const KEEP_FILES: usize = 7;

/// Installs the global subscriber. With `file_dir`, logs also go to a
/// daily-rolling file there; if that can't be set up, a warning goes to
/// stderr and logging continues without the file. Keep the returned guard
/// alive until exit, or buffered file lines are lost.
pub fn init(verbose: u8, file_dir: Option<&Path>) -> Option<WorkerGuard> {
    let stderr = fmt::layer()
        .with_writer(std::io::stderr)
        // Old Windows consoles and redirected output show raw escape codes.
        .with_ansi(std::io::stderr().is_terminal())
        .with_filter(filter(stderr_level(verbose)));

    let (file, guard, error) = match file_dir.map(open) {
        Some(Ok((writer, guard))) => {
            let layer = fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_filter(filter(file_level(verbose)));
            (Some(layer), Some(guard), None)
        }
        Some(Err(e)) => (None, None, Some(e)),
        None => (None, None, None),
    };

    tracing_subscriber::registry()
        .with(stderr)
        .with(file)
        .init();

    if let (Some(dir), Some(e)) = (file_dir, error) {
        tracing::warn!("not logging to {}: {e}", dir.display());
    }
    guard
}

/// `RUST_LOG` when set, otherwise `default`.
fn filter(default: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default))
}

fn stderr_level(verbose: u8) -> &'static str {
    match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    }
}

/// The file is read after the fact (a tray app has no console), so it
/// records launches and reader events (`info`) even without `-v`.
fn file_level(verbose: u8) -> &'static str {
    stderr_level(verbose.max(1))
}

/// Creates `dir` and a non-blocking writer to `mini-launcher.<date>.log` in it.
fn open(dir: &Path) -> Result<(NonBlocking, WorkerGuard), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("mini-launcher")
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(dir)
        .map_err(|e| e.to_string())?;
    Ok(tracing_appender::non_blocking(appender))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mini-launcher-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn file_logs_info_by_default() {
        assert_eq!(stderr_level(0), "warn");
        assert_eq!(file_level(0), "info");
        assert_eq!(file_level(2), "debug");
        assert_eq!(file_level(9), "trace");
    }

    #[test]
    fn open_creates_the_dir_and_writes_a_log_file() {
        let dir = temp_dir("open").join("logs");
        let (mut writer, guard) = open(&dir).unwrap();
        writer.write_all(b"hello\n").unwrap();
        drop(guard); // flushes
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("mini-launcher.") && name.ends_with(".log"));
        assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), "hello\n");
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn open_fails_when_the_dir_cannot_be_created() {
        let base = temp_dir("blocked");
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("not-a-dir");
        std::fs::write(&file, "").unwrap();
        assert!(open(&file.join("logs")).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
}
