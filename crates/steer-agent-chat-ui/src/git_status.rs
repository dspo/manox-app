//! Git branch status for the conversation info bubble.
//!
//! Shells out to the system `git` binary (never `git2` — banned by project
//! rule) to resolve the current branch, then parses the output into a pure
//! value type the UI renders from. All subprocess work runs on the global
//! tokio runtime via [`manox_agent::runtime::handle`]; the results are
//! delivered back to the gpui-side caller through an executor-agnostic
//! `async_channel`, the same bridge the worktree tool uses.
//!
//! Parsing is split from IO so the pure functions are unit-testable without a
//! real git repo.

use std::path::PathBuf;

use manox_agent::runtime;

/// Resolved branch identity for the bubble's branch pair.
///
/// `branch` is `Some` on a normal checked-out branch; `detached_sha` is `Some`
/// in detached-HEAD (caller queries the short sha via `git rev-parse`). Both
/// are `None` when the cwd is not inside a git repo. `is_worktree` is flagged
/// by the caller from `Thread::worktree()` so the label can note it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitBranchDisplay {
    pub branch: Option<String>,
    pub detached_sha: Option<String>,
    pub is_worktree: bool,
}

impl GitBranchDisplay {
    /// Whether the cwd is not under git at all.
    pub fn is_no_repo(&self) -> bool {
        self.branch.is_none() && self.detached_sha.is_none()
    }
}

/// Parse `git branch --show-current` output. Empty output means detached HEAD
/// or a non-git cwd; the caller resolves which via `git rev-parse --short HEAD`.
pub fn parse_branch(output: &str) -> GitBranchDisplay {
    let branch = output.trim();
    if branch.is_empty() {
        GitBranchDisplay::default()
    } else {
        GitBranchDisplay {
            branch: Some(branch.to_string()),
            detached_sha: None,
            is_worktree: false,
        }
    }
}

/// Parse `git rev-parse --short HEAD` output for the detached-HEAD short sha.
pub fn parse_short_sha(output: &str) -> Option<String> {
    let s = output.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

// ── IO: tokio-bridged git shell-outs ───────────────────────────────────────

/// Run `git` with `args` in `cwd` on the global tokio runtime, returning the
/// trimmed stdout. A non-zero exit or a missing `git` binary yields `None`
/// (the UI falls back to its "not a repo" / "unavailable" labels).
async fn run_git(cwd: &str, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Resolve the branch display for `cwd` in a single background task.
///
/// Branch resolution prefers a worktree's recorded branch (passed in via
/// `worktree_branch`) when the thread is inside a worktree; otherwise it
/// shells out to `git branch --show-current`, falling back to the short sha
/// for detached HEAD. Returns `None` entirely when `cwd` is not a git repo.
pub async fn gather_branch(
    cwd: PathBuf,
    worktree_branch: Option<String>,
) -> Option<GitBranchDisplay> {
    let cwd_str = cwd.to_string_lossy().to_string();
    // A single `git rev-parse --show-toplevel` gates everything else: if it
    // fails the cwd is not under git, so the rail shows its not-a-repo label.
    run_git(&cwd_str, &["rev-parse", "--show-toplevel"]).await?;

    let is_worktree = worktree_branch.is_some();
    let branch = if let Some(b) = worktree_branch {
        Some(b)
    } else {
        match run_git(&cwd_str, &["branch", "--show-current"]).await {
            Some(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    };
    let detached_sha = if branch.is_none() {
        run_git(&cwd_str, &["rev-parse", "--short", "HEAD"])
            .await
            .and_then(|s| parse_short_sha(&s))
    } else {
        None
    };
    if branch.is_none() && detached_sha.is_none() {
        // `rev-parse --show-toplevel` succeeded but neither branch nor sha
        // resolved — treat as no-repo so the label is honest.
        return None;
    }
    Some(GitBranchDisplay {
        branch,
        detached_sha,
        is_worktree,
    })
}

/// Spawn [`gather_branch`] on the global tokio runtime and deliver its result
/// back to the gpui executor via an `async_channel` of capacity 1. Returns the
/// result (or `None` on cancellation) so a `cx.spawn` caller can `.await` it
/// without touching `&mut App`.
pub async fn gather_bridged(
    cwd: PathBuf,
    worktree_branch: Option<String>,
) -> Option<GitBranchDisplay> {
    let (tx, rx) = async_channel::bounded(1);
    runtime::handle().spawn(async move {
        let r = gather_branch(cwd, worktree_branch).await;
        let _ = tx.send(r).await;
    });
    rx.recv().await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_normal() {
        let d = parse_branch("main\n");
        assert_eq!(d.branch.as_deref(), Some("main"));
        assert!(d.detached_sha.is_none());
        assert!(!d.is_worktree);
    }

    #[test]
    fn branch_empty_is_detached_or_no_repo() {
        let d = parse_branch("");
        assert!(d.branch.is_none());
        assert!(d.detached_sha.is_none());
        assert!(d.is_no_repo());
    }

    #[test]
    fn branch_whitespace_only_is_detached() {
        let d = parse_branch("   \n");
        assert!(d.branch.is_none());
    }

    #[test]
    fn short_sha_normal() {
        assert_eq!(parse_short_sha("abc1234\n"), Some("abc1234".to_string()));
    }

    #[test]
    fn short_sha_empty() {
        assert_eq!(parse_short_sha(""), None);
        assert_eq!(parse_short_sha("   \n"), None);
    }

    #[test]
    fn is_no_repo_true_when_both_none() {
        assert!(GitBranchDisplay::default().is_no_repo());
        assert!(
            !GitBranchDisplay {
                branch: Some("main".into()),
                detached_sha: None,
                is_worktree: false,
            }
            .is_no_repo()
        );
        assert!(
            !GitBranchDisplay {
                branch: None,
                detached_sha: Some("abc1234".into()),
                is_worktree: false,
            }
            .is_no_repo()
        );
    }
}
