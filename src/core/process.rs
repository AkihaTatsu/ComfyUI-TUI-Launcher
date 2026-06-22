//! Spawn child processes and stream their output to the log bus.

use crate::core::log_bus;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// Crash auto-restart tuning, all in seconds/counts. Only consulted when the
/// supervisor path is taken (i.e. the launcher-settings toggle is on).
pub struct RestartPolicy {
    /// Seconds to wait before each restart.
    pub delay_secs: u64,
    /// Sliding window over which crashes are counted.
    pub window_secs: u64,
    /// If crashes within the window exceed this, the supervisor gives up.
    pub max_fails: u32,
}

/// Set by our signal / console-control handler when the user or terminal asks
/// the launcher to stop (Ctrl+C, SIGTERM/SIGHUP, window close, …). Read by the
/// supervisor loop to decide "stop" vs "restart".
static USER_STOP: AtomicBool = AtomicBool::new(false);

/// How an exited ComfyUI run is classified. The Manager-reboot case is handled
/// separately in the loop (it needs filesystem access), so this stays pure and
/// unit-testable.
#[derive(Debug, PartialEq, Eq)]
enum ExitClass {
    /// Intentional shutdown — do NOT restart.
    Stopped,
    /// Unexpected crash — restart (subject to loop protection).
    Crash,
}

/// Signals that mean "the user / terminal asked it to stop", never a crash.
/// Hard-coded numbers (identical on Linux and macOS) so the classifier is pure
/// and testable on any platform: SIGHUP=1, SIGINT=2, SIGQUIT=3, SIGTERM=15.
fn is_stop_signal(sig: i32) -> bool {
    matches!(sig, 1 | 2 | 3 | 15)
}

/// Decides whether an exited child was stopped on purpose or crashed.
///
/// `success` is the process's `ExitStatus::success()`, `term_signal` is the
/// Unix terminating signal if any (`None` on Windows or for a normal exit), and
/// `user_stop` is whether our handler observed a stop request.
fn classify_exit(success: bool, term_signal: Option<i32>, user_stop: bool) -> ExitClass {
    if user_stop || success {
        return ExitClass::Stopped;
    }
    if let Some(sig) = term_signal {
        if is_stop_signal(sig) {
            return ExitClass::Stopped;
        }
    }
    ExitClass::Crash
}

/// Crash-loop protection: returns true when the number of crashes recorded
/// within `window` of `now` exceeds `max`. `crashes` and `now` are durations
/// measured from a common start instant so this is pure and testable.
fn loop_protection_tripped(
    crashes: &[Duration],
    now: Duration,
    window: Duration,
    max: u32,
) -> bool {
    let recent = crashes
        .iter()
        .filter(|&&t| now.checked_sub(t).is_some_and(|age| age <= window))
        .count();
    recent as u32 > max
}

/// Installs handlers so a stop request flips [`USER_STOP`] instead of killing
/// the launcher outright, letting the supervisor reap the child and decide
/// whether to restart. The handlers only set an atomic flag (async-signal-safe).
fn install_stop_handlers() {
    #[cfg(unix)]
    {
        use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
        extern "C" fn on_signal(_sig: i32) {
            USER_STOP.store(true, Ordering::SeqCst);
        }
        let action = SigAction::new(
            SigHandler::Handler(on_signal),
            SaFlags::empty(),
            SigSet::empty(),
        );
        for sig in [
            Signal::SIGINT,
            Signal::SIGTERM,
            Signal::SIGHUP,
            Signal::SIGQUIT,
        ] {
            // Safety: the handler only performs an atomic store.
            unsafe {
                let _ = sigaction(sig, &action);
            }
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        unsafe extern "system" fn on_ctrl(_ctrl_type: u32) -> i32 {
            USER_STOP.store(true, Ordering::SeqCst);
            1 // TRUE: handled (and stops the default "terminate launcher" action)
        }
        // Safety: registering a console control handler with a static fn.
        unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }
}

/// Returns true and consumes the `__COMFY_CLI_SESSION__` `.reboot` marker if
/// ComfyUI-Manager requested an explicit restart. We do NOT set that env var
/// ourselves (so Manager normally restarts in-process via `os.execv`, fully
/// transparent to us); this guard only fires if something else in the user's
/// environment opted into the marker convention.
fn manager_reboot_requested() -> bool {
    if let Ok(session) = std::env::var("__COMFY_CLI_SESSION__") {
        let marker = PathBuf::from(format!("{session}.reboot"));
        if marker.exists() {
            let _ = std::fs::remove_file(&marker);
            return true;
        }
    }
    false
}

/// Prints a supervisor status line to the user's terminal (the TUI is already
/// torn down at this point) and flushes so it is not swallowed.
fn notice(line: &str) {
    use std::io::Write;
    println!("{line}");
    let _ = std::io::stdout().flush();
}

/// Waits for `child`, but cooperatively honours [`USER_STOP`]: terminal signals
/// already reach the child, so we first let it exit on its own; only if it
/// lingers do we escalate to a graceful then a hard kill (covers a stop request
/// — e.g. `kill <launcher_pid>` — that reached only the launcher).
fn wait_or_stop(child: &mut std::process::Child) -> std::io::Result<std::process::ExitStatus> {
    let mut stop_since: Option<Instant> = None;
    let mut escalated = false;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if USER_STOP.load(Ordering::SeqCst) {
            let elapsed = stop_since.get_or_insert_with(Instant::now).elapsed();
            if elapsed > Duration::from_secs(5) && !escalated {
                escalated = true;
                #[cfg(unix)]
                {
                    use nix::sys::signal::{kill, Signal};
                    use nix::unistd::Pid;
                    let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM);
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }
            }
            if elapsed > Duration::from_secs(10) {
                let _ = child.kill();
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Supervises ComfyUI: spawns it as a child (inheriting stdio so its console is
/// shown exactly as before), and on an *unexpected crash* relaunches it with the
/// same `python`, `args`, and `env`. Intentional shutdowns (Ctrl+C → exit 0,
/// window/terminal close, `kill`) and clean exits do not restart. Crash-loop
/// protection bounds rapid failures per [`RestartPolicy`]. Does not return.
pub fn supervise_comfyui_and_exit(
    python: &Path,
    comfy_dir: &Path,
    args: Vec<String>,
    env: HashMap<String, String>,
    policy: RestartPolicy,
) -> ! {
    use crate::core::i18n;

    let main_py = comfy_dir.join("main.py");
    let mut full: Vec<String> = vec![main_py.display().to_string()];
    full.extend(args);
    let _ = std::env::set_current_dir(comfy_dir);

    install_stop_handlers();
    notice(&i18n::t("restart_supervisor_banner"));

    let started = Instant::now();
    let window = Duration::from_secs(policy.window_secs);
    let mut crashes: Vec<Duration> = Vec::new();

    loop {
        let mut cmd = Command::new(python);
        cmd.args(&full).current_dir(comfy_dir);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        log_bus::push("launch", format!("spawn python {}", full.join(" ")));

        // A spawn failure is treated like a crash so a transient problem can
        // recover, while a permanent one trips loop protection and gives up.
        let exit_code: i32 = match cmd.spawn() {
            Ok(mut child) => {
                let status = match wait_or_stop(&mut child) {
                    Ok(s) => s,
                    Err(e) => {
                        notice(&format!("wait failed: {e}"));
                        let _ = child.kill();
                        std::process::exit(1);
                    }
                };

                if USER_STOP.load(Ordering::SeqCst) {
                    notice(&i18n::t("restart_stopped"));
                    std::process::exit(status.code().unwrap_or(0));
                }
                if manager_reboot_requested() {
                    // Explicit Manager restart: relaunch immediately, not a crash.
                    continue;
                }

                #[cfg(unix)]
                let term_signal = std::os::unix::process::ExitStatusExt::signal(&status);
                #[cfg(not(unix))]
                let term_signal: Option<i32> = None;

                match classify_exit(status.success(), term_signal, false) {
                    ExitClass::Stopped => {
                        notice(&i18n::t("restart_stopped"));
                        std::process::exit(status.code().unwrap_or(0));
                    }
                    ExitClass::Crash => status.code().unwrap_or(1),
                }
            }
            Err(e) => {
                notice(&format!("spawn failed: {e}"));
                -1
            }
        };

        // Record the crash and enforce loop protection.
        let now = started.elapsed();
        crashes.retain(|&t| now.checked_sub(t).is_some_and(|age| age <= window));
        crashes.push(now);
        if loop_protection_tripped(&crashes, now, window, policy.max_fails) {
            notice(&i18n::t_args(
                "restart_giveup",
                &[
                    ("n", &crashes.len().to_string()),
                    ("window", &policy.window_secs.to_string()),
                ],
            ));
            std::process::exit(1);
        }

        notice(&i18n::t_args(
            "restart_notice",
            &[
                ("code", &exit_code.to_string()),
                ("delay", &policy.delay_secs.to_string()),
                ("n", &crashes.len().to_string()),
                ("max", &policy.max_fails.to_string()),
            ],
        ));
        std::thread::sleep(Duration::from_secs(policy.delay_secs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_clean_exit_is_stopped() {
        // ComfyUI catches KeyboardInterrupt and exits 0.
        assert_eq!(classify_exit(true, None, false), ExitClass::Stopped);
    }

    #[test]
    fn user_stop_flag_overrides_nonzero() {
        assert_eq!(classify_exit(false, Some(15), true), ExitClass::Stopped);
    }

    #[test]
    fn terminating_stop_signals_are_not_crashes() {
        for sig in [1, 2, 3, 15] {
            assert_eq!(classify_exit(false, Some(sig), false), ExitClass::Stopped);
        }
    }

    #[test]
    fn nonzero_exit_and_fatal_signals_are_crashes() {
        assert_eq!(classify_exit(false, None, false), ExitClass::Crash); // exit 1
        for sig in [9 /*KILL*/, 11 /*SEGV*/, 6 /*ABRT*/] {
            assert_eq!(classify_exit(false, Some(sig), false), ExitClass::Crash);
        }
    }

    #[test]
    fn loop_protection_trips_only_past_max_within_window() {
        let window = Duration::from_secs(60);
        // 5 crashes, max 5 → not tripped (must exceed).
        let five: Vec<Duration> = (0..5).map(Duration::from_secs).collect();
        assert!(!loop_protection_tripped(
            &five,
            Duration::from_secs(5),
            window,
            5
        ));
        // 6 crashes within window, max 5 → tripped.
        let six: Vec<Duration> = (0..6).map(Duration::from_secs).collect();
        assert!(loop_protection_tripped(
            &six,
            Duration::from_secs(6),
            window,
            5
        ));
    }

    #[test]
    fn loop_protection_ignores_crashes_outside_window() {
        let window = Duration::from_secs(10);
        // Six old crashes long ago + now; only the recent one counts.
        let mut crashes: Vec<Duration> = (0..6).map(Duration::from_secs).collect();
        crashes.push(Duration::from_secs(100));
        assert!(!loop_protection_tripped(
            &crashes,
            Duration::from_secs(100),
            window,
            5
        ));
    }
}
