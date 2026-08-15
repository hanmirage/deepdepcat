//! Research items — the Depwork 调研资料夹 persistence layer.

use crate::core::error::AppResult;
use crate::storage::database::Database;
use rusqlite::params;

/// One saved research source.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResearchItem {
    pub id: i64,
    pub session_id: String,
    pub title: String,
    pub url: String,
    pub source: String,
    pub snippet: String,
    pub snapshot: String,
    pub tags: String,
    pub created_at: String,
}

/// Insert a research item, returning its new id.
#[allow(clippy::too_many_arguments)]
pub fn insert_research_item(
    db: &Database,
    session_id: &str,
    title: &str,
    url: &str,
    source: &str,
    snippet: &str,
    snapshot: &str,
    tags: &str,
) -> AppResult<i64> {
    let conn = db.conn()?;
    let now = chrono::Utc::now();
    conn.execute(
        "INSERT INTO research_items
             (session_id, title, url, source, snippet, snapshot, tags, created_at, created_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            session_id,
            title,
            url,
            source,
            snippet,
            snapshot,
            tags,
            now.to_rfc3339(),
            now.timestamp_millis()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResearchItem> {
    Ok(ResearchItem {
        id: row.get(0)?,
        session_id: row.get(1)?,
        title: row.get(2)?,
        url: row.get(3)?,
        source: row.get(4)?,
        snippet: row.get(5)?,
        snapshot: row.get(6)?,
        tags: row.get(7)?,
        created_at: row.get(8)?,
    })
}

/// List a session's research items, newest first (optional tag filter).
pub fn list_research_items(
    db: &Database,
    session_id: &str,
    tag: Option<&str>,
    limit: usize,
) -> AppResult<Vec<ResearchItem>> {
    let conn = db.conn()?;
    let mut items = Vec::new();
    match tag {
        Some(tag) if !tag.is_empty() => {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, title, url, source, snippet, snapshot, tags, created_at
                 FROM research_items
                 WHERE session_id = ?1 AND tags LIKE ?2
                 ORDER BY created_ms DESC
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(
                params![session_id, format!("%{tag}%"), limit as i64],
                row_to_item,
            )?;
            for row in rows {
                items.push(row?);
            }
        }
        _ => {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, title, url, source, snippet, snapshot, tags, created_at
                 FROM research_items
                 WHERE session_id = ?1
                 ORDER BY created_ms DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![session_id, limit as i64], row_to_item)?;
            for row in rows {
                items.push(row?);
            }
        }
    }
    Ok(items)
}

/// Get one research item by id, scoped to the session (cross-session → None).
pub fn get_research_item(
    db: &Database,
    session_id: &str,
    id: i64,
) -> AppResult<Option<ResearchItem>> {
    let conn = db.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, session_id, title, url, source, snippet, snapshot, tags, created_at
         FROM research_items
         WHERE id = ?1 AND session_id = ?2",
    )?;
    let mut rows = stmt.query_map(params![id, session_id], row_to_item)?;
    match rows.next() {
        Some(Ok(item)) => Ok(Some(item)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Search a session's research items by keyword across title / url /
/// source / snippet / snapshot / tags (case-insensitive LIKE). Newest
/// first. Empty keyword returns no rows.
pub fn search_research_items(
    db: &Database,
    session_id: &str,
    keyword: &str,
    limit: usize,
) -> AppResult<Vec<ResearchItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db.conn()?;
    let pattern = format!("%{keyword}%");
    let mut stmt = conn.prepare(
        "SELECT id, session_id, title, url, source, snippet, snapshot, tags, created_at
         FROM research_items
         WHERE session_id = ?1
           AND (title LIKE ?2 OR url LIKE ?2 OR source LIKE ?2 OR snippet LIKE ?2
                OR snapshot LIKE ?2 OR tags LIKE ?2)
         ORDER BY created_ms DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![session_id, pattern, limit as i64], row_to_item)?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}

/// Remove one research item (scoped to the session so cross-session
/// deletions are impossible). Returns whether a row was removed.
pub fn remove_research_item(db: &Database, session_id: &str, id: i64) -> AppResult<bool> {
    let conn = db.conn()?;
    let deleted = conn.execute(
        "DELETE FROM research_items WHERE id = ?1 AND session_id = ?2",
        params![id, session_id],
    )?;
    Ok(deleted > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        let dir = std::env::temp_dir().join(format!(
            "ddc-research-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("t.db"), false).unwrap();
        db.run_migrations().unwrap();
        db
    }

    fn seed_session(db: &Database, session_id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO sessions
                 (id, title, model, provider, status, created_at, updated_at,
                  system_prompt, turn_count, prompt_tokens, completion_tokens)
             VALUES (?1, 'test', 'deepseek', 'deepseek', 'active', ?2, ?2, '', 0, 0, 0)",
            params![session_id, now],
        )
        .unwrap();
    }

    #[test]
    fn research_items_crud_and_tag_filter() {
        let db = test_db();
        seed_session(&db, "s1");
        let id1 = insert_research_item(
            &db,
            "s1",
            "Paper A",
            "https://example.com/a",
            "scholar",
            "snippet a",
            "snapshot a",
            "ml,agents",
        )
        .unwrap();
        insert_research_item(
            &db,
            "s1",
            "Paper B",
            "https://example.com/b",
            "crossref",
            "snippet b",
            "",
            "web",
        )
        .unwrap();

        let all = list_research_items(&db, "s1", None, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].title, "Paper B", "newest first");

        let tagged = list_research_items(&db, "s1", Some("ml"), 10).unwrap();
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].title, "Paper A");

        assert!(remove_research_item(&db, "s1", id1).unwrap());
        assert!(
            !remove_research_item(&db, "s1", id1).unwrap(),
            "already removed"
        );
        // Cross-session removal is impossible.
        seed_session(&db, "s2");
        assert!(!remove_research_item(&db, "s2", id1).unwrap());
        assert_eq!(list_research_items(&db, "s1", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn research_items_keyword_search_spans_all_fields() {
        let db = test_db();
        seed_session(&db, "s1");
        seed_session(&db, "s2");
        insert_research_item(
            &db,
            "s1",
            "Transformer survey",
            "https://example.com/tr",
            "scholar",
            "attention is all you need",
            "snapshot",
            "ml",
        )
        .unwrap();
        insert_research_item(
            &db,
            "s1",
            "Other paper",
            "https://example.com/other",
            "crossref",
            "nothing here",
            "",
            "web",
        )
        .unwrap();
        insert_research_item(
            &db,
            "s2",
            "Transformer deep dive",
            "https://example.com/s2",
            "scholar",
            "snippet",
            "",
            "ml",
        )
        .unwrap();

        // Title hit.
        let hits = search_research_items(&db, "s1", "Transformer", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Transformer survey");

        // Snippet hit.
        let hits = search_research_items(&db, "s1", "attention", 10).unwrap();
        assert_eq!(hits.len(), 1);

        // Tag hit.
        let hits = search_research_items(&db, "s1", "ml", 10).unwrap();
        assert_eq!(hits.len(), 1);

        // Session scoping: s2's matching item is invisible to s1.
        assert_eq!(
            search_research_items(&db, "s1", "deep dive", 10)
                .unwrap()
                .len(),
            0
        );

        // Empty keyword → no rows.
        assert!(search_research_items(&db, "s1", "  ", 10)
            .unwrap()
            .is_empty());
    }
}
