//! Filesystem-agnostic storage pressure detection.
//!
//! The launcher deliberately does not inspect filesystem names and never
//! attempts an on-disk repair. It asks the operating system for resources
//! available to the current user, and treats file-slot counts as optional
//! because not every platform or filesystem exposes a meaningful value.

use std::io;
use std::path::{Path, PathBuf};

/// Warn when fewer bytes than this remain available to the current user.
pub const MIN_AVAILABLE_BYTES: u64 = 1024 * 1024 * 1024;
/// Warn when available bytes fall below this fraction of total capacity.
pub const MIN_AVAILABLE_PERCENT: f64 = 1.0;
/// Warn when a filesystem reports fewer available inode/file-record slots.
pub const MIN_AVAILABLE_FILE_SLOTS: u64 = 4096;

/// Capacity values returned by the host OS for the volume containing `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSnapshot {
    /// Existing path used for the query. This may be an ancestor of the
    /// requested path when an install destination has not been created yet.
    pub path: PathBuf,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub total_file_slots: Option<u64>,
    pub available_file_slots: Option<u64>,
}

/// Which generic resource classes are under pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoragePressure {
    pub bytes: bool,
    pub file_slots: bool,
}

impl StoragePressure {
    pub fn any(self) -> bool {
        self.bytes || self.file_slots
    }
}

impl StorageSnapshot {
    /// Evaluates the product-level warning thresholds without considering a
    /// filesystem type. Unknown or nonsensical file-slot counters are ignored.
    pub fn pressure(&self) -> StoragePressure {
        let bytes = below_threshold(self.available_bytes, self.total_bytes, MIN_AVAILABLE_BYTES);
        let file_slots = match (self.available_file_slots, self.total_file_slots) {
            (Some(available), Some(total)) if total > 0 && available <= total => {
                below_threshold(available, total, MIN_AVAILABLE_FILE_SLOTS)
            }
            _ => false,
        };
        StoragePressure { bytes, file_slots }
    }

    pub fn available_bytes_percent(&self) -> f64 {
        percent(self.available_bytes, self.total_bytes)
    }

    pub fn available_file_slots_percent(&self) -> Option<f64> {
        match (self.available_file_slots, self.total_file_slots) {
            (Some(available), Some(total)) if total > 0 && available <= total => {
                Some(percent(available, total))
            }
            _ => None,
        }
    }
}

fn below_threshold(available: u64, total: u64, absolute_minimum: u64) -> bool {
    if total == 0 {
        return false;
    }
    available < absolute_minimum || percent(available, total) < MIN_AVAILABLE_PERCENT
}

fn percent(available: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (available as f64 / total as f64) * 100.0
    }
}

/// Returns the nearest existing path so new clone destinations can be checked
/// against the volume on which they will be created.
fn nearest_existing(path: &Path) -> io::Result<PathBuf> {
    let mut current = path;
    loop {
        if current.exists() {
            return if current.is_dir() {
                Ok(current.to_path_buf())
            } else {
                current.parent().map(Path::to_path_buf).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no existing directory ancestor for {}", path.display()),
                    )
                })
            };
        }
        current = current.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no existing ancestor for {}", path.display()),
            )
        })?;
    }
}

/// Queries storage resources for the volume containing `path`.
pub fn inspect(path: &Path) -> io::Result<StorageSnapshot> {
    let existing = nearest_existing(path)?;
    inspect_existing(&existing)
}

#[cfg(unix)]
fn inspect_existing(path: &Path) -> io::Result<StorageSnapshot> {
    let stats = nix::sys::statvfs::statvfs(path).map_err(io::Error::other)?;
    let block_size = stats.fragment_size() as u64;
    let total_bytes = (stats.blocks() as u64).saturating_mul(block_size);
    let available_bytes = (stats.blocks_available() as u64).saturating_mul(block_size);
    let total_slots = stats.files() as u64;
    let available_slots = stats.files_available() as u64;
    let file_slots_meaningful = total_slots > 0 && available_slots <= total_slots;
    Ok(StorageSnapshot {
        path: path.to_path_buf(),
        total_bytes,
        available_bytes,
        total_file_slots: file_slots_meaningful.then_some(total_slots),
        available_file_slots: file_slots_meaningful.then_some(available_slots),
    })
}

#[cfg(windows)]
fn inspect_existing(path: &Path) -> io::Result<StorageSnapshot> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    // UNC directory names require a trailing separator. It is accepted for
    // ordinary drive paths as well, so normalise both forms uniformly.
    if wide
        .last()
        .is_none_or(|last| *last != b'\\' as u16 && *last != b'/' as u16)
    {
        wide.push(b'\\' as u16);
    }
    wide.push(0);
    let mut available = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and the three output pointers remain
    // valid for the duration of the call.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(StorageSnapshot {
        path: path.to_path_buf(),
        total_bytes: total,
        available_bytes: available,
        total_file_slots: None,
        available_file_slots: None,
    })
}

#[cfg(not(any(unix, windows)))]
fn inspect_existing(_path: &Path) -> io::Result<StorageSnapshot> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "storage statistics are unavailable on this platform",
    ))
}

/// Compact binary-unit formatting for user-facing warnings and logs.
pub fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = (1024_u64 * 1024 * 1024) as f64;
    const MIB: f64 = (1024_u64 * 1024) as f64;
    if bytes as f64 >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        available_bytes: u64,
        total_bytes: u64,
        available_slots: Option<u64>,
        total_slots: Option<u64>,
    ) -> StorageSnapshot {
        StorageSnapshot {
            path: PathBuf::from("."),
            total_bytes,
            available_bytes,
            total_file_slots: total_slots,
            available_file_slots: available_slots,
        }
    }

    #[test]
    fn warns_for_absolute_or_percentage_byte_pressure() {
        let two_hundred_gib = 200 * 1024 * 1024 * 1024;
        assert!(
            snapshot(512 * 1024 * 1024, two_hundred_gib, None, None)
                .pressure()
                .bytes
        );
        assert!(
            snapshot(1024 * 1024 * 1024, two_hundred_gib, None, None)
                .pressure()
                .bytes
        );
        assert!(
            !snapshot(3 * 1024 * 1024 * 1024, two_hundred_gib, None, None)
                .pressure()
                .bytes
        );
    }

    #[test]
    fn file_slot_pressure_is_optional_and_validated() {
        assert!(
            snapshot(10_000, 10_000, Some(1500), Some(350_000))
                .pressure()
                .file_slots
        );
        assert!(!snapshot(10_000, 10_000, None, None).pressure().file_slots);
        assert!(
            !snapshot(10_000, 10_000, Some(2), Some(0))
                .pressure()
                .file_slots
        );
        assert!(
            !snapshot(10_000, 10_000, Some(11), Some(10))
                .pressure()
                .file_slots
        );
    }

    #[test]
    fn inspect_accepts_a_nonexistent_descendant() {
        let path = std::env::temp_dir()
            .join("comfyui-launcher-storage-test")
            .join("not-created");
        let result = inspect(&path).expect("temp directory volume should be inspectable");
        assert!(result.total_bytes > 0);
        assert!(result.available_bytes <= result.total_bytes);
    }
}
