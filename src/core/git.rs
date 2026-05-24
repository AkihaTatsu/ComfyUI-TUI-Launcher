//! Thin wrappers around the `git` command-line, with non-interactive
//! safeguards that prevent fetch and clone operations from hanging on a
//! credential or passphrase prompt.

use crate::core::process::{run_logged, Cmd};
use std::collections::HashMap;

/// Merges non-interactive safeguards into the caller-supplied environment.
///
/// Without these, a `git fetch` against an auth-required remote can hang
/// forever waiting for stdin. The injected variables disable prompts and
/// cap SSH connect time so dead remotes fail fast.
///
/// - `GIT_TERMINAL_PROMPT=0` disables git's built-in credential prompt.
/// - `GCM_INTERACTIVE=Never` silences Git Credential Manager popups.
/// - `GIT_ASKPASS` and `SSH_ASKPASS` are emptied and `SSH_ASKPASS_REQUIRE`
///   is set to `never` to suppress GUI askpass helpers.
/// - `GIT_SSH_COMMAND` forces `BatchMode=yes` and a 10 second connect
///   timeout.
fn no_prompt_env(mut env: HashMap<String, String>) -> HashMap<String, String> {
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    env.insert("GCM_INTERACTIVE".into(), "Never".into());
    env.insert("GIT_ASKPASS".into(), String::new());
    env.insert("SSH_ASKPASS".into(), String::new());
    env.insert("SSH_ASKPASS_REQUIRE".into(), "never".into());
    if !env.contains_key("GIT_SSH_COMMAND") {
        env.insert(
            "GIT_SSH_COMMAND".into(),
            "ssh -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new".into(),
        );
    }
    env
}
use anyhow::Result;
use std::path::Path;
use std::process::Command;

/// One commit produced by `git log`.
#[derive(Debug, Clone)]
pub struct Commit {
    /// Short commit hash.
    pub short: String,
    /// Commit subject (first line of the message).
    pub subject: String,
    /// Commit date as `YYYY-MM-DD HH:MM:SS`.
    pub date: String,
}

/// Returns the last `n` commits reachable from `HEAD`, newest first.
pub fn log(repo: &Path, n: usize) -> Result<Vec<Commit>> {
    log_impl(repo, n, false)
}

/// Returns the last `n` commits reachable from any ref (local or remote).
///
/// Includes commits that exist only on `origin/…` and have not been pulled.
pub fn log_all(repo: &Path, n: usize) -> Result<Vec<Commit>> {
    log_impl(repo, n, true)
}

fn log_impl(repo: &Path, n: usize, all: bool) -> Result<Vec<Commit>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).arg("log").arg(format!("-n{n}"));
    if all {
        cmd.arg("--all");
    }
    // Display the commit date (`%cd`) so the column matches the sort key.
    // `--date-order` and `--author-date-order` only constrain parent
    // ordering during topology traversal; they do not produce a strictly
    // time-monotonic listing, so the parsed output is sorted below.
    let out = cmd
        .arg("--pretty=format:%h\x1f%s\x1f%cd")
        .arg("--date=format:%Y-%m-%d %H:%M:%S")
        .output()?;
    if !out.status.success() {
        anyhow::bail!("git log failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits: Vec<Commit> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\x1f');
            Some(Commit {
                short: it.next()?.to_string(),
                subject: it.next()?.to_string(),
                date: it.next()?.to_string(),
            })
        })
        .collect();
    // `YYYY-MM-DD HH:MM:SS` is lexicographically equivalent to chronological
    // order, so a plain string compare yields the correct descending sort.
    commits.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(commits)
}

/// Returns the short hash of the current `HEAD`, or `None` on failure.
pub fn current_commit(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--short")
        .arg("HEAD")
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// Returns the current branch name, or `None` if `HEAD` is detached.
///
/// `rev-parse --abbrev-ref HEAD` returns the literal string `"HEAD"` for a
/// detached head, which is treated as "no branch" so callers can safely
/// use `@{u}` only when this returns `Some`.
pub fn current_branch(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() || name == "HEAD" {
        None
    } else {
        Some(name)
    }
}

/// Returns the number of commits the upstream is ahead of local `HEAD`.
///
/// Network-free; reflects whatever refs the local clone last fetched.
/// Returns 0 when no upstream is configured and the fallback targets are
/// unavailable. The computation tries `HEAD..@{u}` first, then
/// `HEAD..origin/HEAD` for detached heads, then `HEAD..FETCH_HEAD`.
pub fn behind_upstream(repo: &Path) -> u32 {
    for target in ["@{u}", "origin/HEAD", "FETCH_HEAD"] {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("rev-list")
            .arg("--count")
            .arg(format!("HEAD..{target}"))
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                if let Ok(n) = String::from_utf8_lossy(&o.stdout).trim().parse::<u32>() {
                    return n;
                }
            }
        }
    }
    0
}

/// Returns the URL configured for `origin`, or `None` on failure.
pub fn remote_url(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("config")
        .arg("--get")
        .arg("remote.origin.url")
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// Runs `git fetch --all` with non-interactive safeguards applied.
pub fn fetch(repo: &Path, env: std::collections::HashMap<String, String>) -> Result<bool> {
    run_logged(
        "git",
        Cmd::new("git")
            .arg("-C")
            .arg(repo.display().to_string())
            .arg("fetch")
            .arg("--all")
            .envs(no_prompt_env(env)),
    )
}

/// Returns whether the local clone has a shallow-history boundary.
///
/// A shallow clone caps `git log` at the boundary, so listing older
/// versions requires a deepening fetch first.
pub fn is_shallow(repo: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--is-shallow-repository")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Deepens a shallow clone until `is_shallow` reports false.
///
/// Tries `--unshallow` first; if the repo is still shallow afterwards
/// (some mirrors silently refuse `--unshallow`), falls back to a fetch
/// with a very large `--depth`. Returns whether the repository ended up
/// non-shallow.
pub fn deepen_until_full(repo: &Path, env: std::collections::HashMap<String, String>) -> bool {
    if !is_shallow(repo) {
        return true;
    }
    let env = no_prompt_env(env);
    crate::core::log_bus::push("git", "deepening shallow clone (fetch --unshallow)…");
    let _ = run_logged(
        "git",
        Cmd::new("git")
            .arg("-C")
            .arg(repo.display().to_string())
            .arg("fetch")
            .arg("--unshallow")
            .arg("--tags")
            .envs(env.clone()),
    );
    if !is_shallow(repo) {
        return true;
    }
    crate::core::log_bus::push("git", "still shallow — retrying with --depth=2147483647");
    let _ = run_logged(
        "git",
        Cmd::new("git")
            .arg("-C")
            .arg(repo.display().to_string())
            .arg("fetch")
            .arg("--depth=2147483647")
            .arg("--tags")
            .envs(env),
    );
    !is_shallow(repo)
}

/// Checks out `rev`, forcing through local changes to tracked files.
///
/// Many ComfyUI custom nodes write config or cache files into their own
/// repository, which makes a plain `git checkout` fail with "Your local
/// changes would be overwritten". Untracked files are left alone. If
/// `checkout --force` still refuses, falls back to `reset --hard`.
pub fn checkout(
    repo: &Path,
    rev: &str,
    env: std::collections::HashMap<String, String>,
) -> Result<bool> {
    let ok = run_logged(
        "git",
        Cmd::new("git")
            .arg("-C")
            .arg(repo.display().to_string())
            .arg("checkout")
            .arg("--force")
            .arg(rev.to_string())
            .envs(env.clone()),
    )?;
    if !ok {
        // Hard-reset to the requested commit. Handles edge cases where
        // `checkout --force` refuses (for example dirty index entries that
        // cannot be discarded by a plain checkout).
        crate::core::log_bus::push("git", "checkout --force failed; trying reset --hard");
        return run_logged(
            "git",
            Cmd::new("git")
                .arg("-C")
                .arg(repo.display().to_string())
                .arg("reset")
                .arg("--hard")
                .arg(rev.to_string())
                .envs(env),
        );
    }
    Ok(true)
}

/// Runs `git reset --hard <rev>`.
///
/// Used by Reinstall All to force the working tree to match upstream,
/// discarding local edits to tracked files.
pub fn reset_hard(
    repo: &Path,
    rev: &str,
    env: std::collections::HashMap<String, String>,
) -> Result<bool> {
    run_logged(
        "git",
        Cmd::new("git")
            .arg("-C")
            .arg(repo.display().to_string())
            .arg("reset")
            .arg("--hard")
            .arg(rev.to_string())
            .envs(env),
    )
}

/// Runs `git clone <url> <dest>` with non-interactive safeguards applied.
pub fn clone(
    url: &str,
    dest: &Path,
    env: std::collections::HashMap<String, String>,
) -> Result<bool> {
    run_logged(
        "git",
        Cmd::new("git")
            .arg("clone")
            .arg(url.to_string())
            .arg(dest.display().to_string())
            .envs(no_prompt_env(env)),
    )
}

/// One release tag together with the commit it points at.
#[derive(Debug, Clone)]
pub struct TagCommit {
    /// Tag name, for example `v0.3.42`.
    pub tag: String,
    /// Short SHA in git's default abbreviation (the same length `git log %h`
    /// produces in this repository).
    pub commit_short: String,
    /// Commit date in `YYYY-MM-DD` form.
    pub commit_date: String,
    /// Commit subject.
    pub commit_subject: String,
}

/// Parses a `vMAJOR.MINOR.PATCH` tag into a tuple, or `None` on mismatch.
fn parse_release_tag(s: &str) -> Option<(u32, u32, u32)> {
    let rest = s.strip_prefix('v')?;
    let mut parts = rest.split('.');
    let a = parts.next()?.parse::<u32>().ok()?;
    let b = parts.next()?.parse::<u32>().ok()?;
    let c = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

/// Enumerates `v<major>.<minor>.<patch>` release tags in the repository,
/// descending by version. Annotated tags resolve to their peeled commit.
pub fn tags_pointing_at_releases(repo: &Path) -> Vec<TagCommit> {
    let out = match Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("for-each-ref")
        .arg("--format=%(objectname) %(*objectname) %(refname:short)")
        .arg("refs/tags/v*.*.*")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut tags: Vec<(u32, u32, u32, String, String)> = Vec::new();
    for line in text.lines() {
        // Format: "<objectname> <*objectname> <refname:short>". A
        // lightweight tag has an empty `*objectname`, yielding two
        // space-separated fields instead of three.
        let mut it = line.splitn(3, ' ');
        let obj = it.next().unwrap_or("");
        let peeled = it.next().unwrap_or("");
        let name = it.next().unwrap_or("");
        let (name, sha) = if name.is_empty() {
            // Two fields: the second is the refname, no peel.
            (peeled, obj)
        } else if peeled.is_empty() {
            (name, obj)
        } else {
            (name, peeled)
        };
        if name.is_empty() || sha.is_empty() {
            continue;
        }
        let Some((a, b, c)) = parse_release_tag(name) else {
            continue;
        };
        tags.push((a, b, c, name.to_string(), sha.to_string()));
    }
    tags.sort_by_key(|y| std::cmp::Reverse((y.0, y.1, y.2)));
    let mut out_vec = Vec::with_capacity(tags.len());
    for (_, _, _, tag, sha) in tags {
        let (short, date, subject) = show_short_date_subject(repo, &sha);
        let _ = sha;
        out_vec.push(TagCommit {
            tag,
            commit_short: short,
            commit_date: date,
            commit_subject: subject,
        });
    }
    out_vec
}

fn show_short_date_subject(repo: &Path, sha: &str) -> (String, String, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("show")
        .arg("-s")
        .arg("--format=%h\x1f%cs\x1f%s")
        .arg(sha)
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let mut it = s.splitn(3, '\x1f');
            let short = it.next().unwrap_or("").to_string();
            let date = it.next().unwrap_or("").to_string();
            let subject = it.next().unwrap_or("").to_string();
            return (short, date, subject);
        }
    }
    (String::new(), String::new(), String::new())
}

/// Returns the release tag name when `HEAD` points exactly at a tag of the
/// form `v<major>.<minor>.<patch>`.
pub fn current_release_tag(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("describe")
        .arg("--tags")
        .arg("--exact-match")
        .arg("HEAD")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if parse_release_tag(&name).is_some() {
        Some(name)
    } else {
        None
    }
}
