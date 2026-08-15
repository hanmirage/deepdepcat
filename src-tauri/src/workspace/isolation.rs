//! Worktree isolation manager — creates and manages isolated worktrees
//! for sub-agent execution.
//!
//! Each sub-agent can optionally run in its own worktree, preventing
//! file modifications from affecting the main workspace or other sub-agents.
//!
//! Adapted from Cat's xai-fast-worktree architecture, simplified for
//! DeepDepCat's sub-agent isolation use case.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::core::error::{AppError, AppResult};
use tracing::warn;

/// Build a git command with no console window on Windows and with
/// `core.quotepath=false` so non-ASCII paths (Chinese filenames etc.)
/// are emitted as literal characters, not octal escapes.
fn git_cmd() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    crate::core::proc::no_window(&mut cmd);
    cmd.args(["-c", "core.quotepath=false"]);
    cmd
}

/// The isolation mode for sub-agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationMode {
    /// No isolation — sub-agent shares the main workspace.
    None,
    /// Linked worktree — shares object store, separate working directory.
    Linked,
}

/// Outcome of merging a subagent's worktree changes back into the main tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeBackOutcome {
    /// Changes were merged into the main working tree (staged, no commit).
    Merged,
    /// The worktree had no changes — nothing to merge.
    NoChanges,
    /// Merge was skipped for a reason (main tree dirty, merge failed, ...);
    /// the subagent branch is left behind for manual recovery.
    Skipped(String),
}

/// Outcome of an explicit worktree cleanup (scheduled-task runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeCleanupOutcome {
    /// The worktree was removed.
    Removed,
    /// No worktree was registered for this session.
    NotRegistered,
    /// The worktree has uncommitted changes — kept for manual review.
    Dirty,
}

/// A managed worktree for sub-agent isolation.
#[derive(Debug, Clone)]
pub struct ManagedWorktree {
    /// The worktree's filesystem path.
    pub path: PathBuf,
    /// The branch name used for this worktree.
    pub branch: String,
}

/// Manages worktree lifecycle for sub-agent isolation.
#[derive(Clone)]
pub struct WorktreeIsolationManager {
    worktrees: Arc<Mutex<HashMap<String, ManagedWorktree>>>,
    /// Base directory for creating worktrees.
    worktree_base: PathBuf,
    /// Default isolation mode.
    default_mode: IsolationMode,
}

impl WorktreeIsolationManager {
    /// Create a new worktree isolation manager.
    pub fn new(worktree_base: PathBuf, default_mode: IsolationMode) -> Self {
        Self {
            worktrees: Arc::new(Mutex::new(HashMap::new())),
            worktree_base,
            default_mode,
        }
    }

    /// Create an isolated worktree for a sub-agent.
    ///
    /// Returns the worktree path if isolation is enabled, or the original
    /// workspace path if isolation is disabled.
    pub async fn create_isolated_worktree(
        &self,
        workspace: &Path,
        session_id: &str,
        mode: Option<IsolationMode>,
    ) -> AppResult<PathBuf> {
        let mode = mode.unwrap_or(self.default_mode);

        match mode {
            IsolationMode::None => Ok(workspace.to_path_buf()),
            IsolationMode::Linked => self.create_linked_worktree(workspace, session_id).await,
        }
    }

    /// Create a linked worktree for a sub-agent.
    async fn create_linked_worktree(
        &self,
        workspace: &Path,
        session_id: &str,
    ) -> AppResult<PathBuf> {
        // Char-safe prefix: byte slicing (`&session_id[..8]`) would panic on
        // non-ASCII ids. Session ids are ASCII today, but git branch names
        // must be valid regardless.
        let short_id: String = session_id.chars().take(8).collect();
        let branch_name = format!("subagent-{short_id}");
        let worktree_path = self.worktree_base.join(&branch_name);

        // Check if the worktree already exists.
        if worktree_path.exists() {
            // Reuse existing worktree.
            let mut worktrees = self.worktrees.lock().await;
            worktrees.insert(
                session_id.to_string(),
                ManagedWorktree {
                    path: worktree_path.clone(),
                    branch: branch_name,
                },
            );
            return Ok(worktree_path);
        }

        // Create the worktree using git commands.
        self.run_git_worktree_add(workspace, &worktree_path, &branch_name)?;

        // Register the worktree.
        let mut worktrees = self.worktrees.lock().await;
        worktrees.insert(
            session_id.to_string(),
            ManagedWorktree {
                path: worktree_path.clone(),
                branch: branch_name,
            },
        );

        Ok(worktree_path)
    }

    /// Run `git worktree add` to create a linked worktree.
    fn run_git_worktree_add(&self, workspace: &Path, dest: &Path, branch: &str) -> AppResult<()> {
        // Ensure parent directory exists.
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Io(std::io::Error::other(format!(
                    "Failed to create worktree parent directory: {}",
                    e
                )))
            })?;
        }

        // Linked worktree.
        let mut cmd = git_cmd();
        let output = cmd
            .current_dir(workspace)
            .args(["worktree", "add", "-b", branch])
            .arg(dest)
            .output()
            .map_err(|e| AppError::Internal(format!("Failed to run git worktree add: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            // If branch already exists, try without -b.
            if stderr.contains("already exists") {
                let mut cmd2 = git_cmd();
                let output2 = cmd2
                    .current_dir(workspace)
                    .args(["worktree", "add"])
                    .arg(dest)
                    .arg(branch)
                    .output()
                    .map_err(|e| {
                        AppError::Internal(format!("Failed to run git worktree add: {}", e))
                    })?;

                if !output2.status.success() {
                    return Err(AppError::Internal(format!(
                        "Failed to create worktree: {}",
                        String::from_utf8_lossy(&output2.stderr)
                    )));
                }
            } else {
                return Err(AppError::Internal(format!(
                    "Failed to create worktree: {}",
                    stderr
                )));
            }
        }

        Ok(())
    }

    /// Merge a subagent's worktree changes back into the main tree and
    /// clean up the worktree.
    ///
    /// The subagent's branch is committed inside the worktree, then merged
    /// into the main repo's working tree (`--no-commit --no-ff -X theirs` —
    /// changes land STAGED in the parent workspace for review, no commit is
    /// created). The merge is NOT gated on a fully clean main tree: a
    /// previous subagent's merge leaves its changes UNSTAGED (`reset --mixed`
    /// below), so a strict `git status --porcelain` check would skip every
    /// merge after the first. Git itself refuses a merge that would overwrite
    /// uncommitted or untracked files — on such a conflict we abort cleanly
    /// and keep the branch for manual recovery.
    pub async fn merge_back_and_cleanup(
        &self,
        workspace: &Path,
        session_id: &str,
    ) -> AppResult<MergeBackOutcome> {
        let worktree = {
            let mut worktrees = self.worktrees.lock().await;
            worktrees.remove(session_id)
        };
        let Some(wt) = worktree else {
            return Ok(MergeBackOutcome::Skipped("no worktree registered".into()));
        };

        let run = |dir: &Path, args: &[&str]| -> std::io::Result<std::process::Output> {
            let mut cmd = git_cmd();
            cmd.current_dir(dir).args(args).output()
        };

        // 1. Any changes at all?
        let status = run(&wt.path, &["status", "--porcelain"])?;
        if status.stdout.is_empty() {
            self.remove_worktree(&wt.path)?;
            return Ok(MergeBackOutcome::NoChanges);
        }

        // 2. Commit the subagent's work in the worktree.
        let _ = run(&wt.path, &["add", "-A"]);
        let commit = run(
            &wt.path,
            &["commit", "-m", &format!("subagent {session_id} work")],
        )?;
        if !commit.status.success() {
            // Commit failed (most commonly git user.name/email not
            // configured). The subagent's output must NOT be destroyed —
            // keep the worktree for manual recovery instead of silently
            // force-removing it and returning NoChanges (which would make
            // the caller believe the subagent produced nothing).
            warn!(
                worktree = %wt.path.display(),
                branch = %wt.branch,
                error = %String::from_utf8_lossy(&commit.stderr),
                "Commit failed in worktree — keeping the worktree for manual recovery"
            );
            return Ok(MergeBackOutcome::Skipped(format!(
                "commit failed in worktree: {}",
                String::from_utf8_lossy(&commit.stderr).trim()
            )));
        }

        // 3. Merge the branch into the main working tree (staged, no commit).
        //    No `git status` pre-check: a previous subagent's merge left
        //    changes UNSTAGED in the main tree, which is exactly what a
        //    second worktree subagent must merge on top of. Git refuses the
        //    merge itself when it would overwrite uncommitted/untracked files
        //    (a genuine conflict with the user's own work) — we abort cleanly
        //    and keep the branch in that case.
        let merge = run(
            workspace,
            &["merge", "--no-commit", "--no-ff", "-X", "theirs", &wt.branch],
        )?;
        if !merge.status.success() {
            // Real conflict (the subagent's files also have uncommitted main
            // changes, or untracked files it would overwrite). Abort the
            // in-progress merge so the repo is left clean, and keep the
            // subagent branch for manual recovery.
            let _ = run(workspace, &["merge", "--abort"]);
            warn!(
                branch = %wt.branch,
                error = %String::from_utf8_lossy(&merge.stderr),
                "Merge-back conflicted with uncommitted main-tree work — \
                 aborted; subagent branch left behind for manual merge"
            );
            return Ok(MergeBackOutcome::Skipped(format!(
                "merge conflicted with main-tree work: {}",
                String::from_utf8_lossy(&merge.stderr).trim()
            )));
        }

        // 5. Clear the merge state, keep changes unstaged in the working
        //    tree, then unregister the worktree FIRST (git refuses to
        //    delete a branch that is checked out in a worktree), and only
        //    then delete the branch.
        let _ = run(workspace, &["reset", "--mixed", "HEAD"]);
        self.remove_worktree(&wt.path)?;
        let _ = run(workspace, &["branch", "-D", &wt.branch]);
        Ok(MergeBackOutcome::Merged)
    }

    /// Explicitly remove a registered worktree. Refuses when the worktree
    /// has uncommitted changes (the scheduled-run flow deliberately leaves
    /// work behind for review; deleting it would destroy the agent's
    /// output). Returns the outcome without erroring on "nothing to do".
    pub async fn cleanup_worktree(
        &self,
        session_id: &str,
    ) -> AppResult<WorktreeCleanupOutcome> {
        let worktree = self.worktrees.lock().await.remove(session_id);
        let Some(wt) = worktree else {
            return Ok(WorktreeCleanupOutcome::NotRegistered);
        };

        let status = {
            let mut cmd = git_cmd();
            cmd.current_dir(&wt.path)
                .args(["status", "--porcelain"])
                .output()
                .map_err(|e| {
                    AppError::Internal(format!("Failed to inspect worktree status: {e}"))
                })?
        };
        if !status.stdout.is_empty() {
            // Keep the registration — the user may clean up after merging.
            self.worktrees
                .lock()
                .await
                .insert(session_id.to_string(), wt);
            return Ok(WorktreeCleanupOutcome::Dirty);
        }

        self.remove_worktree(&wt.path)?;
        Ok(WorktreeCleanupOutcome::Removed)
    }

    /// Remove a worktree.
    fn remove_worktree(&self, path: &Path) -> AppResult<()> {
        let parent = path
            .parent()
            .ok_or_else(|| AppError::Internal("Worktree path has no parent".to_string()))?;

        // Find the main repo and run `git worktree remove`.
        let mut cmd = git_cmd();
        let output = cmd
            .current_dir(parent)
            .args(["worktree", "remove", "--force"])
            .arg(path)
            .output()
            .map_err(|e| AppError::Internal(format!("Failed to run git worktree remove: {}", e)))?;

        if !output.status.success() {
            // If git worktree remove fails, try to clean up the directory.
            let _ = std::fs::remove_dir_all(path);
        }

        Ok(())
    }
}
