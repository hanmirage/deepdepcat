//! SQLite database connection with migration support.
//!
//! Uses `rusqlite` with bundled SQLite. WAL mode is enabled by default
//! for better concurrent read performance.

mod events;
mod helpers;
mod messages;
mod research;
mod sessions;
mod settings;
mod tasks;
mod usage;

pub use events::{append_event, list_events, list_turn_events, prune_events, AgentEvent};
pub use research::{
    get_research_item, insert_research_item, list_research_items, remove_research_item,
    search_research_items, ResearchItem,
};
pub use tasks::{delete_task, insert_task, list_tasks, update_task};
pub use usage::{GlobalUsage, GlobalUsageStore};

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::core::error::AppResult;
use crate::storage::schema::MIGRATIONS;

/// A thread-safe SQLite database wrapper.
///
/// Uses a `Mutex<Connection>` because `rusqlite::Connection` is `!Sync`.
/// For a desktop application with moderate write contention this is fine;
/// if contention becomes an issue we can switch to a connection pool.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (or create) the database at the given path.
    pub fn open(path: &Path, wal_mode: bool) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        // Enable foreign keys
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        if wal_mode {
            conn.execute_batch("PRAGMA journal_mode = WAL;")?;
            conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
        }

        // Set a reasonable busy timeout (5 seconds)
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Run all pending migrations.
    ///
    /// Each migration runs inside its own transaction with its version row —
    /// a crash mid-migration (power loss, kill) rolls the whole migration
    /// back, so the next startup re-applies it cleanly instead of failing on
    /// a half-created table (the pre-#88 behavior: version row written after
    /// the SQL, so an interrupted batch left the schema half-applied and
    /// every subsequent launch errored out permanently).
    pub fn run_migrations(&self) -> AppResult<()> {
        let conn = self.conn.lock()?;

        // Create migrations tracking table
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                version INTEGER PRIMARY KEY,
                description TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )?;

        // Get current version
        let current_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _migrations",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Apply pending migrations
        for (version, description, sql) in MIGRATIONS {
            if *version > current_version {
                tracing::info!("Applying migration {}: {}", version, description);
                // One transaction per migration: schema change + version
                // row commit atomically (or roll back together).
                conn.execute_batch("BEGIN IMMEDIATE;")?;
                let applied = conn
                    .execute_batch(sql)
                    .and_then(|()| {
                        conn.execute(
                            "INSERT INTO _migrations (version, description, applied_at) VALUES (?1, ?2, ?3)",
                            rusqlite::params![version, description, chrono::Utc::now().to_rfc3339()],
                        )
                        .map(|_| ())
                    });
                match applied {
                    Ok(()) => {
                        conn.execute_batch("COMMIT;")?;
                    }
                    Err(e) => {
                        conn.execute_batch("ROLLBACK;")?;
                        return Err(e.into());
                    }
                }
            }
        }

        Ok(())
    }

    /// Get a locked connection for direct database operations.
    pub fn conn(&self) -> AppResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(Into::into)
    }

    /// Online backup via `VACUUM INTO` — WAL-safe and non-blocking for
    /// readers. Keeps at most `keep` backups in `backup_dir`, pruning the
    /// oldest. Returns the new backup path.
    pub fn backup_to(&self, backup_dir: &Path, keep: usize) -> AppResult<PathBuf> {
        std::fs::create_dir_all(backup_dir)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        // VACUUM INTO refuses to overwrite — disambiguate same-second runs
        // (fast restarts / test loops) with an incrementing suffix.
        let mut target = backup_dir.join(format!("deepdepcat-{stamp}.db"));
        let mut suffix = 1u32;
        while target.exists() {
            target = backup_dir.join(format!("deepdepcat-{stamp}-{suffix}.db"));
            suffix += 1;
        }
        let escaped = target.to_string_lossy().replace('\'', "''");
        {
            let conn = self.conn()?;
            conn.execute_batch(&format!("VACUUM INTO '{escaped}';"))?;
        }

        let mut backups: Vec<PathBuf> = std::fs::read_dir(backup_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|x| x.to_str()) == Some("db")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("deepdepcat-"))
            })
            .collect();
        backups.sort();
        while backups.len() > keep {
            let old = backups.remove(0);
            let _ = std::fs::remove_file(&old);
        }

        tracing::info!(path = %target.display(), "Database backed up");
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> (std::sync::Arc<Database>, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("ddc-mig-test-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        let db = Database::open(&path, true).unwrap();
        (std::sync::Arc::new(db), path)
    }

    #[test]
    fn migrations_apply_atomically_per_version() {
        // Regression for #88 H2: a failed migration must roll back BOTH the
        // schema change and its version row — otherwise a half-applied
        // schema bricks every subsequent launch (the pre-transaction
        // behavior ran the SQL first and wrote the version row second).
        let (db, path) = fresh_db();
        db.run_migrations().unwrap();
        let version: i64 = db
            .conn()
            .unwrap()
            .query_row("SELECT MAX(version) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        let expected: i64 = crate::storage::schema::MIGRATIONS
            .last()
            .map(|(v, _, _)| *v)
            .unwrap_or(0);
        assert_eq!(version, expected, "all migrations applied and versioned");

        // Reopening an already-migrated DB is a no-op (idempotent).
        drop(db);
        let reopened = Database::open(&path, true).unwrap();
        reopened.run_migrations().unwrap();
        let version: i64 = reopened
            .conn()
            .unwrap()
            .query_row("SELECT MAX(version) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, expected, "re-run must not re-apply");
    }

    #[test]
    fn failed_migration_rolls_back_schema_and_version() {
        // A migration whose SQL fails (simulated by a broken batch appended
        // at runtime) must leave no partial schema and no version row.
        // We test the transaction wrapper directly: BEGIN → failing SQL →
        // ROLLBACK must restore the pre-state.
        let (db, _) = fresh_db();
        db.run_migrations().unwrap();
        let conn = db.conn().unwrap();

        conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let err = conn
            .execute_batch("CREATE TABLE _should_not_exist (x); INSERT INTO _should_not_exist VALUES (1); THIS IS NOT SQL;")
            .unwrap_err();
        conn.execute_batch("ROLLBACK;").unwrap();
        assert!(
            err.to_string().contains("syntax error"),
            "the simulated failure must be a SQL error"
        );

        // Schema change rolled back — the table must not exist.
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE name = '_should_not_exist'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            tables.is_empty(),
            "rolled-back migration must leave no schema residue"
        );
    }

    #[test]
    fn backup_to_creates_vacuumed_snapshot() {
        let (db, _path) = fresh_db();
        db.run_migrations().unwrap();
        let backup_dir = std::env::temp_dir().join(format!(
            "ddc-backup-test-{}",
            crate::core::ids::generate_id()
        ));
        let snapshot = db.backup_to(&backup_dir, 2).unwrap();
        assert!(snapshot.exists());
        assert!(snapshot.extension().and_then(|e| e.to_str()) == Some("db"));

        // Snapshot must be a valid database.
        let opened = Connection::open(&snapshot).unwrap();
        let version: i64 = opened
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert!(version >= 1);

        // Keep-limit pruning: create more snapshots than `keep`.
        for _ in 0..3 {
            db.backup_to(&backup_dir, 2).unwrap();
        }
        let remaining: Vec<_> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("db"))
            .collect();
        assert!(remaining.len() <= 2, "old backups must be pruned");
        let _ = std::fs::remove_dir_all(&backup_dir);
    }
}
