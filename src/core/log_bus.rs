//! Process-wide ring buffer for log lines, mirrored to a session log file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const CAPACITY: usize = 4000;

/// A single log entry produced by the launcher or a child process.
#[derive(Debug, Clone)]
pub struct LogLine {
    /// Timestamp formatted as `HH:MM:SS`.
    pub ts: String,
    /// Logical source name (for example `git`, `pip`, `launcher`).
    pub source: String,
    /// Message text.
    pub text: String,
}

static BUS: OnceLock<Mutex<Vec<LogLine>>> = OnceLock::new();
// Optional session log file. Populated by `init_file`; if init fails, file
// logging is silently skipped while the in-memory buffer keeps working.
static FILE: OnceLock<Mutex<File>> = OnceLock::new();
static FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

fn bus() -> &'static Mutex<Vec<LogLine>> {
    BUS.get_or_init(|| Mutex::new(Vec::with_capacity(CAPACITY)))
}

/// Opens the on-disk session log at `path` so lines pushed afterwards are
/// also written to disk.
///
/// Safe to call once at startup; subsequent calls are no-ops.
pub fn init_file(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = OpenOptions::new().create(true).append(true).open(path)?;
    let _ = FILE.set(Mutex::new(f));
    let _ = FILE_PATH.set(path.to_path_buf());
    Ok(())
}

/// Appends a log line to the ring buffer and to the session log file.
pub fn push(source: impl Into<String>, text: impl Into<String>) {
    let source = source.into();
    let text = text.into();
    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
    // Append to the file first so an out-of-memory panic from the in-memory
    // ring still leaves the line persisted.
    if let Some(m) = FILE.get() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{ts} [{source}] {text}");
        }
    }
    let mut g = bus().lock().unwrap();
    if g.len() == CAPACITY {
        g.remove(0);
    }
    g.push(LogLine { ts, source, text });
}

/// Returns a snapshot of the current in-memory log buffer.
pub fn snapshot() -> Vec<LogLine> {
    bus().lock().unwrap().clone()
}

/// Returns a newline-delimited dump of the in-memory log buffer formatted as
/// `HH:MM:SS [source] message`.
pub fn dump_text() -> String {
    let g = bus().lock().unwrap();
    let mut out = String::with_capacity(g.len() * 64);
    for l in g.iter() {
        out.push_str(&l.ts);
        out.push(' ');
        out.push('[');
        out.push_str(&l.source);
        out.push(']');
        out.push(' ');
        out.push_str(&l.text);
        out.push('\n');
    }
    out
}
