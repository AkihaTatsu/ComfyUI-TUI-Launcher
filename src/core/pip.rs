//! Pip install helpers used by the extension install / update flows.

use crate::core::process::{run_logged, Cmd};
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

/// Installs `requirements.txt` from `repo` using `python -m pip install`.
///
/// Returns `Ok(true)` when the file is missing (nothing to do) or when the
/// install succeeds, and `Ok(false)` on a non-zero pip exit.
pub fn install_requirements(
    python: &Path,
    repo: &Path,
    env: HashMap<String, String>,
) -> Result<bool> {
    let req = repo.join("requirements.txt");
    if !req.is_file() {
        return Ok(true);
    }
    run_logged(
        "pip",
        Cmd::new(python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("-r")
            .arg(req.display().to_string())
            .envs(env),
    )
}
