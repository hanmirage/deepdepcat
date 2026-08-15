//! Session learning extraction — the "自我进化" half of the memory system.
//!
//! After a turn that actually changed something, an LLM extracts 1-3
//! NON-OBVIOUS learnings (hidden relationships, quirks, workarounds,
//! build commands, architecture decisions) and we:
//! 1. store them into the memory store (category `learning`), and
//! 2. append them (deduplicated, capped) to `.deepdepcat/learnings.md` in
//!    the workspace so the project itself accumulates working knowledge.

use crate::core::error::AppResult;
use crate::core::types::ConversationItem;
use crate::llm::client::LlmClient;
use crate::llm::provider::{LlmProvider, LlmRequest};
use std::path::{Path, PathBuf};

/// Maximum learnings per extraction pass.
pub const MAX_LEARNINGS_PER_TURN: usize = 3;
/// Maximum bullet lines kept in the learnings file (oldest dropped).
pub const MAX_LEARNINGS_FILE_LINES: usize = 200;
/// How many recent conversation items feed one extraction pass.
const EXTRACTION_CONTEXT_ITEMS: usize = 30;
/// Marker the LLM returns when nothing is worth persisting.
const NO_LEARNINGS_MARKER: &str = "NO_LEARNINGS";

/// Parse numbered/bulleted learnings from the LLM response.
pub fn parse_learnings(text: &str) -> Vec<String> {
    if text.contains(NO_LEARNINGS_MARKER) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let bullet = line.strip_prefix("- ").or_else(|| line.strip_prefix("* "));
        let numbered = line
            .strip_prefix(|c: char| c.is_ascii_digit())
            .and_then(|rest| rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")));
        // Only bullet/numbered lines count — plain prose from the model is
        // not a learning.
        let Some(cleaned) = bullet.or(numbered) else {
            continue;
        };
        let cleaned = cleaned.trim().trim_matches('`').trim();
        if cleaned.is_empty() || cleaned.eq_ignore_ascii_case(NO_LEARNINGS_MARKER) {
            continue;
        }
        if out.len() < MAX_LEARNINGS_PER_TURN {
            out.push(cleaned.to_string());
        }
    }
    out
}

/// Drop learnings that duplicate existing ones (case-insensitive
/// containment either direction).
pub fn dedupe_learnings(new: Vec<String>, existing: &[String]) -> Vec<String> {
    let existing_lower: Vec<String> = existing.iter().map(|e| e.to_lowercase()).collect();
    new.into_iter()
        .filter(|item| {
            let lower = item.to_lowercase();
            !existing_lower
                .iter()
                .any(|e| e.contains(&lower) || lower.contains(e))
        })
        .collect()
}

/// Truncate to `max` items, dropping the OLDEST first (newest survive).
pub fn cap_learnings(list: &mut Vec<String>, max: usize) {
    if list.len() > max {
        let drop = list.len() - max;
        list.drain(..drop);
    }
}

/// Path of the workspace learnings file (`.deepdepcat/learnings.md`).
pub fn learnings_path(workspace: Option<&Path>) -> Option<PathBuf> {
    workspace.map(|w| w.join(".deepdepcat").join("learnings.md"))
}

/// Read the raw learnings file, lossily decoded (UTF-8 → GBK → UTF-16).
/// A strict UTF-8-only read would return empty on a Chinese-Windows GBK/ANSI
/// file — and the persist path would then rewrite the file from only the new
/// bullets, silently wiping every prior learning.
fn read_learnings_raw(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => crate::core::encoding::decode_native_output(&bytes),
        Err(_) => String::new(),
    }
}

/// Read existing learnings bullets from the file (if any). Bullets are `- `
/// prefixed lines; non-bullet lines (headings, comments, prose the user
/// hand-wrote) are ignored for dedup but never discarded by the writer.
pub fn read_learnings(path: &Path) -> Vec<String> {
    read_learnings_raw(path)
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Append new learnings to the workspace file — append-only, dedup + cap on
/// BULLET lines only. Every existing line (including non-bullet user content
/// like headings or comments) is preserved; the file is never rebuilt from
/// scratch, so a hand-edited or non-UTF-8 learnings.md cannot be wiped.
pub fn persist_learnings_file(path: &Path, new: &[String]) -> std::io::Result<usize> {
    if new.is_empty() {
        return Ok(0);
    }
    let bullets = read_learnings(path);
    let added = dedupe_learnings(new.to_vec(), &bullets);
    if added.is_empty() {
        return Ok(0);
    }
    let added_len = added.len();

    // Start from the EXISTING raw content (lossily decoded) so headings,
    // comments, and any non-bullet user content survive the write.
    let mut content = read_learnings_raw(path);
    if content.is_empty() {
        content.push_str("# Session Learnings\n\n");
    } else if !content.ends_with('\n') {
        content.push('\n');
    }
    for bullet in &added {
        content.push_str(&format!("- {bullet}\n"));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Cap: drop the OLDEST bullet lines when over budget. Non-bullet lines
    // are preserved (the cap only prunes the machine-written bullets, never
    // user-authored content).
    let mut lines: Vec<&str> = content.lines().collect();
    let mut bullet_count = lines
        .iter()
        .filter(|l| l.trim().starts_with("- "))
        .count();
    while bullet_count > MAX_LEARNINGS_FILE_LINES {
        if let Some(idx) = lines.iter().position(|l| l.trim().starts_with("- ")) {
            lines.remove(idx);
            bullet_count -= 1;
        } else {
            break;
        }
    }
    let joined = lines.join("\n") + "\n";

    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, joined)?;
    std::fs::rename(&tmp, path)?;
    Ok(added_len)
}

/// Serialize the recent conversation tail for the extraction prompt.
pub(crate) fn serialize_tail(conversation: &[ConversationItem], max_items: usize) -> String {
    let start = conversation.len().saturating_sub(max_items);
    let mut out = String::new();
    for item in &conversation[start..] {
        match item {
            ConversationItem::User(u) => {
                for part in &u.content {
                    if let crate::core::types::ContentPart::Text { text } = part {
                        out.push_str(&format!(
                            "[User]: {}\n",
                            text.chars().take(500).collect::<String>()
                        ));
                    }
                }
            }
            ConversationItem::Assistant(a) => {
                out.push_str(&format!(
                    "[Assistant]: {}\n",
                    a.content.chars().take(500).collect::<String>()
                ));
                for tc in &a.tool_calls {
                    out.push_str(&format!(
                        "  [Tool: {}({})]\n",
                        tc.name,
                        tc.arguments.chars().take(200).collect::<String>()
                    ));
                }
            }
            ConversationItem::ToolResult(tr) => {
                out.push_str(&format!(
                    "  [Result]: {}\n",
                    tr.content.chars().take(300).collect::<String>()
                ));
            }
            _ => {}
        }
    }
    out
}

/// Max conversation items per extraction chunk.
const DROP_CHUNK_ITEMS: usize = 30;
/// Max chunks extracted from a dropped segment — nearest 2 + earliest 2.
const MAX_DROP_CHUNKS: usize = 4;

/// Extract information that MUST survive a conversation compression, from a
/// segment that is about to be discarded. Unlike `extract_learnings` (which
/// scans only the tail 30 items for non-obvious lessons), this pulls out the
/// conversation-unique details a summary may not cover: unfinished work,
/// decisions, temporary constraints, key file paths. Chunked so a large
/// dropped segment doesn't blow the extraction call.
const EXTRACT_CRITICAL_SYSTEM: &str = r#"You are extracting information that MUST survive a conversation compression.
From the conversation segment, extract ONLY items that would be lost if the
segment is discarded and a summary does not cover them:
- Unfinished work items / remaining to-dos
- Important decisions and their reasons
- Temporary constraints or requirements the user stated
- Key file paths, symbols, commands, error strings
- Next steps / plans
One bullet per item ('- '), at most 8, under 120 characters, same language as
the segment. Reply NO_LEARNINGS only when the segment contains nothing worth
preserving."#;

/// Select the chunks to extract from a dropped segment: the earliest 2 (task
/// setup) + the nearest 2 (recent decisions/constraints), deduped, capped at
/// `MAX_DROP_CHUNKS`. Pure — testable without an LLM.
fn select_drop_chunks(drop: &[ConversationItem]) -> Vec<&[ConversationItem]> {
    if drop.is_empty() {
        return Vec::new();
    }
    let all: Vec<&[ConversationItem]> = drop.chunks(DROP_CHUNK_ITEMS).collect();
    let n = all.len();
    let mut unique: Vec<&[ConversationItem]> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in all[..n.min(2)].iter() {
        if seen.insert(c.as_ptr() as usize) {
            unique.push(c);
        }
    }
    if n > 2 {
        for c in all[n.saturating_sub(2)..].iter() {
            if seen.insert(c.as_ptr() as usize) {
                unique.push(c);
            }
        }
    }
    unique.truncate(MAX_DROP_CHUNKS);
    unique
}

/// Extract the critical information from a dropped conversation segment
/// (compression externalization). Chunks the segment (nearest 2 + earliest
/// 2 chunks, capped), LLM-extracts each, merges and dedups. Empty input or
/// nothing worth preserving → empty vec.
pub async fn extract_critical_from_drop(
    llm: &LlmClient,
    model: &str,
    provider: Option<&str>,
    drop: &[ConversationItem],
) -> Vec<String> {
    let unique_chunks = select_drop_chunks(drop);
    if unique_chunks.is_empty() {
        return Vec::new();
    }

    let mut all_critical: Vec<String> = Vec::new();
    for chunk in unique_chunks.iter().take(MAX_DROP_CHUNKS) {
        let text = serialize_tail(chunk, DROP_CHUNK_ITEMS);
        let request = LlmRequest {
            model: model.to_string(),
            provider: provider.map(str::to_string),
            messages: vec![ConversationItem::user(format!(
                "Conversation segment:\n\n{text}"
            ))],
            tools: vec![],
            system_prompt: EXTRACT_CRITICAL_SYSTEM.to_string(),
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(300),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };
        if let Ok(resp) = llm.complete(&request).await {
            for item in parse_learnings(&resp.content) {
                if !all_critical.contains(&item) {
                    all_critical.push(item);
                }
            }
        }
    }
    cap_learnings(&mut all_critical, 12);
    all_critical
}

/// Extract 1-3 non-obvious learnings from a conversation tail. Returns
/// `None` when the extraction call fails; an empty vec when nothing is
/// worth persisting.
pub async fn extract_learnings(
    llm: &LlmClient,
    model: &str,
    provider: Option<&str>,
    conversation: &[ConversationItem],
) -> Option<Vec<String>> {
    let tail = serialize_tail(conversation, EXTRACTION_CONTEXT_ITEMS);
    if tail.trim().is_empty() {
        return Some(Vec::new());
    }
    let system_prompt = "You extract NON-OBVIOUS learnings from an agent session \
        so the project and user can improve over time. Only include: hidden \
        relationships between files/modules, execution paths that differ from \
        how code appears, non-obvious config/env/flags, debugging breakthroughs \
        where error messages were misleading, API/tool quirks and workarounds, \
        build/test commands not in the README, architectural decisions and \
        constraints, files that must change together. NEVER include: obvious \
        facts, standard framework behavior, things already known, verbose \
        explanations, session-specific details like message ids or timestamps. \
        The session below is a fresh working session: the discoveries it \
        records are NOT already-known facts — if it contains any item from \
        the include list, you MUST extract it (one per line, '- ', at most 3, \
        under 120 characters, same language as the session). Reply exactly \
        NO_LEARNINGS only when the session genuinely contains no such item. \
        Do not over-filter: a debugging session that found a misleading error \
        cause always qualifies.";
    let request = LlmRequest {
        model: model.to_string(),
        provider: provider.map(str::to_string),
        messages: vec![ConversationItem::user(format!("Session tail:\n\n{tail}"))],
        tools: vec![],
        system_prompt: system_prompt.to_string(),
        // Background extraction must be deterministic: at 0.3 the model
        // returned NO_LEARNINGS for qualifying sessions ~50% of the time
        // (observed in the real-API smoke), starving the memory system.
        // 0.0 keeps the discovery bar but removes sampling-driven misses.
        temperature: Some(0.0),
        top_p: None,
        max_tokens: Some(300),
        stream: false,
        reasoning_effort: None,
        response_format: None,
        cache_control: None,
        user_id: None,
    };
    let response = llm.complete(&request).await.ok()?;
    Some(parse_learnings(&response.content))
}

/// Full pipeline used by the tool and the background hook: extract →
/// dedupe → store globally → persist to the workspace learnings file.
///
/// Learnings are project-level knowledge, so they are stored WITHOUT a
/// session (session_id = NULL) — that is what qualifies them for the
/// global-source boost and evergreen supplement in memory retrieval, and it
/// matches the workspace learnings file they mirror. Returns the learnings
/// that were persisted.
pub async fn run_learning_pass(
    llm: &LlmClient,
    model: &str,
    provider: Option<&str>,
    conversation: &[ConversationItem],
    memory: &crate::memory::store::MemoryStore,
    workspace: Option<&Path>,
) -> AppResult<Vec<String>> {
    let Some(learnings) = extract_learnings(llm, model, provider, conversation).await else {
        return Ok(Vec::new());
    };
    if learnings.is_empty() {
        return Ok(Vec::new());
    }
    // Dedupe against existing learnings before storing so repeated extraction
    // across sessions does not accumulate near-identical rows in the store
    // (the workspace file already dedupes on write).
    let existing: Vec<String> = memory
        .search_by_category("learning", 5000)
        .map(|ms| ms.into_iter().map(|m| m.content).collect())
        .unwrap_or_default();
    let fresh = dedupe_learnings(learnings, &existing);
    for learning in &fresh {
        let _ = memory.store(learning, "learning", None, None);
    }
    if let Some(path) = learnings_path(workspace) {
        persist_learnings_file(&path, &fresh).ok();
    }
    Ok(fresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_drop_chunks_takes_edges() {
        // Empty → no chunks.
        assert!(select_drop_chunks(&[]).is_empty());
        // Small segment (fits one chunk) → one chunk.
        let small: Vec<ConversationItem> = (0..10).map(|_| ConversationItem::user("x")).collect();
        assert_eq!(select_drop_chunks(&small).len(), 1);
        // Large segment (5+ chunks) → earliest 2 + nearest 2 = 4.
        let large: Vec<ConversationItem> =
            (0..150).map(|_| ConversationItem::user("x")).collect();
        let chunks = select_drop_chunks(&large);
        assert_eq!(chunks.len(), 4, "{chunks:?}");
        // First chunk is the segment start, last is the segment end.
        assert_eq!(chunks[0].len(), 30);
        assert_eq!(chunks[3].len(), 30);
    }

    #[test]
    fn parses_bullets_and_numbers() {
        let out = parse_learnings(
            "- hidden relation: A depends on B at runtime\n1. tsc cache must be cleared\nplain prose is ignored\n- ",
        );
        assert_eq!(out.len(), 2);
        assert!(out[0].starts_with("hidden relation"));
        assert!(out[1].starts_with("tsc cache"));
        assert!(!out.iter().any(|l| l.contains("plain prose")));
    }

    #[test]
    fn marker_returns_empty() {
        assert!(parse_learnings("NO_LEARNINGS").is_empty());
        assert!(parse_learnings("nothing here").is_empty());
    }

    #[test]
    fn dedupe_skips_contained_duplicates() {
        let existing = vec!["always use -c http.version=HTTP/1.1 on this network".to_string()];
        let new = vec![
            "use -c http.version=HTTP/1.1".to_string(),
            "brand new learning".to_string(),
        ];
        let kept = dedupe_learnings(new, &existing);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].contains("brand new"));
    }

    #[test]
    fn file_roundtrip_dedupes_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".deepdepcat").join("learnings.md");
        let added = persist_learnings_file(&path, &["first".to_string()]).unwrap();
        assert_eq!(added, 1);
        // Duplicate on the second pass is dropped.
        let added =
            persist_learnings_file(&path, &["first".to_string(), "second".to_string()]).unwrap();
        assert_eq!(added, 1);
        let bullets = read_learnings(&path);
        assert_eq!(bullets, vec!["first", "second"]);

        // Cap keeps the newest.
        let many: Vec<String> = (0..250).map(|i| format!("learning {i}")).collect();
        persist_learnings_file(&path, &many).unwrap();
        let bullets = read_learnings(&path);
        assert!(bullets.len() <= MAX_LEARNINGS_FILE_LINES);
        assert!(bullets.contains(&"learning 249".to_string()));
        assert!(!bullets.contains(&"learning 0".to_string()));
    }

    #[test]
    fn non_utf8_gbk_file_survives_append() {
        // A GBK/ANSI learnings.md (common on Chinese Windows) must NOT be
        // wiped by the next write — the old strict read_to_string returned
        // empty and the rewrite destroyed every prior learning.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".deepdepcat").join("learnings.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // GBK bytes for "中" (0xD6 0xD0) are not valid UTF-8.
        let existing = b"# Session Learnings\n\n- \xd6\xd0\xb9\xfa\xd1\xa7\xcf\xb0\n";
        std::fs::write(&path, existing).unwrap();

        let added = persist_learnings_file(&path, &["brand new".to_string()]).unwrap();
        assert_eq!(added, 1);
        // The prior GBK learning, decoded lossily, is still in the file.
        let decoded = crate::core::encoding::decode_native_output(&std::fs::read(&path).unwrap());
        assert!(
            decoded.contains("中国学习"),
            "prior GBK learning must survive the write: {decoded:?}"
        );
        assert!(read_learnings(&path).contains(&"brand new".to_string()));
    }

    #[test]
    fn hand_written_content_survives_append() {
        // A user who added a heading/comment/prose line to learnings.md must
        // not have it dropped by the next write (the old writer rebuilt the
        // file from only `- ` bullets).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".deepdepcat").join("learnings.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# Session Learnings\n\n## 2026-08-15 手动记录\n下面是我的备注。\n- older\n",
        )
        .unwrap();

        let added = persist_learnings_file(&path, &["newer".to_string()]).unwrap();
        assert_eq!(added, 1);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("手动记录"), "heading preserved: {raw}");
        assert!(raw.contains("下面是我的备注。"), "prose preserved: {raw}");
        assert!(raw.contains("- older"), "old bullet preserved: {raw}");
        assert!(raw.contains("- newer"), "new bullet appended: {raw}");
    }

    #[test]
    fn cap_prunes_only_bullet_lines() {
        // The cap must drop the OLDEST bullet lines while leaving non-bullet
        // user content (headings/comments) untouched.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".deepdepcat").join("learnings.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# Session Learnings\n\n## keep me\n").unwrap();

        let many: Vec<String> = (0..MAX_LEARNINGS_FILE_LINES + 10)
            .map(|i| format!("learning {i}"))
            .collect();
        persist_learnings_file(&path, &many).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("keep me"), "non-bullet content survives the cap");
        assert!(raw.contains("learning 249") || raw.contains("keep me"));
        let bullets = read_learnings(&path);
        assert!(bullets.len() <= MAX_LEARNINGS_FILE_LINES);
    }
}
