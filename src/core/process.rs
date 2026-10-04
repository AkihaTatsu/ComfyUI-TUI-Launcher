//! Spawn child processes and stream their output to the log bus.

use crate::core::log_bus;
use anyhow::Result;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const HF_FORCE_MIRROR_WRAPPER: &str = include_str!("../../assets/python/hf_force_mirror.py");

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
    /// Optional concise argument rendering for commands whose real arguments
    /// contain large embedded helpers.
    pub display_args: Option<String>,
}

impl Cmd {
    /// Constructs a new `Cmd` for the given program.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec![],
            cwd: None,
            env: HashMap::new(),
            display_args: None,
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
    /// Sets the child working directory.
    pub fn current_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }
    /// Overrides only the argument text written to the task log.
    pub fn display_args(mut self, args: impl Into<String>) -> Self {
        self.display_args = Some(args.into());
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
    use std::thread;

    let displayed_args = c.display_args.clone().unwrap_or_else(|| c.args.join(" "));
    log_bus::push(
        source,
        format!("$ {} {displayed_args}", c.program.display()),
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
            stream_output(s, &src_o, "stdout");
        })
    });
    let h_err = stderr.map(|s| {
        thread::spawn(move || {
            stream_output(s, &src_e, "stderr");
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

fn stream_output<R: Read>(mut reader: R, source: &str, stream: &str) {
    let progress_key = format!("{source}:{stream}");
    let mut pending = Vec::<u8>::new();
    let mut had_progress = false;
    let mut buf = [0u8; 4096];

    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            match b {
                b'\r' => {
                    push_pending_progress(source, &progress_key, &pending);
                    pending.clear();
                    had_progress = true;
                }
                b'\n' => {
                    push_pending_line(source, &progress_key, &pending, had_progress);
                    pending.clear();
                    had_progress = false;
                }
                _ => pending.push(b),
            }
        }
    }

    if !pending.is_empty() {
        push_pending_line(source, &progress_key, &pending, had_progress);
    }
}

fn push_pending_progress(source: &str, key: &str, pending: &[u8]) {
    if pending.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(pending);
    log_bus::push_progress(source, key, text);
}

fn push_pending_line(source: &str, key: &str, pending: &[u8], had_progress: bool) {
    if pending.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(pending);
    if had_progress {
        log_bus::push_progress(source, key, text.as_ref());
    } else {
        log_bus::push(source, text.as_ref());
    }
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
/// console-inheriting child and exiting (Windows). The launched process
/// inherits the launcher's working directory on every platform.
///
/// Does not return on success.
pub fn launch_comfyui_and_exit(
    python: &Path,
    comfy_dir: &Path,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> ! {
    let main_py = comfy_dir.join("main.py");
    let full = comfyui_python_args(&main_py, args, &env);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = comfyui_command(python, &full, &env);
        let err = cmd.exec();
        eprintln!("exec failed: {err}");
        std::process::exit(1);
    }

    #[cfg(windows)]
    {
        let mut cmd = comfyui_command(python, &full, &env);
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

/// Crash auto-restart tuning, all in seconds/counts.
#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    /// Seconds to wait before each restart.
    pub delay_secs: u64,
    /// Sliding window over which crashes are counted.
    pub window_secs: u64,
    /// If crashes within the window exceed this, the supervisor gives up.
    pub max_fails: u32,
}

/// Final result of a supervised ComfyUI run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// ComfyUI exited cleanly or was intentionally stopped.
    Normal { exit_code: i32 },
    /// ComfyUI failed and any configured restart budget was exhausted.
    Crash { exit_code: i32 },
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

/// Restores the process handlers that were active before a supervised run.
struct StopHandlerGuard {
    #[cfg(unix)]
    previous: Vec<(nix::sys::signal::Signal, nix::sys::signal::SigAction)>,
    #[cfg(windows)]
    installed: bool,
}

impl Drop for StopHandlerGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (signal, action) in self.previous.drain(..).rev() {
            // Safety: restore the exact handler returned by `sigaction`.
            unsafe {
                let _ = nix::sys::signal::sigaction(signal, &action);
            }
        }

        #[cfg(windows)]
        if self.installed {
            // Safety: unregister the same static callback installed below.
            unsafe {
                windows_sys::Win32::System::Console::SetConsoleCtrlHandler(
                    Some(on_console_stop),
                    0,
                );
            }
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn on_console_stop(_ctrl_type: u32) -> i32 {
    USER_STOP.store(true, Ordering::SeqCst);
    1 // TRUE: handled (and stops the default "terminate launcher" action)
}

/// Installs handlers so a stop request flips [`USER_STOP`] instead of killing
/// the launcher outright, letting the supervisor reap the child and classify
/// its exit. The returned guard restores the prior handlers before the TUI is
/// entered again.
fn install_stop_handlers() -> StopHandlerGuard {
    USER_STOP.store(false, Ordering::SeqCst);

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
        let mut previous = Vec::new();
        for sig in [
            Signal::SIGINT,
            Signal::SIGTERM,
            Signal::SIGHUP,
            Signal::SIGQUIT,
        ] {
            // Safety: the handler only performs an atomic store.
            unsafe {
                if let Ok(old) = sigaction(sig, &action) {
                    previous.push((sig, old));
                }
            }
        }
        StopHandlerGuard { previous }
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        // Safety: registering a console control handler with a static fn.
        let installed = unsafe { SetConsoleCtrlHandler(Some(on_console_stop), 1) != 0 };
        StopHandlerGuard { installed }
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

/// Waits out a restart delay while still allowing Ctrl+C to cancel it.
fn wait_restart_delay(delay: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < delay {
        if USER_STOP.load(Ordering::SeqCst) {
            return true;
        }
        let remaining = delay.saturating_sub(started.elapsed());
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    USER_STOP.load(Ordering::SeqCst)
}

/// Supervises ComfyUI: spawns it as a child (inheriting stdio so its console is
/// shown exactly as before), classifies its final exit, and optionally retries
/// unexpected crashes. Manager-requested reboots are always immediate and do
/// not consume the crash budget.
pub fn run_comfyui_supervised(
    python: &Path,
    comfy_dir: &Path,
    args: Vec<String>,
    env: HashMap<String, String>,
    restart_policy: Option<RestartPolicy>,
) -> RunOutcome {
    use crate::core::i18n;

    let main_py = comfy_dir.join("main.py");
    let full = comfyui_python_args(&main_py, args, &env);

    let _stop_handlers = install_stop_handlers();
    if restart_policy.is_some() {
        notice(&i18n::t("restart_supervisor_banner"));
    }

    let started = Instant::now();
    let mut crashes: Vec<Duration> = Vec::new();

    loop {
        if USER_STOP.load(Ordering::SeqCst) {
            notice(&i18n::t("restart_stopped"));
            return RunOutcome::Normal { exit_code: 0 };
        }
        let mut cmd = comfyui_command(python, &full, &env);
        log_bus::push("launch", format!("spawn python {}", display_args(&full)));

        // A spawn failure is treated like a crash so a transient problem can
        // recover, while a permanent one trips loop protection and gives up.
        let exit_code = match cmd.spawn() {
            Ok(mut child) => match wait_or_stop(&mut child) {
                Ok(status) => {
                    let user_stop = USER_STOP.load(Ordering::SeqCst);
                    if !user_stop && manager_reboot_requested() {
                        // Explicit Manager restart: relaunch immediately, not a crash.
                        continue;
                    }

                    #[cfg(unix)]
                    let term_signal = std::os::unix::process::ExitStatusExt::signal(&status);
                    #[cfg(not(unix))]
                    let term_signal: Option<i32> = None;

                    match classify_exit(status.success(), term_signal, user_stop) {
                        ExitClass::Stopped => {
                            notice(&i18n::t("restart_stopped"));
                            return RunOutcome::Normal {
                                exit_code: status.code().unwrap_or(0),
                            };
                        }
                        ExitClass::Crash => status.code().unwrap_or(1),
                    }
                }
                Err(e) => {
                    notice(&format!("wait failed: {e}"));
                    let _ = child.kill();
                    let _ = child.wait();
                    if USER_STOP.load(Ordering::SeqCst) {
                        notice(&i18n::t("restart_stopped"));
                        return RunOutcome::Normal { exit_code: 0 };
                    }
                    1
                }
            },
            Err(e) => {
                notice(&format!("spawn failed: {e}"));
                1
            }
        };

        let Some(policy) = restart_policy else {
            return RunOutcome::Crash { exit_code };
        };

        // Record the crash and enforce loop protection.
        let now = started.elapsed();
        let window = Duration::from_secs(policy.window_secs);
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
            return RunOutcome::Crash { exit_code };
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
        if wait_restart_delay(Duration::from_secs(policy.delay_secs)) {
            notice(&i18n::t("restart_stopped"));
            return RunOutcome::Normal { exit_code: 0 };
        }
    }
}

fn comfyui_python_args(
    main_py: &Path,
    args: Vec<String>,
    env: &HashMap<String, String>,
) -> Vec<String> {
    let mut full = if env.contains_key(crate::core::env::HF_FORCE_MIRROR_ENV) {
        vec![
            "-c".to_string(),
            HF_FORCE_MIRROR_WRAPPER.to_string(),
            main_py.display().to_string(),
        ]
    } else {
        vec![main_py.display().to_string()]
    };
    full.extend(args);
    full
}

/// Builds a ComfyUI process without overriding its working directory. This
/// keeps direct launch and crash-restart behaviour identical on all platforms.
fn comfyui_command(python: &Path, args: &[String], env: &HashMap<String, String>) -> Command {
    let mut cmd = Command::new(python);
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd
}

fn display_args(args: &[String]) -> String {
    if args.len() >= 2 && args[0] == "-c" && args[1] == HF_FORCE_MIRROR_WRAPPER {
        let mut out = vec!["-c".to_string(), "<hf-force-mirror-wrapper>".to_string()];
        out.extend(args[2..].iter().cloned());
        out.join(" ")
    } else {
        args.join(" ")
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

    #[test]
    fn comfyui_args_are_plain_without_force_mirror() {
        let got = comfyui_python_args(
            Path::new("main.py"),
            vec!["--listen".into(), "0.0.0.0".into()],
            &HashMap::new(),
        );
        assert_eq!(got, vec!["main.py", "--listen", "0.0.0.0"]);
    }

    #[test]
    fn comfyui_args_use_wrapper_with_force_mirror() {
        let mut env = HashMap::new();
        env.insert(
            crate::core::env::HF_FORCE_MIRROR_ENV.into(),
            "https://hf-mirror.com".into(),
        );
        let got = comfyui_python_args(Path::new("main.py"), vec!["--listen".into()], &env);
        assert_eq!(got[0], "-c");
        assert_eq!(got[1], HF_FORCE_MIRROR_WRAPPER);
        assert_eq!(got[2], "main.py");
        assert_eq!(got[3], "--listen");
    }

    #[test]
    fn comfyui_command_inherits_the_launchers_working_directory() {
        let command = comfyui_command(
            Path::new("/python"),
            &["/comfy/main.py".into(), "--listen".into()],
            &HashMap::new(),
        );

        assert_eq!(command.get_current_dir(), None);
    }

    #[test]
    fn display_args_hides_embedded_wrapper() {
        let got = display_args(&[
            "-c".into(),
            HF_FORCE_MIRROR_WRAPPER.into(),
            "main.py".into(),
        ]);
        assert_eq!(got, "-c <hf-force-mirror-wrapper> main.py");
    }

    #[test]
    fn stderr_progress_replaces_one_log_line_without_an_error_prefix() {
        let source = "progress_replaces_one_log_line";
        stream_output(
            std::io::Cursor::new(b"0%\r50%\r100%\n".to_vec()),
            source,
            "stderr",
        );

        let snap = crate::core::log_bus::snapshot();
        let key = format!("{source}:stderr");
        let got: Vec<_> = snap
            .iter()
            .filter(|line| line.progress_key.as_deref() == Some(key.as_str()))
            .collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, source);
        assert_eq!(got[0].text, "100%");
    }

    #[test]
    fn normal_newlines_append_log_lines() {
        let source = "normal_newlines_append_log_lines";
        stream_output(
            std::io::Cursor::new(b"a\nb\nc\n".to_vec()),
            source,
            "stdout",
        );

        let snap = crate::core::log_bus::snapshot();
        let got: Vec<&str> = snap
            .iter()
            .filter(|line| line.source == source)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(got, vec!["a", "b", "c"]);
        assert!(snap
            .iter()
            .filter(|line| line.source == source)
            .all(|line| line.progress_key.is_none()));
    }

    #[test]
    fn stderr_newlines_are_preserved_without_an_error_prefix() {
        let source = "stderr_newlines_without_error_prefix";
        stream_output(
            std::io::Cursor::new(b"notice\nWARNING: example\nERROR: original\n".to_vec()),
            source,
            "stderr",
        );

        let snap = crate::core::log_bus::snapshot();
        let got: Vec<&str> = snap
            .iter()
            .filter(|line| line.source == source)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(got, vec!["notice", "WARNING: example", "ERROR: original"]);
    }
}
