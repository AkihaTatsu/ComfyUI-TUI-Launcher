//! Filesystem paths owned by the launcher.
//!
//! The default build follows platform conventions (`dirs::config_dir`,
//! `dirs::cache_dir`, the system temp directory). The `portable` build
//! puts every writable path under `<exe_dir>/local_data/` so the launcher
//! is self-contained.

use std::path::{Path, PathBuf};

#[cfg_attr(feature = "portable", allow(dead_code))]
const APP_DIR: &str = "comfyui-tui-launcher";

/// Returns the writable root used by portable builds.
///
/// Resolves to `<exe_dir>/local_data/`, falling back to the current working
/// directory if `current_exe` is unavailable.
#[cfg(feature = "portable")]
fn portable_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("local_data")
}

/// Returns the directory that stores launcher configuration files.
pub fn config_dir() -> PathBuf {
    #[cfg(feature = "portable")]
    {
        return portable_root().join("config");
    }
    #[cfg(not(feature = "portable"))]
    {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
            .join(APP_DIR)
    }
}

/// Returns the path to `launcher_config.toml`.
pub fn config_file() -> PathBuf {
    config_dir().join("launcher_config.toml")
}

/// Returns the path to `comfyui_config.toml`.
///
/// The file holds a flat top-level map of overrides for the ComfyUI
/// settings schema.
pub fn comfyui_config_file() -> PathBuf {
    config_dir().join("comfyui_config.toml")
}

/// Performs a best-effort one-time migration and cleanup of legacy file
/// names in the configuration directory.
///
/// Existing installs keep their user settings after upgrading, and any
/// previously extracted schema files are removed because the schema is
/// embedded in the binary and must never live on disk.
pub fn migrate_legacy_filenames() {
    let dir = config_dir();
    // Rename user-value files that changed name in earlier releases.
    {
        let (old, new) = ("config.toml", "launcher_config.toml");
        let old_p = dir.join(old);
        let new_p = dir.join(new);
        if old_p.is_file() && !new_p.is_file() {
            let _ = std::fs::rename(&old_p, &new_p);
        }
    }
    // Delete schema files left by older builds. The schema lives in the
    // binary; keeping a copy on disk would let stale field labels survive
    // upgrades.
    for stale in [
        "comfyui_schema.toml",
        "launcher_schema.toml",
        "settings_schema.toml",
    ] {
        let p = dir.join(stale);
        if p.is_file() {
            let _ = std::fs::remove_file(&p);
        }
    }
    // Older builds named the schema `comfyui_config.toml`. A real
    // post-split file holds a flat map of overrides; any line starting
    // with `[[tab` marks a stray schema that can be removed safely.
    let stray = dir.join("comfyui_config.toml");
    if stray.is_file() {
        if let Ok(t) = std::fs::read_to_string(&stray) {
            if t.contains("[[tab") || t.contains("[[tab.field") {
                let _ = std::fs::remove_file(&stray);
            }
        }
    }
}

/// Ensures the launcher configuration directory exists.
pub fn ensure_config_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())
}

/// Returns the launcher's cache directory.
///
/// Standard builds use the OS cache directory; portable builds use
/// `<exe_dir>/local_data/cache/`. Only files that can be regenerated on
/// demand belong here — never user settings.
pub fn cache_dir() -> PathBuf {
    #[cfg(feature = "portable")]
    {
        return portable_root().join("cache");
    }
    #[cfg(not(feature = "portable"))]
    {
        dirs::cache_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
            .join(APP_DIR)
    }
}

/// Ensures the launcher cache directory exists.
pub fn ensure_cache_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(cache_dir())
}

/// Returns the directory used for session log files and Export Logs output.
///
/// Standard builds use `<temp>/comfyui-tui-launcher/`; portable builds use
/// `<exe_dir>/local_data/logs/` so logs travel with the binary.
pub fn logs_dir() -> PathBuf {
    #[cfg(feature = "portable")]
    {
        return portable_root().join("logs");
    }
    #[cfg(not(feature = "portable"))]
    {
        std::env::temp_dir().join(APP_DIR)
    }
}

/// Convenience accessor for the standard directory layout of a ComfyUI
/// installation rooted at `root`.
pub struct ComfyDirs {
    /// Root directory of the ComfyUI installation.
    pub root: PathBuf,
}

impl ComfyDirs {
    /// Constructs a `ComfyDirs` rooted at the given path.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }
    /// Returns the `custom_nodes` subdirectory.
    pub fn custom_nodes(&self) -> PathBuf {
        self.root.join("custom_nodes")
    }
    /// Returns the `input` subdirectory.
    pub fn input(&self) -> PathBuf {
        self.root.join("input")
    }
    /// Returns the `output` subdirectory.
    pub fn output(&self) -> PathBuf {
        self.root.join("output")
    }
    /// Returns the path to `main.py`.
    pub fn main_py(&self) -> PathBuf {
        self.root.join("main.py")
    }
    /// Returns whether the root looks like a valid ComfyUI installation.
    pub fn is_valid(&self) -> bool {
        self.main_py().is_file()
    }
}
