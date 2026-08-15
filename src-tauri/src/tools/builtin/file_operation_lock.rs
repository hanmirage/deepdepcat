//! File operation lock — prevents concurrent file modifications.
//!
//! Provides a per-file locking mechanism so that multiple tool calls
//! (or subagents) don't clobber each other's edits. Uses a path-keyed
//! RwLock map.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::debug;

/// Per-file lock state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockState {
    /// The file path being locked.
    pub path: String,
    /// The session that holds the lock.
    pub holder_session_id: String,
    /// When the lock was acquired (epoch millis).
    pub acquired_at_ms: u64,
}

/// Locks older than this are treated as stale (e.g. the holding session
/// crashed or was abandoned) and can be taken over by another session.
const LOCK_STALE_MS: u64 = 10 * 60 * 1000;

/// Shared lock manager — tracks all active file locks.
#[derive(Clone)]
pub struct FileLockManager {
    locks: Arc<RwLock<HashMap<String, LockState>>>,
}

impl FileLockManager {
    /// Create a new empty lock manager.
    pub fn new() -> Self {
        Self {
            locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Try to acquire a lock on a file path.
    ///
    /// Returns `Err` if the file is already locked by a different session
    /// and that lock is still fresh. A stale lock (past the timeout — the
    /// holding session likely died) is taken over; a fresh lock by the same
    /// session is a no-op. Paths are canonicalized before locking so the
    /// same file reached via different spellings collides on one key.
    pub fn acquire(&self, path: &str, session_id: &str) -> Result<(), String> {
        let key = canonical_key(path);
        let mut locks = self.locks.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = locks.get(&key) {
            if existing.holder_session_id != session_id {
                let stale =
                    current_epoch_ms().saturating_sub(existing.acquired_at_ms) >= LOCK_STALE_MS;
                if !stale {
                    return Err(format!(
                        "File '{}' is locked by session '{}'",
                        path, existing.holder_session_id
                    ));
                }
            } else {
                // Same session re-acquiring — no-op.
                return Ok(());
            }
        }

        locks.insert(
            key,
            LockState {
                path: path.to_string(),
                holder_session_id: session_id.to_string(),
                acquired_at_ms: current_epoch_ms(),
            },
        );
        debug!(path, session_id, "File lock acquired");
        Ok(())
    }

    /// Release a lock on a file path.
    ///
    /// Returns `false` if the lock doesn't exist or is held by a different session.
    pub fn release(&self, path: &str, session_id: &str) -> bool {
        let key = canonical_key(path);
        let mut locks = self.locks.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = locks.get(&key) {
            if existing.holder_session_id == session_id {
                locks.remove(&key);
                debug!(path, session_id, "File lock released");
                return true;
            }
        }
        false
    }

    /// Check the lock status of a file.
    pub fn status(&self, path: &str) -> Option<LockState> {
        let key = canonical_key(path);
        self.locks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
    }

    /// List all active locks.
    pub fn list(&self) -> Vec<LockState> {
        self.locks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }
}

impl Default for FileLockManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonicalize a path for use as a lock key, so the same file locks once
/// no matter how it is spelled. Non-existent paths (a file locked before
/// its first write) can't be canonicalized — they fall back to the raw
/// path.
fn canonical_key(path: &str) -> String {
    let p = std::path::Path::new(path);
    if let Ok(canon) = p.canonicalize() {
        canon.to_string_lossy().to_string()
    } else {
        path.to_string()
    }
}

fn current_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// File operation lock tool.
pub struct FileOperationLockTool {
    manager: FileLockManager,
}

impl FileOperationLockTool {
    /// Create a new file lock tool with the given manager.
    pub fn new(manager: FileLockManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for FileOperationLockTool {
    fn name(&self) -> &str {
        "file_operation_lock"
    }

    fn description(&self) -> &str {
        "Coordinate file edits across sessions/subagents via advisory locks. \
        Acquire/check/release per-file locks so parallel workers can AVOID \
        clobbering each other. Advisory only: write tools (edit_file, \
        search_replace, apply_patch, write_file) do NOT auto-check the lock — \
        a lock records intent, it does not enforce exclusion. Check `status` \
        before editing a file another worker may be touching."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["acquire", "release", "status", "list"],
                    "description": "The lock operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "File path (required for acquire, release, status)"
                }
            },
            "required": ["operation"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn check_permissions(&self, _args: &Value, _ctx: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let operation = args
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'operation'".into())
            })?;

        let session_id = &ctx.session_id;

        match operation {
            "acquire" => {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::core::error::AppError::ToolNotFound("missing 'path'".into())
                })?;

                match self.manager.acquire(path, session_id) {
                    Ok(()) => Ok(ToolResult::success(format!("Lock acquired on '{path}'."))),
                    Err(e) => Ok(ToolResult::error(e)),
                }
            }
            "release" => {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::core::error::AppError::ToolNotFound("missing 'path'".into())
                })?;

                if self.manager.release(path, session_id) {
                    Ok(ToolResult::success(format!("Lock released on '{path}'.")))
                } else {
                    Ok(ToolResult::error(format!(
                        "No lock held on '{path}' by this session."
                    )))
                }
            }
            "status" => {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::core::error::AppError::ToolNotFound("missing 'path'".into())
                })?;

                match self.manager.status(path) {
                    Some(state) => Ok(ToolResult::success(format!(
                        "File '{}' is locked by session '{}' (acquired at {}ms).",
                        state.path, state.holder_session_id, state.acquired_at_ms
                    ))),
                    None => Ok(ToolResult::success(format!("File '{path}' is not locked."))),
                }
            }
            "list" => {
                let locks = self.manager.list();
                if locks.is_empty() {
                    Ok(ToolResult::success("No active file locks."))
                } else {
                    let lines: Vec<String> = locks
                        .iter()
                        .map(|l| {
                            format!(
                                "- {} (session: {}, acquired: {}ms)",
                                l.path, l.holder_session_id, l.acquired_at_ms
                            )
                        })
                        .collect();
                    Ok(ToolResult::success(format!(
                        "Active locks:\n{}",
                        lines.join("\n")
                    )))
                }
            }
            _ => Ok(ToolResult::error(format!("Unknown operation: {operation}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let mgr = FileLockManager::new();
        assert!(mgr.acquire("file.rs", "s1").is_ok());
        assert!(mgr.status("file.rs").is_some());
        assert!(mgr.release("file.rs", "s1"));
        assert!(mgr.status("file.rs").is_none());
    }

    #[test]
    fn cannot_acquire_locked_file() {
        let mgr = FileLockManager::new();
        mgr.acquire("file.rs", "s1").unwrap();
        let result = mgr.acquire("file.rs", "s2");
        assert!(result.is_err());
    }

    #[test]
    fn same_session_reacquire_is_ok() {
        let mgr = FileLockManager::new();
        mgr.acquire("file.rs", "s1").unwrap();
        assert!(mgr.acquire("file.rs", "s1").is_ok());
    }

    #[test]
    fn stale_lock_can_be_taken_over() {
        let mgr = FileLockManager::new();
        mgr.acquire("file.rs", "s1").unwrap();
        // Age the lock past the timeout — as if session s1 crashed.
        {
            let mut locks = mgr.locks.write().unwrap_or_else(|e| e.into_inner());
            let st = locks.get_mut("file.rs").unwrap();
            st.acquired_at_ms = current_epoch_ms() - LOCK_STALE_MS - 1;
        }
        assert!(
            mgr.acquire("file.rs", "s2").is_ok(),
            "stale lock must be stealable"
        );
        let state = mgr.status("file.rs").unwrap();
        assert_eq!(state.holder_session_id, "s2");
    }

    #[test]
    fn fresh_lock_by_other_session_is_kept() {
        let mgr = FileLockManager::new();
        mgr.acquire("file.rs", "s1").unwrap();
        assert!(mgr.acquire("file.rs", "s2").is_err());
    }

    #[test]
    fn canonical_paths_lock_once() {
        // The same real file reached via different spellings must collide
        // on one lock key.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.rs");
        std::fs::write(&f, "x").unwrap();
        let abs = f.to_string_lossy().to_string();
        let dot = format!("{}{}", dir.path().join(".").to_string_lossy(), "\\a.rs");
        let mgr = FileLockManager::new();
        assert!(mgr.acquire(&abs, "s1").is_ok());
        assert!(mgr.acquire(&dot, "s2").is_err(), "same file must lock once");
    }
}
