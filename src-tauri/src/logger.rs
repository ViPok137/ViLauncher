//! Минимальный файловый логгер с ротацией (аналог electron-log: app.log, 5 МБ).

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_SIZE: u64 = 5 * 1024 * 1024;

struct FileLogger {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Log for FileLogger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= Level::Info
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let Ok(_g) = self.lock.lock() else { return };
        if std::fs::metadata(&self.path).map(|m| m.len() > MAX_SIZE).unwrap_or(false) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("old.log"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "[{secs}] [{}] {}", r.level(), r.args());
        }
        eprintln!("[{}] {}", r.level(), r.args());
    }

    fn flush(&self) {}
}

pub fn init(launcher_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(launcher_dir);
    let logger = FileLogger { path: launcher_dir.join("app.log"), lock: Mutex::new(()) };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}
