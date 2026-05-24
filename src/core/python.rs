//! Python interpreter detection and virtualenv inspection.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A Python interpreter discovered on disk, with a display label describing
/// where it was found.
#[derive(Debug, Clone)]
pub struct PythonCandidate {
    /// Absolute path to the interpreter executable.
    pub path: PathBuf,
    /// Human-readable label, for example `python3 (PATH)`.
    pub label: String,
}

fn push(out: &mut Vec<PythonCandidate>, p: PathBuf, src: &str, do_validate: bool) {
    if !p.is_file() {
        return;
    }
    if do_validate && validate(&p).is_none() {
        return;
    }
    if out.iter().any(|c| c.path == p) {
        return;
    }
    let label = format!("{} ({})", p.display(), src);
    out.push(PythonCandidate { path: p, label });
}

/// Returns every Python interpreter candidate the launcher can discover.
///
/// Looks up `python` and `python3` on `PATH`, scans direct subdirectories
/// of `comfy_dir` for virtualenvs whose interpreter passes `--version`,
/// and probes the pyenv shim.
pub fn detect(comfy_dir: Option<&Path>) -> Vec<PythonCandidate> {
    let mut out: Vec<PythonCandidate> = Vec::new();

    for name in ["python3", "python"] {
        if let Ok(found) = which(name) {
            push(&mut out, found, "PATH", false);
        }
    }

    // Scan every direct subdirectory of the ComfyUI root. A folder that
    // contains `bin/python(3)` (Unix) or `Scripts/python(3).exe` (Windows)
    // and whose interpreter passes `--version` is added. Name-agnostic, so
    // `venv`, `.venv`, `.linux_venv`, `conda-env-foo`, and so on are all
    // matched.
    if let Some(root) = comfy_dir {
        if let Ok(rd) = std::fs::read_dir(root) {
            for ent in rd.flatten() {
                let p = ent.path();
                if !p.is_dir() {
                    continue;
                }
                if let Some(bin) = venv_python(&p) {
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("?");
                    let src = format!("env in {name}/");
                    push(&mut out, bin, &src, true);
                }
            }
        }
    }

    // pyenv shim.
    if let Some(home) = dirs::home_dir() {
        let shim = home.join(".pyenv").join("shims").join("python");
        push(&mut out, shim, "pyenv", false);
    }

    out
}

/// Resolves `name` against `PATH`, honouring `PATHEXT` on Windows.
pub fn which(name: &str) -> std::io::Result<PathBuf> {
    let paths = std::env::var_os("PATH")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "PATH unset"))?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE".into())
            .split(';')
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for d in std::env::split_paths(&paths) {
        for ext in &exts {
            let cand = d.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        name.to_string(),
    ))
}

/// Returns the interpreter inside `p` when `p` is a venv directory;
/// otherwise returns `p` unchanged.
///
/// Lets the user paste the venv folder instead of `bin/python`.
pub fn resolve(p: &Path) -> PathBuf {
    if p.is_dir() {
        if let Some(bin) = venv_python(p) {
            return bin;
        }
    }
    p.to_path_buf()
}

fn venv_python(dir: &Path) -> Option<PathBuf> {
    let candidates: [PathBuf; 4] = [
        dir.join("bin").join("python"),
        dir.join("bin").join("python3"),
        dir.join("Scripts").join("python.exe"),
        dir.join("Scripts").join("python3.exe"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Runs `<p> --version` and returns the trimmed output on success.
pub fn validate(p: &Path) -> Option<String> {
    let out = Command::new(p).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = if s.trim().is_empty() {
        String::from_utf8_lossy(&out.stderr).to_string()
    } else {
        s.to_string()
    };
    Some(s.trim().to_string())
}

/// Returns whether the `git` executable is available on `PATH`.
pub fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns the venv root when `python_path` lives inside a virtualenv.
///
/// Detection looks for an `activate` script next to the interpreter
/// (`bin/activate` on Unix, `Scripts/activate.bat` on Windows). Returns
/// `None` for a system Python.
pub fn venv_root(python_path: &Path) -> Option<PathBuf> {
    let parent = python_path.parent()?; // .../venv/bin or .../venv/Scripts
    let root = parent.parent()?; // .../venv
    let activate = if cfg!(windows) {
        root.join("Scripts").join("activate.bat")
    } else {
        root.join("bin").join("activate")
    };
    if activate.is_file() {
        Some(root.to_path_buf())
    } else {
        None
    }
}

/// Returns whether the current shell has the given virtualenv activated.
///
/// Compares `$VIRTUAL_ENV` against `venv_root` after canonicalising both
/// paths.
pub fn currently_activated(venv_root: &Path) -> bool {
    let cur = match std::env::var("VIRTUAL_ENV") {
        Ok(s) if !s.is_empty() => s,
        _ => return false,
    };
    let a = std::fs::canonicalize(std::path::Path::new(&cur)).ok();
    let b = std::fs::canonicalize(venv_root).ok();
    a.is_some() && a == b
}
