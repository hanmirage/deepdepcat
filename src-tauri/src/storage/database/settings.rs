use rusqlite::{params, OptionalExtension};

use chrono::Utc;

use crate::core::error::AppResult;

use super::Database;

impl Database {
    /// Get a setting value by key.
    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self.conn.lock()?;
        let value = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    /// Set a setting value.
    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.conn.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![key, value, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        let dir = std::env::temp_dir().join(format!(
            "ddc-settings-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("test.db"), true).unwrap();
        db.run_migrations().unwrap();
        db
    }

    #[test]
    fn settings_roundtrip_and_overwrite() {
        let db = test_db();
        assert_eq!(db.get_setting("diagnostics_enabled").unwrap(), None);

        db.set_setting("diagnostics_enabled", "true").unwrap();
        assert_eq!(
            db.get_setting("diagnostics_enabled").unwrap().as_deref(),
            Some("true")
        );

        // INSERT OR REPLACE — setting the same key again overwrites.
        db.set_setting("diagnostics_enabled", "false").unwrap();
        assert_eq!(
            db.get_setting("diagnostics_enabled").unwrap().as_deref(),
            Some("false")
        );

        // Empty string is a legitimate value (cleared last workspace) and
        // must round-trip, not read back as None.
        db.set_setting("last_workspace", "").unwrap();
        assert_eq!(
            db.get_setting("last_workspace").unwrap().as_deref(),
            Some("")
        );

        // Unrelated keys stay independent.
        assert_eq!(db.get_setting("other").unwrap(), None);
    }
}
