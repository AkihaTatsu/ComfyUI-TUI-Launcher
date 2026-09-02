# ADR 0001: Windows TUI responsiveness and update correctness

- Status: Accepted
- Date: 2026-08-05

## Context

On Windows Terminal, three failures shared two underlying boundaries:

1. Resize events could arrive in bursts while fixed-size layout code was still
   subtracting from tiny `u16` rectangles. After restoring the window, stale
   hit rectangles and a terminal surface that had not been explicitly cleared
   could leave a black, non-interactive screen.
2. Rendering and frame housekeeping performed repository and Python discovery
   synchronously. One main-screen render could start several child processes;
   extension scans and concurrent `pip` commands amplified Windows process and
   console overhead.
3. Update tasks treated process completion as task success. The recorded
   Windows session showed `git checkout` and `git reset --hard` failing with
   “dubious ownership” (exit 128), followed by `pip` and a success-looking UI
   refresh that displayed the unchanged `HEAD`.

Ratatui updates its viewport size during draw and provides explicit terminal
clear/autoresize operations. Crossterm also documents that resize events may be
batched. Git runtime configuration (`GIT_CONFIG_*`) is process-scoped and can
therefore solve drive-ownership checks without changing the user's global
configuration.

## Decision

### Viewport and event loop

- Treat viewports below 40 by 8 cells as a separate `TooSmall` state.
- In that state, suspend the full layout, discard cached mouse hit rectangles,
  ignore input other than Ctrl+C, and render only a compact message.
- Drain up to 256 queued events, use only the newest resize dimensions, and
  discard mouse coordinates from the same resize batch.
- On recovery, call terminal autoresize and clear before the first full frame.
- Use saturating rectangle arithmetic throughout popup inner geometry.
- Redraw on input, resize, log changes, transient UI state, or active background
  work instead of continuously repainting an idle terminal.

### Work scheduling

- Rendering consumes cached main-screen rows only. Git/Python discovery runs in
  a background snapshot loader on entry, relevant configuration changes, or a
  completed repository mutation.
- Core and extension local scans run in background tasks and duplicate initial
  requests are suppressed.
- Git work in bulk extension operations is bounded (four workers on Windows,
  eight elsewhere). All `pip` installs are serialized because they mutate the
  same Python environment.
- The log bus uses a deque and renderers copy only the visible range or a small
  tail.

### Repository mutation contract

- Every launcher-owned Git subprocess receives ephemeral
  `safe.directory = *` through `GIT_CONFIG_*`. Existing runtime URL mirror
  entries are appended to, not overwritten. The user's Git configuration is
  never modified.
- Checkout and update retain the existing destructive policy: tracked changes
  may be overwritten by `checkout --force` / `reset --hard`; untracked files
  are retained.
- A failed Git stage is a repository failure and skips that repository's pip
  stage. Bulk operations continue with other repositories.
- A successful source mutation followed by a failed pip install or state read
  is a partial failure.
- The displayed row is refreshed from the actual post-operation `HEAD`; command
  intent is never used as the displayed result.
- Task completion is modelled as `Success`, `PartialFailure`, or `Failure`.
  Failures are summarized in a dismissible modal and retain the session-log
  path for full diagnostics.

## Consequences

The UI remains usable after extreme resize sequences, and idle screens no
longer spawn recurring repository processes. Version actions can take the same
amount of network time as before and remain modal by design, but input/rendering
work no longer competes with unbounded child-process creation. The wildcard
safe-directory policy is intentionally limited to launcher subprocesses; it
does not weaken Git's policy for commands run outside the launcher.

The task system carries more explicit state, and mutation closures must return
an honest outcome. This additional bookkeeping is accepted because it prevents
false success and makes partial dependency failures visible.

## References

- [Ratatui `Terminal`](https://docs.rs/ratatui/latest/ratatui/struct.Terminal.html)
- [Crossterm `Event`](https://docs.rs/crossterm/latest/crossterm/event/enum.Event.html)
- [Git configuration scopes](https://git-scm.com/docs/git-config)
