#!/usr/bin/env python3
"""Linux PTY regression test; never reads or changes tmux sessions/configuration.

Run with python3 scripts/test-environment-activation.py [--features portable].
"""

import errno
import json
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import subprocess
import sys
import tempfile
import time


PROBE = "screens::main_launcher::tests::environment_activation_integration_probe"
PROMPT = b"ACTIVATION_TEST_READY> "
ROOT = Path(__file__).resolve().parent.parent


class Terminal:
    def __init__(self, cwd, env):
        self.buffer = b""
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd)
            os.execve(shutil.which("bash"), ["bash", "--noprofile", "--norc"], env)
        self.expect(PROMPT)
        self.command("stty -echo")
        self.expect(PROMPT)

    def command(self, command):
        os.write(self.fd, command.encode() + b"\n")

    def expect(self, marker, timeout=10):
        deadline = time.monotonic() + timeout
        while marker not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.fd], [], [], remaining)[0]:
                raise AssertionError(f"Timed out waiting for {marker!r}: {self.buffer!r}")
            try:
                data = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                data = b""
            if not data:
                raise AssertionError(f"Terminal closed before {marker!r}: {self.buffer!r}")
            self.buffer += data
        before, self.buffer = self.buffer.split(marker, 1)
        return before

    def close(self):
        # Closing this test's PTY hangs up only its own foreground processes.
        os.close(self.fd)
        os.waitpid(self.pid, 0)


def children(pid):
    result = subprocess.run(
        ["ps", "-o", "pid=", "--ppid", str(pid)], capture_output=True, text=True
    )
    assert result.returncode in (0, 1), result.stderr
    return [int(value) for value in result.stdout.split()]


def only_child(pid):
    found = children(pid)
    assert len(found) == 1, (pid, found)
    return found[0]


def assert_launcher(pid, executable):
    assert Path(f"/proc/{pid}/exe").resolve() == executable
    # This is the command a direct-child process saver records.
    argv = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
    assert argv[0] == os.fsencode(executable), argv


def exercise(executable, shell, directory):
    first, second = directory / "environment one", directory / "environment two"
    for root in (first, second):
        (root / "bin").mkdir(parents=True)
        (root / "bin" / "activate").touch()
    wrapper = directory / "test shell"
    args = [shell, "-df"] if Path(shell).name == "zsh" else [shell, "--noprofile", "--norc"]
    wrapper.write_text(
        "#!/bin/sh\nexport PS1=" + shlex.quote(PROMPT.decode())
        + "\nexec " + shlex.join(args) + "\n"
    )
    wrapper.chmod(0o700)
    env = os.environ.copy()
    env.pop("VIRTUAL_ENV", None)
    env.update(
        HOME=str(directory), ZDOTDIR=str(directory), HISTFILE="/dev/null",
        SHELL=str(wrapper), PS1=PROMPT.decode(), TERM="xterm-256color",
        PYTHONHOME="must-be-removed", COMFYUI_ACTIVATION_TEST_LANGUAGE="en-US",
        COMFYUI_ACTIVATION_TEST_PYTHON=str(first / "bin" / "python"),
    )
    probe = shlex.join([str(executable), PROBE, "--ignored", "--exact", "--nocapture"])
    terminal = Terminal(directory, env)
    try:
        terminal.command(probe + "; printf 'FIRST_STATUS:%s\\n' \"$?\"")
        terminal.expect(PROMPT)
        launcher = only_child(terminal.pid)
        assert_launcher(launcher, executable)
        active_shell = only_child(launcher)
        terminal.command(
            "printf 'ENV:%s|%s|%s|%s\\n' "
            '"$VIRTUAL_ENV" "${PATH%%:*}" "${PYTHONHOME-unset}" "$PWD"'
        )
        terminal.expect(f"ENV:{first}|{first / 'bin'}|unset|{directory}".encode())
        terminal.expect(PROMPT)

        for _ in range(3):
            terminal.command(probe)
            terminal.expect(b"ENVIRONMENT_ALREADY_ACTIVE")
            terminal.expect(PROMPT)
            assert children(terminal.pid) == [launcher]
            assert children(launcher) == [active_shell]
            assert children(active_shell) == []

        # Ctrl+C at the prompt and during a command must not end the launcher.
        os.write(terminal.fd, b"\x03")
        terminal.expect(PROMPT)
        terminal.command("printf 'SLEEP_STARTED\\n'; sleep 30")
        terminal.expect(b"SLEEP_STARTED\r\n")
        deadline = time.monotonic() + 5
        while not (
            (running := children(active_shell))
            and os.tcgetpgrp(terminal.fd) == running[0]
        ):
            assert time.monotonic() < deadline, "sleep did not acquire the terminal"
            time.sleep(0.01)
        os.write(terminal.fd, b"\x03")
        terminal.expect(PROMPT)
        assert_launcher(launcher, executable)

        terminal.command(
            "COMFYUI_ACTIVATION_TEST_PYTHON=" + shlex.quote(str(second / "bin" / "python"))
            + " " + probe + "; printf 'SECOND_STATUS:%s\\n' \"$?\""
        )
        terminal.expect(PROMPT)
        nested_launcher = only_child(active_shell)
        assert_launcher(nested_launcher, executable)
        only_child(nested_launcher)
        terminal.command("printf 'SECOND_ENV:%s\\n' \"$VIRTUAL_ENV\"; exit 7")
        terminal.expect(f"SECOND_ENV:{second}".encode())
        terminal.expect(b"SECOND_STATUS:7")
        terminal.expect(PROMPT)
        assert children(active_shell) == []
        terminal.command("printf 'RESTORED_ENV:%s\\n' \"$VIRTUAL_ENV\"; exit 0")
        terminal.expect(f"RESTORED_ENV:{first}".encode())
        terminal.expect(b"FIRST_STATUS:0")
        terminal.expect(PROMPT)
        assert children(terminal.pid) == []

        # Unix signal termination is reported as 128 + signal number.
        terminal.command(probe + "; printf 'SIGNAL_STATUS:%s\\n' \"$?\"")
        terminal.expect(PROMPT)
        terminal.command("kill -KILL $$")
        terminal.expect(b"SIGNAL_STATUS:137")
        terminal.expect(PROMPT)

        for language, message in [
            ("zh-CN", "启动或等待环境终端失败："),
            ("en-US", "Failed to start or wait for the environment shell:"),
            ("en-UK", "Failed to start or wait for the environment shell:"),
        ]:
            terminal.command(
                "SHELL=/nonexistent/activation-test-shell COMFYUI_ACTIVATION_TEST_LANGUAGE="
                + language + " " + probe + "; printf 'ERROR_STATUS:%s\\n' \"$?\""
            )
            terminal.expect(message.encode())
            terminal.expect(b"ERROR_STATUS:1")
            terminal.expect(PROMPT)
    finally:
        terminal.close()
    print(f"{Path(shell).name}: environment activation checks passed", flush=True)


def main():
    build = subprocess.run(
        ["cargo", "test", "--offline", "--no-run", "--message-format=json", *sys.argv[1:]],
        cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True,
    )
    executable = next(
        Path(item["executable"]).resolve()
        for line in build.stdout.splitlines()
        if (item := json.loads(line)).get("executable") and item.get("profile", {}).get("test")
    )
    for name in ("zsh", "bash"):
        shell = shutil.which(name)
        if shell is None:
            raise RuntimeError(f"Required test shell is unavailable: {name}")
        with tempfile.TemporaryDirectory(prefix="activation-test-") as temporary:
            exercise(executable, shell, Path(temporary))


if __name__ == "__main__":
    main()
