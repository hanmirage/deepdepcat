//! Permission grant store — durable "always allow" memories.
//!
//! When the user picks "Always allow" in the permission dialog, the backend
//! records a grant (tool + argument pattern). Subsequent calls that match a
//! grant skip the prompt entirely — unless the operation is dangerous
//! (dangerous bash commands are NEVER covered by grants).
//!
//! Grant granularity:
//! - `bash`     → `cmd:<first-word>` (e.g. `cmd:git`, `cmd:npm`)
//! - path tools → `path:<exact path>` (write/edit/search_replace/apply_patch/…)
//! - everything else → `*` (whole tool)
//!
//! Persisted as `permission_grants.json` (atomic write: tmp + rename).
//! Bounded: the oldest grants are dropped beyond the cap.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use tracing::info;

/// Maximum number of remembered grants before the oldest are dropped.
const MAX_GRANTS: usize = 200;

/// A single remembered grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionGrant {
    /// The tool this grant applies to.
    pub tool_name: String,
    /// The argument pattern (`cmd:git`, `path:/abs/file.rs`, or `*`).
    pub pattern: String,
    /// Whether a whole-tool `*` pattern was recorded through an EXPLICIT
    /// "整个工具" choice in the permission dialog. Legacy `*` grants (old
    /// builds recorded them without a scope choice) stay inert for
    /// path-bearing tools — one click must never silently exempt every
    /// future path of a file tool.
    #[serde(default)]
    pub explicit_whole_tool: bool,
    /// RFC3339 timestamp of when the grant was recorded.
    pub created_at: String,
}

/// Session-scoped grant map: session_id → (tool_name, pattern) pairs.
/// Lives on `AppState`; pure memory, never persisted.
pub type SessionGrantMap = HashMap<String, Vec<(String, String)>>;

/// The result of a user's permission decision, carried over the pending
/// request channel. `reason` is an optional user-provided rejection
/// message — it is fed back to the model so a denied call explains
/// itself instead of being retried blindly.
#[derive(Debug, Clone)]
pub struct PermissionReply {
    pub allow: bool,
    pub reason: Option<String>,
}

/// A pending permission request in flight — carries the tool metadata so
/// an "always allow" decision can be recorded as a durable grant.
pub struct PendingPermission {
    pub sender: tokio::sync::oneshot::Sender<PermissionReply>,
    pub tool_name: String,
    pub args: serde_json::Value,
    /// The session that issued the request — used to scope session-level
    /// grants ("allow for this session") and to report pending interactions.
    pub session_id: String,
}

/// The grant store — thread-safe, bounded, persisted.
pub struct PermissionGrantStore {
    grants: RwLock<Vec<PermissionGrant>>,
    path: PathBuf,
}

impl PermissionGrantStore {
    /// Load the store from disk (missing/corrupt file → empty store).
    pub fn load(app_data_dir: &std::path::Path) -> Self {
        let path = app_data_dir.join("permission_grants.json");
        let grants = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<PermissionGrant>>(&raw).ok())
            .unwrap_or_default();
        if !grants.is_empty() {
            info!(count = grants.len(), "Loaded permission grants");
        }
        Self {
            grants: RwLock::new(grants),
            path,
        }
    }

    /// Record a grant for a tool call (deduplicated by tool+pattern).
    pub fn record(&self, tool_name: &str, args: &serde_json::Value) {
        self.record_pattern(tool_name, &extract_pattern(tool_name, args));
    }

    /// Record a grant for an explicit pattern. `*` means whole-tool —
    /// dangerous bash commands are still never covered by it (see
    /// [`grants_cover`]), so a whole-bash grant is a harmless no-op.
    /// Record a whole-tool `*` grant — only meaningful after an explicit
    /// user choice in the dialog (`scope: "tool"`). For path-bearing tools
    /// this is the ONLY path to a matching `*` grant; legacy records
    /// without the flag never match those tools.
    pub fn record_whole_tool(&self, tool_name: &str) {
        self.record_pattern_with_flag(tool_name, "*", true);
    }

    /// Record a grant for an explicit pattern. `*` here is treated as a
    /// NON-explicit whole-tool record (inert for path-bearing tools) —
    /// callers wanting an explicit whole-tool grant use
    /// [`Self::record_whole_tool`].
    pub fn record_pattern(&self, tool_name: &str, pattern: &str) {
        self.record_pattern_with_flag(tool_name, pattern, false);
    }

    fn record_pattern_with_flag(&self, tool_name: &str, pattern: &str, explicit_whole_tool: bool) {
        let mut grants = self.grants.write().unwrap_or_else(|e| e.into_inner());
        grants.retain(|g| !(g.tool_name == tool_name && g.pattern == pattern));
        grants.push(PermissionGrant {
            tool_name: tool_name.to_string(),
            pattern: pattern.to_string(),
            explicit_whole_tool,
            created_at: chrono::Utc::now().to_rfc3339(),
        });
        if grants.len() > MAX_GRANTS {
            let excess = grants.len() - MAX_GRANTS;
            grants.drain(..excess);
        }
        drop(grants);
        self.persist();
    }

    /// Whether a grant covers this tool call.
    ///
    /// Dangerous bash commands are never grantable — the user must confirm
    /// those every time (mirrors the "dangerous commands always prompt"
    /// invariant). A bash command is only covered when **every** statement
    /// in it is covered: a `cmd:git` grant allows `git pull` but not the
    /// `rm -rf src` tail of `git pull && rm -rf src`.
    pub fn allows(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        let grants = self.grants.read().unwrap_or_else(|e| e.into_inner());
        let pairs: Vec<(String, String, bool)> = grants
            .iter()
            .map(|g| {
                (
                    g.tool_name.clone(),
                    g.pattern.clone(),
                    g.explicit_whole_tool,
                )
            })
            .collect();
        grants_cover(&pairs, tool_name, args)
    }

    /// Snapshot of all durable grants (settings-page audit view).
    pub fn list_grants(&self) -> Vec<PermissionGrant> {
        self.grants
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Remove ONE grant by tool + pattern — takes effect immediately (the
    /// next matching call prompts again) and persists.
    pub fn remove(&self, tool_name: &str, pattern: &str) -> bool {
        let mut grants = self.grants.write().unwrap_or_else(|e| e.into_inner());
        let before = grants.len();
        grants.retain(|g| !(g.tool_name == tool_name && g.pattern == pattern));
        let removed = grants.len() != before;
        drop(grants);
        if removed {
            self.persist();
        }
        removed
    }

    /// Clear all grants (settings-page button).
    pub fn clear(&self) {
        let mut grants = self.grants.write().unwrap_or_else(|e| e.into_inner());
        grants.clear();
        drop(grants);
        self.persist();
    }

    /// Persist atomically (tmp + rename).
    fn persist(&self) {
        let grants = self.grants.read().unwrap_or_else(|e| e.into_inner());
        if let Ok(raw) = serde_json::to_string(&*grants) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, raw).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

impl Default for PermissionGrantStore {
    fn default() -> Self {
        Self {
            grants: RwLock::new(Vec::new()),
            path: PathBuf::new(),
        }
    }
}

/// Extract the grant pattern for a tool call.
///
/// - bash → `cmd:<first word of the command>` — a missing/empty command
///   yields `cmd:` (empty first word), which can never match a real
///   statement, so a malformed call can never record a whole-bash wildcard
/// - MCP tools → `mcp:<server>` (everything before the first `__`), or
///   `mcp:<full name>` when the name has no server separator — NEVER the
///   whole-tool `*`, so one approval can't exempt a whole third-party server
/// - path tools → `path:<the exact path argument>`
/// - everything else → `*`
pub(crate) fn extract_pattern(tool_name: &str, args: &serde_json::Value) -> String {
    if tool_name == "bash" {
        if let Some(command) = args.get("command").and_then(|c| c.as_str()) {
            if let Some(word) = command.split_whitespace().next() {
                return format!("cmd:{}", normalize_bash_word(word));
            }
        }
        // No command → an empty `cmd:` pattern that never matches a real
        // statement (a statement always produces a non-empty first word).
        return "cmd:".to_string();
    }
    if is_mcp_tool(tool_name) {
        return mcp_pattern(tool_name);
    }
    if let Some(key) = path_grant_key(tool_name) {
        if let Some(path) = args.get(key).and_then(|p| p.as_str()) {
            if !path.is_empty() {
                return format!("path:{}", path);
            }
        }
    }
    "*".to_string()
}

/// The argument key carrying the path that scopes a grant for this tool.
///
/// Path-bearing tools get exact-path grant identities (`path:...`) instead
/// of a whole-tool `*` — one "always allow" must never exempt every future
/// call of a file tool (M7 granularity contract). Mirrors
/// [`crate::core::pattern::tool_path_field`] so rules, filesystem checks
/// and grants can never drift apart.
pub(crate) fn path_grant_key(tool_name: &str) -> Option<&'static str> {
    crate::core::pattern::tool_path_field(tool_name)
}

/// Human-readable description of what an "always allow" grant covers,
/// shown in the permission dialog before the user commits. Mirrors the
/// grant granularity of [`extract_pattern`]: `*` is whole-tool, bash is
/// first-word-scoped, path tools exact-path, MCP server-scoped.
pub fn describe_grant(tool_name: &str, pattern: &str) -> String {
    if pattern == "*" {
        return format!("整个工具 {tool_name} 的所有调用");
    }
    if tool_name == "bash" {
        return match pattern.strip_prefix("cmd:") {
            Some("") => "bash 命令（无法识别的调用）".to_string(),
            Some(cmd) => format!("bash 命令（{cmd} 开头）"),
            None => format!("bash 命令（{pattern}）"),
        };
    }
    if let Some(path) = pattern.strip_prefix("path:") {
        return format!("路径 {path}");
    }
    if let Some(server) = pattern.strip_prefix("mcp:") {
        if server.contains("__") {
            return format!("MCP 工具 {server}");
        }
        return format!("MCP 服务器 {server} 的所有工具");
    }
    format!("工具 {tool_name}（{pattern}）")
}

/// Normalize a bash command's first word so grants survive cosmetic
/// variation: `"Git"` vs `git`, `cargo.exe` vs `cargo`, `NPM` vs `npm`.
/// The grant stays first-word-scoped — this only collapses spelling.
fn normalize_bash_word(word: &str) -> String {
    let word = word.trim_matches('"').trim_matches('\'');
    let lower = word.to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stripped) = lower.strip_suffix(suffix) {
            if !stripped.is_empty() {
                return stripped.to_string();
            }
        }
    }
    lower
}

/// Normalize a path for grant identity: trim quotes/whitespace, unify
/// separators (`\` → `/`), resolve relative paths against the workspace,
/// strip `./` segments, collapse duplicate separators and lexically resolve
/// `..` segments. Purely lexical — no filesystem I/O.
///
/// Without this, the SAME file granted "always allow" keeps re-prompting
/// whenever the model changes spelling: `src\main.rs` vs `src/main.rs` vs
/// `./src/main.rs` vs the absolute path all produce different `path:` grant
/// patterns even though they name one file.
fn normalize_grant_path(raw: &str, workspace: Option<&std::path::Path>) -> String {
    let raw = raw.trim().trim_matches('"').trim_matches('\'');
    if raw.is_empty() {
        return String::new();
    }
    let is_absolute = std::path::Path::new(raw).is_absolute() || raw.contains(':');
    let mut full = if is_absolute {
        raw.to_string()
    } else if let Some(ws) = workspace {
        format!("{}/{}", ws.to_string_lossy().replace('\\', "/"), raw)
    } else {
        raw.to_string()
    };
    full = full.replace('\\', "/");

    let mut parts: Vec<&str> = Vec::new();
    for seg in full.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                // Lexically resolve `a/../b` → `b`; at the root the `..`
                // is kept so escaping paths are never hidden from the
                // permission identity.
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else {
                    parts.push(seg);
                }
            }
            seg => parts.push(seg),
        }
    }
    let mut out = parts.join("/");
    if full.starts_with('/') && !out.starts_with('/') {
        out.insert(0, '/');
    }
    // Windows filesystems are case-insensitive — the model alternates drive
    // letter and directory casing freely, and the same file must share one
    // grant identity. Only on Windows: lowercasing a case-sensitive
    // filesystem would merge distinct paths.
    #[cfg(windows)]
    {
        out = out.to_lowercase();
    }
    out
}

/// Normalize a tool call's arguments for the GRANT identity only.
///
/// The caller keeps the raw args for the user-facing dialog; the normalized
/// clone is what gets recorded and matched. Path tools get their `path`
/// argument normalized (workspace-relative resolution + separator/segment
/// cleanup); every other tool passes through unchanged.
pub(crate) fn normalize_grant_args(
    workspace: Option<&std::path::Path>,
    tool_name: &str,
    args: &serde_json::Value,
) -> serde_json::Value {
    let Some(key) = path_grant_key(tool_name) else {
        return args.clone();
    };
    let Some(path) = args.get(key).and_then(|p| p.as_str()) else {
        return args.clone();
    };
    let mut normalized = args.clone();
    normalized[key] = serde_json::Value::String(normalize_grant_path(path, workspace));
    normalized
}

/// Whether a tool name refers to an MCP tool: `server__tool` or the
/// `mcp:server__tool` qualified form.
fn is_mcp_tool(tool_name: &str) -> bool {
    let n = tool_name.to_ascii_lowercase();
    n.starts_with("mcp:") || n.contains("__")
}

/// Server-scoped grant pattern for an MCP tool (`mcp:linear` for
/// `mcp:linear__list`). Names without a `__` separator fall back to an
/// exact-name grant.
fn mcp_pattern(tool_name: &str) -> String {
    let name = tool_name.strip_prefix("mcp:").unwrap_or(tool_name);
    match name.split_once("__") {
        Some((server, _)) if !server.is_empty() => format!("mcp:{server}"),
        _ => format!("mcp:{name}"),
    }
}

/// Exact full-name pattern for MCP tools (`mcp:linear__list`), used to match
/// exact-tool grants alongside the server-scoped pattern.
fn mcp_full_name(tool_name: &str) -> Option<String> {
    if !is_mcp_tool(tool_name) {
        return None;
    }
    let name = tool_name.strip_prefix("mcp:").unwrap_or(tool_name);
    Some(format!("mcp:{name}"))
}

/// Whether a set of grants covers a tool call. Shared by the durable grant
/// store and session grants so the two can never drift. Each entry is a
/// `(tool_name, pattern)` pair.
///
/// - bash → every statement must be covered by the tool's `cmd:*` patterns
///   and none may be dangerous, see [`bash_patterns_cover`];
/// - MCP → a `mcp:<server>` pattern covers every tool of that server, and
///   `mcp:<server>__<tool>` covers exactly that tool;
/// - a `*` wildcard covers only the exact tool it was recorded for (a legacy
///   whole-tool MCP grant must not spread to other servers);
/// - everything else → the exact extracted pattern.
pub(crate) fn grants_cover(
    grants: &[(String, String, bool)],
    tool_name: &str,
    args: &serde_json::Value,
) -> bool {
    if tool_name == "bash" {
        if let Some(command) = args.get("command").and_then(|c| c.as_str()) {
            let patterns: Vec<String> = grants
                .iter()
                .filter(|(t, _, _)| t == "bash")
                .map(|(_, p, _)| p.clone())
                .collect();
            return bash_patterns_cover(&patterns, command);
        }
    }
    let pattern = extract_pattern(tool_name, args);
    let exact = mcp_full_name(tool_name);
    grants.iter().any(|(gt, gp, explicit)| {
        if gp == "*" {
            // Whole-tool grants only apply to tools WITHOUT a path field —
            // unless the user explicitly chose "整个工具" in the dialog
            // (legacy `*` grants stay inert for file tools).
            gt == tool_name && (*explicit || path_grant_key(gt).is_none())
        } else if gp == &pattern {
            true
        } else {
            exact.as_ref().is_some_and(|f| gp == f)
        }
    })
}

/// Whether a bash command is dangerous enough to never be grant-covered.
fn is_dangerous_command(command: &str) -> bool {
    use crate::permissions::security::bash::{BashSecurity, Severity};
    matches!(BashSecurity::new().analyze(command), Severity::Dangerous(_))
}

/// Whether a durable grant list covers a bash command.
///
/// The command is split into statements; **every** statement must be covered
/// by a grant for the whole command to be pre-approved. A single covered
/// Whether a set of grant patterns (`cmd:git`, …) covers every statement of
/// a bash command. Used for both durable and session grants.
///
/// A statement is covered when its first-word pattern appears in `patterns`
/// and it is not dangerous. Dangerous statements are never grant-covered —
/// they always reach the explicit permission check. A `*` wildcard NEVER
/// covers bash: a whole-bash grant would let one "always allow" click
/// exempt every future command; only the exact `cmd:<first-word>` pattern
/// matches.
pub(crate) fn bash_patterns_cover(patterns: &[String], command: &str) -> bool {
    use crate::permissions::security::bash::BashSecurity;
    for segment in BashSecurity::new().split_commands(command) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if is_dangerous_command(segment) {
            return false;
        }
        let pattern = extract_pattern("bash", &serde_json::json!({ "command": segment }));
        if !patterns.iter().any(|p| p == &pattern) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> PermissionGrantStore {
        PermissionGrantStore::default()
    }

    #[test]
    fn list_and_remove_single_grant() {
        let dir = std::env::temp_dir().join(format!(
            "ddc-grant-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = PermissionGrantStore::load(&dir);
        store.record("bash", &json!({"command": "git status"}));
        store.record("bash", &json!({"command": "npm test"}));

        let grants = store.list_grants();
        assert_eq!(grants.len(), 2);
        assert!(store.remove("bash", "cmd:git"));
        assert_eq!(store.list_grants().len(), 1);
        // Removing a non-existent grant returns false and changes nothing.
        assert!(!store.remove("bash", "cmd:git"));
        // Reload from disk — the removal persisted.
        let reloaded = PermissionGrantStore::load(&dir);
        assert_eq!(reloaded.list_grants().len(), 1);
        assert_eq!(reloaded.list_grants()[0].pattern, "cmd:npm");
    }

    #[test]
    fn bash_grant_is_command_word_scoped() {
        let s = store();
        s.record("bash", &json!({ "command": "git status --short" }));
        assert!(s.allows("bash", &json!({ "command": "git log --oneline" })));
        assert!(!s.allows("bash", &json!({ "command": "npm install" })));
    }

    #[test]
    fn path_grant_is_exact_path() {
        let s = store();
        s.record("edit_file", &json!({ "path": "src/main.rs" }));
        assert!(s.allows("edit_file", &json!({ "path": "src/main.rs" })));
        assert!(!s.allows("edit_file", &json!({ "path": "src/other.rs" })));
    }

    #[test]
    fn other_tools_are_whole_tool() {
        let s = store();
        s.record("todo_write", &json!({ "todos": [] }));
        assert!(s.allows("todo_write", &json!({ "todos": [{ "a": 1 }] })));
    }

    #[test]
    fn mcp_grant_is_server_scoped_not_whole_tool() {
        let s = store();
        s.record("mcp:linear__list", &json!({}));
        // A server grant covers every tool of the same server…
        assert!(s.allows("mcp:linear__get", &json!({})));
        assert!(s.allows("linear__search", &json!({})));
        // …but NOT other servers or non-MCP tools.
        assert!(!s.allows("mcp:notion__list", &json!({})));
        assert!(!s.allows("read_file", &json!({ "path": "x" })));
        // No whole-tool `*` grant exists for MCP tools: the recorded pattern
        // is server-scoped, so an unrelated tool never matches it.
        let patterns = vec![(
            "mcp:linear__list".to_string(),
            extract_pattern("mcp:linear__list", &json!({})),
            false,
        )];
        assert_eq!(patterns[0].1, "mcp:linear");
        assert!(!grants_cover(&patterns, "mcp:notion__list", &json!({})));
        assert!(grants_cover(&patterns, "mcp:linear__create", &json!({})));
    }

    #[test]
    fn mcp_exact_tool_grant_matches_that_tool_only() {
        let s = store();
        s.record("linear__create", &json!({}));
        // Unqualified name: server prefix is still extracted from `__`.
        assert!(s.allows("linear__get", &json!({})));
        assert!(!s.allows("other__get", &json!({})));
    }

    #[test]
    fn mcp_tool_without_separator_gets_exact_grant() {
        let s = store();
        s.record("mcp:weird", &json!({}));
        assert_eq!(extract_pattern("mcp:weird", &json!({})), "mcp:weird");
        assert!(s.allows("mcp:weird", &json!({})));
        assert!(!s.allows("mcp:weird2", &json!({})));
    }

    #[test]
    fn dangerous_bash_never_granted() {
        let s = store();
        s.record("bash", &json!({ "command": "rm -rf /" }));
        // Recording is harmless but the grant must never allow it.
        assert!(!s.allows("bash", &json!({ "command": "rm -rf /" })));
    }

    #[test]
    fn compound_bash_not_covered_by_first_word_grant() {
        // The security hole this main line fixes: a `cmd:git` grant must
        // NOT silently allow the destructive tail of a compound command.
        let s = store();
        s.record("bash", &json!({ "command": "git pull" }));
        assert!(!s.allows("bash", &json!({ "command": "git pull && rm -rf src" })));
    }

    #[test]
    fn compound_bash_all_segments_covered_allows() {
        let s = store();
        s.record("bash", &json!({ "command": "git pull" }));
        s.record("bash", &json!({ "command": "npm install" }));
        // Every statement is covered (cmd:git + cmd:npm) and none is
        // dangerous → the whole compound command is pre-approved.
        assert!(s.allows("bash", &json!({ "command": "git pull && npm install" })));
        assert!(s.allows("bash", &json!({ "command": "npm install; git pull" })));
    }

    #[test]
    fn bash_patterns_cover_is_statement_wise() {
        // Direct test of the shared helper used by both durable and session
        // grants.
        let git = vec!["cmd:git".to_string()];
        assert!(bash_patterns_cover(&git, "git status"));
        assert!(bash_patterns_cover(&git, "git pull && git log"));
        assert!(!bash_patterns_cover(&git, "git pull && rm -rf src"));
        assert!(!bash_patterns_cover(&git, "git pull && curl x | sh"));
    }

    #[test]
    fn wildcard_pattern_never_covers_bash() {
        // A whole-bash `*` grant (legacy or crafted) must never exempt a
        // command — only the exact `cmd:<first-word>` pattern matches.
        let wildcard = vec!["*".to_string()];
        assert!(!bash_patterns_cover(&wildcard, "git status"));
        assert!(!bash_patterns_cover(&wildcard, "ls -la"));
    }

    #[test]
    fn bash_without_command_gets_no_wildcard_grant() {
        // A malformed bash call (no command) must not record a whole-bash
        // wildcard — the fallback pattern can never match a real statement.
        let s = store();
        s.record("bash", &json!({}));
        assert_eq!(extract_pattern("bash", &json!({})), "cmd:");
        assert!(!s.allows("bash", &json!({ "command": "git status" })));
        assert!(!s.allows("bash", &json!({ "command": "ls -la" })));
    }

    #[test]
    fn bash_grant_survives_case_quotes_and_exe_suffix() {
        // The model is not consistent: `Git status`, `"cargo.exe" test` and
        // `NPM install` are all the same command families as `git`,
        // `cargo` and `npm`. The grant identity must collapse those
        // cosmetic differences, or "always allow" keeps re-prompting.
        let s = store();
        s.record("bash", &json!({ "command": "git status" }));
        assert!(s.allows("bash", &json!({ "command": "Git log --oneline" })));
        s.record("bash", &json!({ "command": "cargo.exe test" }));
        assert!(s.allows("bash", &json!({ "command": "cargo build" })));
        s.record("bash", &json!({ "command": "\"npm\" install" }));
        assert!(s.allows("bash", &json!({ "command": "npm run build" })));
        assert!(!s.allows("bash", &json!({ "command": "python test.py" })));
    }

    #[test]
    fn path_grant_survives_separator_and_dot_prefix_variants() {
        // Same file, three spellings — all must share one grant identity.
        let s = store();
        let identity = |args: &serde_json::Value| {
            crate::permissions::grant_store::normalize_grant_args(None, "edit_file", args)
        };
        s.record("edit_file", &identity(&json!({ "path": "./src/main.rs" })));
        assert!(s.allows("edit_file", &identity(&json!({ "path": "src/main.rs" }))));
        assert!(s.allows("edit_file", &identity(&json!({ "path": "src\\main.rs" }))));
        assert!(s.allows(
            "edit_file",
            &identity(&json!({ "path": "src\\nested\\..\\main.rs" }))
        ));
        assert!(!s.allows("edit_file", &identity(&json!({ "path": "src/other.rs" }))));
    }

    #[test]
    fn path_grant_normalizes_relative_against_workspace() {
        // With a workspace, a relative grant and the absolute spelling of
        // the same file normalize to one identity.
        let ws = std::path::Path::new(r"D:\proj");
        let relative = crate::permissions::grant_store::normalize_grant_args(
            Some(ws),
            "write_file",
            &json!({ "path": "src/util.rs" }),
        );
        let absolute = crate::permissions::grant_store::normalize_grant_args(
            Some(ws),
            "write_file",
            &json!({ "path": r"D:\proj\src\util.rs" }),
        );
        assert_eq!(
            relative.get("path").and_then(|v| v.as_str()),
            absolute.get("path").and_then(|v| v.as_str()),
            "relative and absolute spellings must produce one grant identity"
        );
        assert_eq!(
            relative.get("path").and_then(|v| v.as_str()),
            Some(if cfg!(windows) {
                r"d:/proj/src/util.rs"
            } else {
                r"D:/proj/src/util.rs"
            })
        );

        let s = store();
        s.record("write_file", &relative);
        assert!(s.allows("write_file", &absolute));
    }

    #[test]
    fn depwork_writers_get_path_scoped_grants() {
        // A "always allow" on a Depwork file tool must remember the exact
        // target path, never the whole tool (M7 granularity contract).
        let s = store();
        s.record("docx_generate", &json!({ "path": "out/report.docx" }));
        assert_eq!(
            extract_pattern("docx_generate", &json!({ "path": "out/report.docx" })),
            "path:out/report.docx"
        );
        assert!(s.allows("docx_generate", &json!({ "path": "out/report.docx" })));
        assert!(!s.allows("docx_generate", &json!({ "path": "out/other.docx" })));

        s.record("chart_generate", &json!({ "output": "charts/revenue.svg" }));
        assert_eq!(
            extract_pattern("chart_generate", &json!({ "output": "charts/revenue.svg" })),
            "path:charts/revenue.svg"
        );
        assert!(s.allows("chart_generate", &json!({ "output": "charts/revenue.svg" })));
        assert!(!s.allows("chart_generate", &json!({ "output": "charts/costs.svg" })));
    }

    #[test]
    fn windows_grant_path_is_case_insensitive() {
        #[cfg(windows)]
        {
            let s = store();
            let lower = normalize_grant_args(
                None,
                "edit_file",
                &json!({ "path": r"D:\Proj\Src\Main.Rs" }),
            );
            let upper = normalize_grant_args(
                None,
                "edit_file",
                &json!({ "path": r"d:\proj\src\main.rs" }),
            );
            assert_eq!(lower, upper, "Windows grant identity must ignore case");
            s.record("edit_file", &lower);
            assert!(s.allows("edit_file", &upper));
        }
    }

    #[test]
    fn normalize_grant_args_only_touches_path_tools() {
        let args = json!({ "command": "git status" });
        let out = crate::permissions::grant_store::normalize_grant_args(
            Some(std::path::Path::new(r"D:\proj")),
            "bash",
            &args,
        );
        assert_eq!(out, args, "bash args pass through untouched");

        // Non-path tools keep their whole payload.
        let todo = json!({ "todos": [{ "text": "x" }] });
        let out = crate::permissions::grant_store::normalize_grant_args(
            Some(std::path::Path::new(r"D:\proj")),
            "todo_write",
            &todo,
        );
        assert_eq!(out, todo);
    }

    #[test]
    fn whole_tool_grant_covers_every_call_of_that_tool() {
        let s = store();
        // Only an EXPLICIT "整个工具" dialog choice produces a matching
        // whole-tool grant for a path-bearing tool.
        s.record_whole_tool("edit_file");
        assert!(s.allows("edit_file", &json!({ "path": "src/a.rs" })));
        assert!(s.allows("edit_file", &json!({ "path": "src/b/c.rs" })));
        // Whole-tool grants stay tool-scoped — other tools unaffected.
        assert!(!s.allows("write_file", &json!({ "path": "src/a.rs" })));
    }

    #[test]
    fn legacy_whole_tool_grants_are_inert_for_path_tools() {
        // Old builds recorded `*` for path tools WITHOUT a scope choice
        // (Depwork writers especially). Those records must not silently
        // exempt every future path — only explicit whole-tool grants match.
        let s = store();
        s.record_pattern("edit_file", "*");
        s.record_pattern("docx_generate", "*");
        assert!(!s.allows("edit_file", &json!({ "path": "src/a.rs" })));
        assert!(!s.allows("docx_generate", &json!({ "path": "out/report.docx" })));
        // Non-path tools keep their legacy whole-tool grants.
        s.record_pattern("todo_write", "*");
        assert!(s.allows("todo_write", &json!({ "todos": [] })));
    }

    #[test]
    fn whole_tool_grant_never_covers_bash() {
        // A crafted `*` grant must not exempt a single command: bash stays
        // first-word-scoped and dangerous statements always prompt.
        let s = store();
        s.record_pattern("bash", "*");
        assert!(!s.allows("bash", &json!({ "command": "git status" })));
        assert!(!s.allows("bash", &json!({ "command": "ls -la" })));
    }

    #[test]
    fn record_pattern_dedupes_and_persists() {
        let dir = std::env::temp_dir().join(format!(
            "ddc-grant-pattern-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let s = PermissionGrantStore::load(&dir);
        s.record_pattern("todo_write", "*");
        s.record_pattern("todo_write", "*");
        assert_eq!(s.list_grants().len(), 1);
        let reloaded = PermissionGrantStore::load(&dir);
        assert_eq!(reloaded.list_grants().len(), 1);
        assert_eq!(reloaded.list_grants()[0].pattern, "*");
    }

    #[test]
    fn describe_grant_covers_all_granularities() {
        assert_eq!(
            describe_grant("edit_file", "path:D:/proj/src/main.rs"),
            "路径 D:/proj/src/main.rs"
        );
        assert_eq!(describe_grant("bash", "cmd:git"), "bash 命令（git 开头）");
        assert_eq!(
            describe_grant("mcp:linear__list", "mcp:linear"),
            "MCP 服务器 linear 的所有工具"
        );
        assert_eq!(
            describe_grant("mcp:linear__list", "mcp:linear__list"),
            "MCP 工具 linear__list"
        );
        assert_eq!(
            describe_grant("todo_write", "*"),
            "整个工具 todo_write 的所有调用"
        );
    }
}
