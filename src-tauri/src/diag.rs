//! A small append-only log in the app data folder, so an unexpected exit always
//! leaves a reason behind: panics, startup errors and why the event loop ended.

use std::backtrace::Backtrace;
use std::fmt::Display;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
const MAX_LOG_BYTES: u64 = 1 << 20;

/// Starts writing to `<dir>/photomanager.log`, keeping one previous log.
pub fn init(dir: &std::path::Path) {
    let path = dir.join("photomanager.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("photomanager.old.log"));
    }
    let _ = LOG_FILE.set(path);
}

pub fn log(message: impl Display) {
    let line = format!("{} {message}", photo_core::now_ms());
    eprintln!("{line}");
    if let Some(path) = LOG_FILE.get()
        && let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Panics in background threads (scans, decoding) don't take the app down, but they
/// should never be silent.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log(format!("panic: {info}\n{}", Backtrace::force_capture()));
        default(info);
    }));
}
