//! Spawn child processes and stream their output to the log bus.

use crate::core::log_bus;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Builder for a child process invocation.
pub struct Cmd {
    /// Path to the executable.
    pub program: PathBuf,
    /// Arguments passed to the executable.
    pub args: Vec<String>,
    /// Optional working directory.
    pub cwd: Option<PathBuf>,
    /// Additional environment variables.
    pub env: HashMap<String, String>,
}

impl Cmd {
    /// Constructs a new `Cmd` for the given program.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec![],
            cwd: None,
            env: HashMap::new(),
        }
    }
    /// Appends one argument.
    pub fn arg(mut self, s: impl Into<String>) -> Self {
        self.args.push(s.into());
        self
    }
    /// Merges the given environment variables into the command.
    pub fn envs(mut self, e: HashMap<String, String>) -> Self {
        self.env.extend(e);
        self
    }
}

/// Runs a command and streams its combined output to the log bus line by
/// line as it is produced.
///
/// Returns whether the command exited successfully. Streaming matters for
/// long-running commands such as `pip install` or `git fetch` so the user
/// sees progress rather than a single dump at the end.
pub fn run_logged(source: &str, c: Cmd) -> Result<bool> {
    use std::io::{BufRead, BufReader};
    use std::thread;

    log_bus::push(
        source,
        format!("$ {} {}", c.program.display(), c.args.join(" ")),
    );
    let mut cmd = Command::new(&c.program);
    cmd.args(&c.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = &c.cwd {
        cmd.current_dir(d);
    }
    for (k, v) in &c.env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let src_o = source.to_string();
    let src_e = source.to_string();

    let h_out = stdout.map(|s| {
        thread::spawn(move || {
            let reader = BufReader::new(s);
            for line in reader.lines().map_while(|l| l.ok()) {
                log_bus::push(&src_o, line);
            }
        })
    });
    let h_err = stderr.map(|s| {
        thread::spawn(move || {
            let reader = BufReader::new(s);
            for line in reader.lines().map_while(|l| l.ok()) {
                log_bus::push(&src_e, format!("err: {line}"));
            }
        })
    });

    let status = child.wait()?;
    if let Some(h) = h_out {
        let _ = h.join();
    }
    if let Some(h) = h_err {
        let _ = h.join();
    }
    log_bus::push(source, format!("(exit {})", status.code().unwrap_or(-1)));
    Ok(status.success())
}

/// Replaces this process with an interactive shell whose environment has
/// the given virtualenv activated.
///
/// `VIRTUAL_ENV` is set to the venv root, `PATH` is prepended with the
/// venv's `bin` or `Scripts` directory, and `PYTHONHOME` is cleared. On
/// Unix the launcher `execvp`s into `$SHELL`; on Windows a new `%COMSPEC%`
/// process inherits the parent console. Does not return on success.
pub fn activate_env_and_exit(venv_root: &Path) -> ! {
    use std::ffi::OsString;
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    let venv_bin = venv_root.join(bin);
    let path_sep = if cfg!(windows) { ";" } else { ":" };
    let mut new_path: OsString = venv_bin.clone().into_os_string();
    if let Some(cur) = std::env::var_os("PATH") {
        new_path.push(path_sep);
        new_path.push(cur);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = Command::new(&shell);
        cmd.env("VIRTUAL_ENV", venv_root)
            .env("PATH", &new_path)
            .env_remove("PYTHONHOME");
        let err = cmd.exec();
        eprintln!("exec shell failed: {err}");
        std::process::exit(1);
    }

    #[cfg(windows)]
    {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        let mut cmd = Command::new(&shell);
        cmd.env("VIRTUAL_ENV", venv_root)
            .env("PATH", &new_path)
            .env_remove("PYTHONHOME");
        match cmd.spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("spawn shell failed: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Launches ComfyUI by replacing this process (Unix) or by spawning a
/// console-inheriting child and exiting (Windows).
///
/// Does not return on success.
pub fn launch_comfyui_and_exit(
    python: &Path,
    comfy_dir: &Path,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> ! {
    let main_py = comfy_dir.join("main.py");
    let mut full: Vec<String> = vec![main_py.display().to_string()];
    full.extend(args);

    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(python);
        cmd.args(&full).current_dir(comfy_dir);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        // chdir to the ComfyUI directory so relative paths resolve.
        let _ = std::env::set_current_dir(comfy_dir);
        let err = cmd.exec();
        eprintln!("exec failed: {err}");
        let _ = CString::new(""); // silence unused warning on some platforms
        std::process::exit(1);
    }

    #[cfg(windows)]
    {
        let mut cmd = Command::new(python);
        cmd.args(&full).current_dir(comfy_dir);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        // Spawn and exit so the child inherits the console on Windows.
        match cmd.spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("spawn failed: {e}");
                std::process::exit(1);
            }
        }
    }
}
