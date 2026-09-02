//! Process-wide ring buffer for log lines, mirrored to a session log file.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const CAPACITY: usize = 4000;

/// A single log entry produced by the launcher or a child process.
#[derive(Debug, Clone)]
pub struct LogLine {
    /// Stable identity of this logical entry. Progress updates retain it.
    pub(crate) id: u64,
    /// Monotonic content version used by display caches.
    pub(crate) version: u64,
    /// Timestamp formatted as `HH:MM:SS`.
    pub ts: String,
    /// Logical source name (for example `git`, `pip`, `launcher`).
    pub source: String,
    /// Message text.
    pub text: String,
    /// Stable key for an in-place progress line.
    pub progress_key: Option<String>,
}

static BUS: OnceLock<Mutex<VecDeque<LogLine>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static REVISION: AtomicU64 = AtomicU64::new(0);
// Optional session log file. Populated by `init_file`; if init fails, file
// logging is silently skipped while the in-memory buffer keeps working.
static FILE: OnceLock<Mutex<File>> = OnceLock::new();
static FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

fn bus() -> &'static Mutex<VecDeque<LogLine>> {
    BUS.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
}

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn next_revision() -> u64 {
    REVISION.fetch_add(1, Ordering::Relaxed) + 1
}

impl LogLine {
    /// Stable identity consumed by reusable log-display caches.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// Content version consumed by reusable log-display caches.
    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    #[cfg(test)]
    pub(crate) fn test(id: u64, source: &str, text: &str) -> Self {
        Self {
            id,
            version: 1,
            ts: "00:00:00".to_string(),
            source: source.to_string(),
            text: text.to_string(),
            progress_key: None,
        }
    }
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

/// Returns the current session log path, when file logging was initialized.
pub fn file_path() -> Option<PathBuf> {
    FILE_PATH.get().cloned()
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
        g.pop_front();
    }
    let version = next_revision();
    g.push_back(LogLine {
        id: next_id(),
        version,
        ts,
        source,
        text,
        progress_key: None,
    });
}

/// Appends or replaces a single in-place progress line.
pub fn push_progress(source: impl Into<String>, key: impl Into<String>, text: impl Into<String>) {
    let source = source.into();
    let key = key.into();
    let text = text.into();
    if text.is_empty() {
        return;
    }
    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
    if let Some(m) = FILE.get() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{ts} [{source}] {text}");
        }
    }
    let mut g = bus().lock().unwrap();
    if let Some(index) = g
        .iter_mut()
        .rposition(|line| line.progress_key.as_deref() == Some(key.as_str()))
    {
        let mut line = g
            .remove(index)
            .expect("progress line index came from deque");
        line.version = next_revision();
        line.ts = ts;
        line.source = source;
        line.text = text;
        // A progress update is logically the newest entry. Moving it to the
        // back keeps small tail snapshots live even when the same source key
        // was first created thousands of lines earlier.
        g.push_back(line);
        return;
    }
    if g.len() == CAPACITY {
        g.pop_front();
    }
    let version = next_revision();
    g.push_back(LogLine {
        id: next_id(),
        version,
        ts,
        source,
        text,
        progress_key: Some(key),
    });
}

/// Returns a snapshot of the current in-memory log buffer.
#[cfg(test)]
pub fn snapshot() -> Vec<LogLine> {
    bus().lock().unwrap().iter().cloned().collect()
}

/// Monotonic counter changed whenever the in-memory buffer changes.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

/// Visits a consistent view of the retained entries without cloning their
/// message strings. The callback should only copy lightweight metadata; log
/// formatting is deliberately performed after this function releases the
/// bus lock.
pub(crate) fn with_lines<R>(f: impl FnOnce(&VecDeque<LogLine>, u64) -> R) -> R {
    let g = bus().lock().unwrap();
    let revision = REVISION.load(Ordering::Relaxed);
    f(&g, revision)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_replace_existing_key() {
        let key = "progress_lines_replace_existing_key";
        push_progress("pip", key, "10%");
        let first = snapshot()
            .into_iter()
            .find(|line| line.progress_key.as_deref() == Some(key))
            .expect("progress line exists");
        push_progress("pip", key, "50%");
        push_progress("pip", key, "100%");

        let snap = snapshot();
        let got: Vec<_> = snap
            .iter()
            .filter(|line| line.progress_key.as_deref() == Some(key))
            .collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, "pip");
        assert_eq!(got[0].text, "100%");
        assert_eq!(got[0].id, first.id);
        assert!(got[0].version > first.version);
    }

    #[test]
    fn normal_lines_do_not_replace_progress() {
        let key = "normal_lines_do_not_replace_progress";
        push_progress("pip", key, "50%");
        push("pip", "done");

        let snap = snapshot();
        assert!(snap
            .iter()
            .any(|line| line.progress_key.as_deref() == Some(key) && line.text == "50%"));
        assert!(snap
            .iter()
            .any(|line| line.progress_key.is_none() && line.text == "done"));
    }
}
