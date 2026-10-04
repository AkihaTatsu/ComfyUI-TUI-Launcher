//! Pip install helpers used by the extension install / update flows.

use crate::core::process::{run_logged, Cmd};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const NODE_POST_INSTALL_HELPER: &str = include_str!("../../assets/python/node_post_install.py");

/// Implementation selected for a custom-node post-install lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostInstallBackend {
    Manager,
    BuiltIn,
}

/// Stage at which custom-node post-install processing failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostInstallStage {
    Bootstrap,
    Requirements,
    InstallScript,
    Repair,
}

impl PostInstallStage {
    /// Stable task-result label used by the failure popup.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bootstrap => "post-install bootstrap",
            Self::Requirements => "pip install",
            Self::InstallScript => "install.py",
            Self::Repair => "dependency repair",
        }
    }
}

/// Structured failure returned by custom-node post-install processing.
#[derive(Debug, Clone)]
pub struct PostInstallFailure {
    pub backend: Option<PostInstallBackend>,
    pub stage: PostInstallStage,
    pub message: String,
}

impl PostInstallFailure {
    /// User-facing detail that identifies which implementation failed.
    pub fn detail(&self, context: &str) -> String {
        let backend = match self.backend {
            Some(PostInstallBackend::Manager) => "ComfyUI-Manager",
            Some(PostInstallBackend::BuiltIn) => "built-in fallback",
            None => "post-install helper",
        };
        format!("{context}; {backend}: {}; see the task log", self.message)
    }
}

#[derive(Debug, Deserialize)]
struct HelperStatus {
    state: String,
    backend: String,
    stage: String,
    #[serde(default)]
    message: String,
}

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

/// Runs the complete custom-node post-install lifecycle.
///
/// An enabled ComfyUI-Manager is used when available. The embedded
/// compatibility backend is selected only when Manager cannot finish loading
/// before it starts node work. Both backends process dependencies before
/// executing `install.py` from the node root.
pub fn install_custom_node(
    python: &Path,
    comfy_root: &Path,
    repo: &Path,
    mut env: HashMap<String, String>,
) -> std::result::Result<(), PostInstallFailure> {
    if !repo.join("requirements.txt").is_file() && !repo.join("install.py").is_file() {
        return Ok(());
    }
    if python.as_os_str().is_empty() {
        return Err(PostInstallFailure {
            backend: None,
            stage: PostInstallStage::Bootstrap,
            message: "Python interpreter is not configured".into(),
        });
    }

    insert_if_unset(&mut env, "COMFYUI_PATH", comfy_root);
    insert_if_unset(&mut env, "COMFYUI_FOLDERS_BASE_PATH", comfy_root);
    let status_path = helper_status_path();
    let status_arg = status_path.display().to_string();
    let root_arg = comfy_root.display().to_string();
    let repo_arg = repo.display().to_string();
    let process_result = run_logged(
        "post-install",
        Cmd::new(python)
            .arg("-c")
            .arg(NODE_POST_INSTALL_HELPER)
            .arg(status_arg)
            .arg(root_arg)
            .arg(repo_arg)
            .envs(env)
            .current_dir(repo)
            .display_args(format!("-c <node-post-install-helper> {}", repo.display())),
    );

    let status = std::fs::read_to_string(&status_path)
        .ok()
        .and_then(|text| serde_json::from_str::<HelperStatus>(&text).ok());
    let _ = std::fs::remove_file(&status_path);

    match (status, process_result) {
        (Some(status), Ok(true)) if status.state == "completed" => Ok(()),
        (Some(status), process_result) if status.state == "completed" => Err(PostInstallFailure {
            backend: parse_backend(&status.backend),
            stage: PostInstallStage::Repair,
            message: match process_result {
                Ok(false) => "helper reported completion but exited unsuccessfully".into(),
                Err(error) => format!("helper reported completion but process failed: {error}"),
                Ok(true) => unreachable!(),
            },
        }),
        (Some(status), _) => Err(PostInstallFailure {
            backend: parse_backend(&status.backend),
            stage: parse_stage(&status.stage),
            message: if status.message.is_empty() {
                format!("post-install helper ended in state {}", status.state)
            } else {
                status.message
            },
        }),
        (None, process_result) => Err(PostInstallFailure {
            backend: None,
            stage: PostInstallStage::Bootstrap,
            message: match process_result {
                Ok(true) => "post-install helper produced no result".into(),
                Ok(false) => "post-install helper failed before producing a result".into(),
                Err(error) => format!("could not start post-install helper: {error}"),
            },
        }),
    }
}

fn insert_if_unset(env: &mut HashMap<String, String>, key: &str, value: &Path) {
    if !env.contains_key(key) && std::env::var_os(key).is_none() {
        env.insert(key.into(), value.display().to_string());
    }
}

fn helper_status_path() -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "comfyui-tui-post-install-{}-{nonce}.json",
        std::process::id()
    ))
}

fn parse_backend(value: &str) -> Option<PostInstallBackend> {
    match value {
        "manager" => Some(PostInstallBackend::Manager),
        "builtin" => Some(PostInstallBackend::BuiltIn),
        _ => None,
    }
}

fn parse_stage(value: &str) -> PostInstallStage {
    match value {
        "requirements" => PostInstallStage::Requirements,
        "install_script" => PostInstallStage::InstallScript,
        "repair" => PostInstallStage::Repair,
        _ => PostInstallStage::Bootstrap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "comfyui-tui-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn available_python() -> Option<&'static str> {
        ["python3", "python"].into_iter().find(|python| {
            Command::new(python)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    fn write_fake_manager(comfy_root: &Path, execute_body: &str) {
        let manager = comfy_root.join("custom_nodes/ComfyUI-Manager");
        std::fs::create_dir_all(manager.join("glob")).unwrap();
        std::fs::write(manager.join("glob/manager_core.py"), "").unwrap();
        let cli = format!(
            r#"
class DummyFixer:
    def __init__(self, *args):
        pass
    def fix_broken(self):
        pass

class ManagerUtil:
    PIPFixer = DummyFixer
    @staticmethod
    def get_installed_packages():
        return {{}}

class ManagerFuncs:
    def run_script(self, command, cwd="."):
        return 0

class Core:
    manager_funcs = ManagerFuncs()
    manager_files_path = "unused"

class UnifiedManager:
    def execute_install_script(self, url, repo_path, instant_execution=False):
{execute_body}

manager_util = ManagerUtil()
core = Core()
unified_manager = UnifiedManager()
"#
        );
        std::fs::write(manager.join("cm-cli.py"), cli).unwrap();
    }

    #[test]
    fn status_values_map_to_task_stages() {
        assert_eq!(parse_backend("manager"), Some(PostInstallBackend::Manager));
        assert_eq!(parse_backend("builtin"), Some(PostInstallBackend::BuiltIn));
        assert_eq!(parse_stage("requirements"), PostInstallStage::Requirements);
        assert_eq!(
            parse_stage("install_script"),
            PostInstallStage::InstallScript
        );
        assert_eq!(parse_stage("repair"), PostInstallStage::Repair);
        assert_eq!(parse_stage("unexpected"), PostInstallStage::Bootstrap);
    }

    #[test]
    fn no_post_install_files_need_no_python() {
        let repo = unique_temp_dir("no-post-install");
        std::fs::create_dir_all(&repo).unwrap();
        let result = install_custom_node(Path::new(""), Path::new("/comfy"), &repo, HashMap::new());
        assert!(result.is_ok());
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn post_install_files_require_a_python_interpreter() {
        let repo = unique_temp_dir("needs-python");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("install.py"), "").unwrap();
        let error = install_custom_node(Path::new(""), Path::new("/comfy"), &repo, HashMap::new())
            .unwrap_err();
        assert_eq!(error.stage, PostInstallStage::Bootstrap);
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn enabled_manager_is_preferred_for_post_install() {
        let Some(python) = available_python() else {
            return;
        };
        let root = unique_temp_dir("manager-success");
        let repo = root.join("custom_nodes/example");
        let marker = root.join("manager-ran");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("install.py"),
            "raise RuntimeError('must not run')\n",
        )
        .unwrap();
        write_fake_manager(
            &root,
            "        import os\n        with open(os.environ['FAKE_MANAGER_MARKER'], 'w', encoding='utf-8') as stream:\n            stream.write(repo_path)\n        return True",
        );
        let env = HashMap::from([(
            "FAKE_MANAGER_MARKER".to_string(),
            marker.display().to_string(),
        )]);

        let result = install_custom_node(Path::new(python), &root, &repo, env);

        assert!(result.is_ok());
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            repo.display().to_string()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manager_failure_after_start_does_not_run_builtin_fallback() {
        let Some(python) = available_python() else {
            return;
        };
        let root = unique_temp_dir("manager-failure");
        let repo = root.join("custom_nodes/example");
        let manager_marker = root.join("manager-ran");
        let fallback_marker = root.join("fallback-ran");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("install.py"),
            "import os\nopen(os.environ['FAKE_FALLBACK_MARKER'], 'w').close()\n",
        )
        .unwrap();
        write_fake_manager(
            &root,
            "        import os\n        open(os.environ['FAKE_MANAGER_MARKER'], 'w').close()\n        raise RuntimeError('simulated Manager failure')",
        );
        let env = HashMap::from([
            (
                "FAKE_MANAGER_MARKER".to_string(),
                manager_marker.display().to_string(),
            ),
            (
                "FAKE_FALLBACK_MARKER".to_string(),
                fallback_marker.display().to_string(),
            ),
        ]);

        let error = install_custom_node(Path::new(python), &root, &repo, env).unwrap_err();

        assert_eq!(error.backend, Some(PostInstallBackend::Manager));
        assert!(manager_marker.is_file());
        assert!(!fallback_marker.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
