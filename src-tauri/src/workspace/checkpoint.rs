//! File state tracking and checkpoint management for session rewind.
//!
//! Captures file snapshots before and after each agent turn, enabling
//! deterministic restoration of workspace state to any previous turn.
//!
//! Adapted from the checkpoint system in Cat's workspace module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::core::error::{AppError, AppResult};

/// A snapshot of a single file's content at a specific point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    /// Path to the file, relative to the workspace root.
    pub path: PathBuf,
    /// The content of the file at the time of snapshot (None if file didn't exist).
    pub content: Option<String>,
    /// When this snapshot was taken.
    pub captured_at: DateTime<Utc>,
}

impl FileSnapshot {
    /// Create a new file snapshot.
    pub fn new(path: impl Into<PathBuf>, content: Option<String>) -> Self {
        Self {
            path: path.into(),
            content,
            captured_at: Utc::now(),
        }
    }
}

/// A checkpoint representing the state at a specific agent turn.
///
/// Contains snapshots of all files that were read or modified during
/// that turn's processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindPoint {
    /// Index of the agent turn (0-based, corresponds to user prompt).
    pub turn_index: usize,
    /// When this rewind point was created.
    pub created_at: DateTime<Utc>,
    /// File snapshots captured BEFORE any operations for this turn.
    pub before_snapshots: HashMap<PathBuf, FileSnapshot>,
    /// File snapshots captured AFTER all operations for this turn completed.
    pub after_snapshots: HashMap<PathBuf, FileSnapshot>,
}

impl RewindPoint {
    /// Create a new empty rewind point for the given turn index.
    pub fn new(turn_index: usize) -> Self {
        Self {
            turn_index,
            created_at: Utc::now(),
            before_snapshots: HashMap::new(),
            after_snapshots: HashMap::new(),
        }
    }

    /// Add a before-snapshot for a file (only the first snapshot is kept).
    pub fn add_before_snapshot(&mut self, snapshot: FileSnapshot) {
        self.before_snapshots
            .entry(snapshot.path.clone())
            .or_insert(snapshot);
    }

    /// Set the after-snapshot for a file (latest wins).
    pub fn set_after_snapshot(&mut self, snapshot: FileSnapshot) {
        self.after_snapshots.insert(snapshot.path.clone(), snapshot);
    }
}

/// A single rewind point loaded from the database: (turn_index, created_at,
/// before_snapshots, after_snapshots).
type PersistedPoint = (
    usize,
    String,
    HashMap<PathBuf, FileSnapshot>,
    HashMap<PathBuf, FileSnapshot>,
);

/// Tracks file states across turns in a session for rewind functionality.
#[derive(Debug, Clone)]
pub struct FileStateTracker {
    rewind_points: Arc<Mutex<HashMap<usize, RewindPoint>>>,
    current_turn: Arc<Mutex<Option<usize>>>,
}

impl FileStateTracker {
    /// Create a new file state tracker.
    pub fn new(_workspace: Option<PathBuf>) -> Self {
        Self {
            rewind_points: Arc::new(Mutex::new(HashMap::new())),
            current_turn: Arc::new(Mutex::new(None)),
        }
    }

    /// Start tracking a new turn.
    pub async fn begin_turn(&self, turn_index: usize) {
        let mut current = self.current_turn.lock().await;
        *current = Some(turn_index);

        let mut points = self.rewind_points.lock().await;
        points
            .entry(turn_index)
            .or_insert_with(|| RewindPoint::new(turn_index));
    }

    /// End tracking for the given turn.
    pub async fn end_turn(&self, turn_index: usize) {
        let mut current = self.current_turn.lock().await;
        *current = None;

        // Capture after-snapshots for all files that were touched.
        let paths_to_capture: Vec<PathBuf> = {
            let points = self.rewind_points.lock().await;
            if let Some(point) = points.get(&turn_index) {
                point.before_snapshots.keys().cloned().collect()
            } else {
                vec![]
            }
        };

        for path in paths_to_capture {
            let content = self.read_file_content(&path).await;
            let snapshot = FileSnapshot::new(&path, content);

            let mut points = self.rewind_points.lock().await;
            if let Some(point) = points.get_mut(&turn_index) {
                // Hash dedup: an unchanged file must not be re-snapshotted —
                // bounded storage, one snapshot per distinct content state.
                let unchanged = point
                    .after_snapshots
                    .get(&path)
                    .map(|s| s.content == snapshot.content)
                    .unwrap_or(false);
                if !unchanged {
                    point.set_after_snapshot(snapshot);
                }
            }
        }
    }

    /// Capture a file's current state before an operation.
    ///
    /// Should be called BEFORE reading or writing a file during tool execution.
    pub async fn capture_file_state(&self, path: &Path, workspace: &Path) {
        let current = self.current_turn.lock().await;
        let Some(turn_index) = *current else {
            return;
        };
        drop(current);

        // Convert to relative path for portable storage.
        let rel_path = path.strip_prefix(workspace).unwrap_or(path).to_path_buf();

        let content = self.read_file_content(path).await;
        let snapshot = FileSnapshot::new(&rel_path, content);

        let mut points = self.rewind_points.lock().await;
        if let Some(point) = points.get_mut(&turn_index) {
            // Hash dedup: only first-seen content is stored (a re-capture of
            // the same state adds nothing to restore) — bounded storage.
            let unchanged = point
                .before_snapshots
                .get(&rel_path)
                .map(|s| s.content == snapshot.content)
                .unwrap_or(false);
            if !unchanged {
                point.add_before_snapshot(snapshot);
            }
        }
    }

    /// Get all rewind points.
    pub async fn get_rewind_points(&self) -> Vec<RewindPoint> {
        let points = self.rewind_points.lock().await;
        let mut result: Vec<RewindPoint> = points.values().cloned().collect();
        result.sort_by_key(|p| p.turn_index);
        result
    }

    /// Clear all rewind points after (and including) the specified turn index.
    pub async fn truncate_from(&self, turn_index: usize) {
        let mut points = self.rewind_points.lock().await;
        points.retain(|&idx, _| idx < turn_index);
    }

    /// Read file content, returning None if the file doesn't exist or can't be read.
    async fn read_file_content(&self, path: &Path) -> Option<String> {
        tokio::fs::read_to_string(path).await.ok()
    }

    /// Save all rewind points to the database for a session.
    ///
    /// Persists BOTH before- and after-snapshots so a restored point can
    /// still detect external modifications (`rewind_to` compares current
    /// content against the after-snapshot). Clears the session's rows
    /// first, then inserts the current in-memory points.
    pub async fn save_to_db(
        &self,
        session_id: &str,
        db: &crate::storage::database::Database,
    ) -> AppResult<()> {
        // Collect data first to avoid holding the lock across blocking calls.
        let points_data: Vec<(usize, String, String, String)> = {
            let points = self.rewind_points.lock().await;
            points
                .iter()
                .map(|(turn_index, point)| {
                    let before_json =
                        serde_json::to_string(&point.before_snapshots).unwrap_or_default();
                    let after_json =
                        serde_json::to_string(&point.after_snapshots).unwrap_or_default();
                    (
                        *turn_index,
                        point.created_at.to_rfc3339(),
                        before_json,
                        after_json,
                    )
                })
                .collect()
        };

        let conn = db.conn()?;

        // Clear existing points for this session.
        conn.execute(
            "DELETE FROM rewind_points WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;

        // Insert all rewind points.
        for (turn_index, created_at, before_json, after_json) in points_data {
            conn.execute(
                "INSERT INTO rewind_points
                     (session_id, turn_index, created_at, snapshots_json, after_snapshots_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    session_id,
                    turn_index as i64,
                    created_at,
                    before_json,
                    after_json
                ],
            )?;
        }

        Ok(())
    }

    /// Load rewind points from the database for a session.
    ///
    /// Restores both before- and after-snapshots so persisted points behave
    /// identically to live ones (conflict detection included).
    pub async fn load_from_db(
        &self,
        session_id: &str,
        db: &crate::storage::database::Database,
    ) -> AppResult<()> {
        // Collect data first to avoid holding the lock across blocking calls.
        let rows_data: Vec<PersistedPoint> = {
            let conn = db.conn()?;

            let mut stmt = conn.prepare(
                "SELECT turn_index, created_at, snapshots_json, after_snapshots_json
                 FROM rewind_points
                 WHERE session_id = ?1 ORDER BY turn_index",
            )?;

            let rows = stmt.query_map(rusqlite::params![session_id], |row| {
                let turn_index: i64 = row.get(0)?;
                let created_at: String = row.get(1)?;
                let snapshots_json: String = row.get(2)?;
                let after_json: String = row.get(3)?;
                Ok((turn_index as usize, created_at, snapshots_json, after_json))
            })?;

            let mut result = Vec::new();
            for row in rows {
                let (turn_index, created_at, snapshots_json, after_json) =
                    row.map_err(|e| AppError::Database(e.to_string()))?;

                let before_snapshots: HashMap<PathBuf, FileSnapshot> =
                    serde_json::from_str(&snapshots_json)?;
                let after_snapshots: HashMap<PathBuf, FileSnapshot> =
                    serde_json::from_str(&after_json)?;

                result.push((turn_index, created_at, before_snapshots, after_snapshots));
            }

            result
        };

        let mut points = self.rewind_points.lock().await;
        points.clear();

        for (turn_index, created_at, before_snapshots, after_snapshots) in rows_data {
            let mut point = RewindPoint::new(turn_index);
            point.created_at = created_at.parse().unwrap_or_else(|_| Utc::now());
            point.before_snapshots = before_snapshots;
            point.after_snapshots = after_snapshots;
            points.insert(turn_index, point);
        }

        Ok(())
    }

    /// Restore workspace state to before the specified turn index.
    ///
    /// Returns a list of restored files and any conflicts detected.
    pub async fn rewind_to(&self, turn_index: usize, workspace: &Path) -> RewindResult {
        let mut restored_files = Vec::new();
        let mut conflicts = Vec::new();

        // Collect files to revert: gather earliest before-snapshot per file.
        let mut files_to_revert: HashMap<PathBuf, Option<String>> = HashMap::new();

        let all_points = self.get_rewind_points().await;
        for point in all_points.iter().filter(|p| p.turn_index >= turn_index) {
            for (path, before_snapshot) in &point.before_snapshots {
                files_to_revert
                    .entry(path.clone())
                    .or_insert_with(|| before_snapshot.content.clone());
            }
        }

        for (rel_path, content) in &files_to_revert {
            let abs_path = workspace.join(rel_path);

            // Detect conflicts: check if the file was externally modified.
            let current_content = self.read_file_content(&abs_path).await;
            let after_content = all_points
                .iter()
                .rev()
                .find_map(|p| p.after_snapshots.get(rel_path))
                .and_then(|s| s.content.clone());

            if current_content != after_content && after_content.is_some() {
                conflicts.push(RewindConflict {
                    path: rel_path.clone(),
                    conflict_type: ConflictType::ModifiedExternally,
                });
            }

            // Perform the restore.
            match content {
                Some(data) => {
                    if let Err(e) = tokio::fs::write(&abs_path, data.as_bytes()).await {
                        conflicts.push(RewindConflict {
                            path: rel_path.clone(),
                            conflict_type: ConflictType::WriteError(e.to_string()),
                        });
                        continue;
                    }
                }
                None => {
                    if abs_path.exists() {
                        if let Err(e) = tokio::fs::remove_file(&abs_path).await {
                            conflicts.push(RewindConflict {
                                path: rel_path.clone(),
                                conflict_type: ConflictType::DeleteError(e.to_string()),
                            });
                            continue;
                        }
                    }
                }
            }
            restored_files.push(rel_path.clone());
        }

        // NOTE: no "ghost artifact" cleanup here. The old remove_ghost_files
        // deleted any file absent from every snapshot whose mtime fell inside
        // the rollback window, on the theory that such files were created by
        // tools that never snapshot (bash). That heuristic conflates "created
        // in the window" with "pre-existing but modified in the window"
        // (bash `echo x >> data.csv` bumps the mtime), and it walked the WHOLE
        // workspace (including node_modules/target). Deleting a file we have no
        // snapshot to restore from can destroy a user's pre-existing content —
        // far worse than leaving an agent-created scratch file behind. Rewind
        // restores exactly the files it has before-content for, and touches
        // nothing else.

        // Truncate rewind points from the target index onward.
        if conflicts.iter().all(|c| {
            !matches!(
                c.conflict_type,
                ConflictType::WriteError(_) | ConflictType::DeleteError(_)
            )
        }) {
            self.truncate_from(turn_index).await;
        }

        let success = !conflicts.iter().any(|c| {
            matches!(
                c.conflict_type,
                ConflictType::WriteError(_) | ConflictType::DeleteError(_)
            )
        });

        let error = if success {
            None
        } else {
            Some("Some files could not be reverted due to conflicts".to_string())
        };

        RewindResult {
            restored_files,
            conflicts,
            success,
            error,
        }
    }
}

/// The result of a rewind operation.
#[derive(Debug, Clone)]
pub struct RewindResult {
    pub restored_files: Vec<PathBuf>,
    pub conflicts: Vec<RewindConflict>,
    pub success: bool,
    pub error: Option<String>,
}

/// A conflict detected during rewind.
#[derive(Debug, Clone)]
pub struct RewindConflict {
    pub path: PathBuf,
    pub conflict_type: ConflictType,
}

/// The type of conflict detected during rewind.
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictType {
    ModifiedExternally,
    WriteError(String),
    DeleteError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::database::Database;

    /// Open a fresh temp DB with migrations applied.
    fn test_db() -> Database {
        let dir = std::env::temp_dir().join(format!(
            "ddc-rewind-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("t.db"), false).unwrap();
        db.run_migrations().unwrap();
        db
    }

    /// Create a parent session row — `rewind_points` has a FK to sessions,
    /// so inserts require the session to exist first.
    fn seed_session(db: &Database, session_id: &str) {
        use rusqlite::Connection;
        let now = chrono::Utc::now().to_rfc3339();
        let conn: std::sync::MutexGuard<'_, Connection> = db.conn().unwrap();
        conn.execute(
            "INSERT INTO sessions
                 (id, title, model, provider, status, created_at, updated_at,
                  system_prompt, turn_count, prompt_tokens, completion_tokens)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, '', 0, 0, 0)",
            rusqlite::params![session_id, "test", "deepseek", "deepseek", now, now],
        )
        .unwrap();
    }

    /// Seed a tracker with two turns of before+after snapshots and persist.
    async fn seed_tracker(session_id: &str, db: &Database) -> FileStateTracker {
        let tracker = FileStateTracker::new(Some(std::path::PathBuf::from("/tmp")));
        tracker.begin_turn(0).await;
        tracker
            .capture_file_state(
                std::path::Path::new("/tmp/a.rs"),
                std::path::Path::new("/tmp"),
            )
            .await;
        tracker.end_turn(0).await;
        tracker.begin_turn(1).await;
        tracker
            .capture_file_state(
                std::path::Path::new("/tmp/b.rs"),
                std::path::Path::new("/tmp"),
            )
            .await;
        tracker.end_turn(1).await;
        tracker.save_to_db(session_id, db).await.unwrap();
        tracker
    }

    #[tokio::test]
    async fn save_load_roundtrip_preserves_both_snapshots() {
        let db = test_db();
        let session = "s1";
        seed_session(&db, session);
        seed_tracker(session, &db).await;

        // A fresh tracker (simulating restart) must restore the persisted points.
        let restored = FileStateTracker::new(None);
        restored.load_from_db(session, &db).await.unwrap();

        let points = restored.get_rewind_points().await;
        assert_eq!(points.len(), 2, "both turns must be restored");
        assert_eq!(points[0].turn_index, 0);
        assert_eq!(points[1].turn_index, 1);
        assert_eq!(
            points[0].before_snapshots.len(),
            1,
            "before snapshots restored"
        );
        assert_eq!(
            points[0].after_snapshots.len(),
            1,
            "after snapshots restored — conflict detection stays intact"
        );
        assert!(
            points[1]
                .before_snapshots
                .contains_key(&std::path::PathBuf::from("b.rs")),
            "second turn snapshot present"
        );
    }

    #[tokio::test]
    async fn save_clears_previous_session_rows() {
        let db = test_db();
        let session = "s1";
        seed_session(&db, session);

        // Seed 2 turns, then re-save a tracker with only 1 turn — the DB must
        // reflect the latest state (no stale turn-1 row).
        seed_tracker(session, &db).await;
        let tracker = FileStateTracker::new(None);
        tracker.begin_turn(0).await;
        tracker.end_turn(0).await;
        tracker.save_to_db(session, &db).await.unwrap();

        let restored = FileStateTracker::new(None);
        restored.load_from_db(session, &db).await.unwrap();
        assert_eq!(restored.get_rewind_points().await.len(), 1);
    }

    #[tokio::test]
    async fn persists_after_rewind_truncation() {
        let db = test_db();
        let session = "s1";
        seed_session(&db, session);
        seed_tracker(session, &db).await;

        // Simulate a rewind: load, truncate to turn 1, re-save.
        let tracker = FileStateTracker::new(None);
        tracker.load_from_db(session, &db).await.unwrap();
        tracker.truncate_from(1).await;
        tracker.save_to_db(session, &db).await.unwrap();

        // A third tracker (restart after rewind) sees the truncated set.
        let again = FileStateTracker::new(None);
        again.load_from_db(session, &db).await.unwrap();
        let points = again.get_rewind_points().await;
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].turn_index, 0);
    }

    #[tokio::test]
    async fn rewind_never_touches_files_outside_snapshots() {
        // Regression: the old "ghost artifact" cleanup deleted any file not in
        // a snapshot whose mtime fell inside the rollback window. That
        // destroyed pre-existing user files merely MODIFIED by an
        // unsnapshotted tool (bash `echo x >> data.csv`), and even swept
        // node_modules/target. Rewind must restore only files it has
        // before-content for — never delete anything it cannot restore.
        let dir = std::env::temp_dir().join(format!(
            "ddc-rewind-safe-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // A user file that predates the session and is never snapshotted.
        let user_file = dir.join("data.csv");
        std::fs::write(&user_file, "original,rows\n1,2\n").unwrap();

        let tracker = FileStateTracker::new(Some(dir.clone()));
        tracker.begin_turn(0).await;
        // The turn snapshots only a DIFFERENT file.
        let tracked = dir.join("a.rs");
        std::fs::write(&tracked, "fn main() {}\n").unwrap();
        tracker.capture_file_state(&tracked, &dir).await;
        // Simulate bash appending to the user file (no snapshot taken).
        std::fs::write(&user_file, "original,rows\n1,2\n3,4\n").unwrap();
        tracker.end_turn(0).await;

        let result = tracker.rewind_to(0, &dir).await;
        assert!(result.success);
        assert!(
            result.restored_files.contains(&PathBuf::from("a.rs")),
            "snapshot-known file is restored"
        );
        assert!(
            !result.restored_files.contains(&PathBuf::from("data.csv")),
            "a file with no snapshot must NOT be listed as restored"
        );
        assert!(
            user_file.exists(),
            "a pre-existing user file with no snapshot must survive rewind intact"
        );
        let content = std::fs::read_to_string(&user_file).unwrap();
        assert!(
            content.contains("3,4"),
            "the user's in-window change must be left alone (no snapshot to restore from)"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
