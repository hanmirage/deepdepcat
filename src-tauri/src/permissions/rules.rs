//! Permission rules — pattern-based allow/deny/ask rules from config.
//!
//! Rule format: `ToolName(pattern)` where pattern is a glob.
//! Examples:
//! - `Read(*)` — allow all read operations
//! - `Read(~/.ssh/*)` — deny reading SSH keys
//! - `Bash(npm *)` — allow npm commands
//! - `Bash(rm -rf *)` — deny dangerous rm commands

use crate::core::config::PermissionsSection;
use crate::permissions::mode::PermissionMode;
use std::sync::RwLock;

/// A parsed permission rule.
#[derive(Debug, Clone)]
pub struct PermissionRule {
    pub tool: String,
    pub pattern: String,
    pub action: RuleAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    Allow,
    Deny,
    Ask,
}

/// Per-agent permission rules compiled from an agent definition
/// (`.deepdepcat/agents/*.md` `permissions:` frontmatter), using the same
/// `Tool(pattern)` syntax as the settings lists.
///
/// Precedence contract:
/// - `deny` is a hard veto — it beats every other layer (including project
///   allow and settings allow). Parent denies are merged in at spawn so a
///   restrictive parent chain always propagates to nested subagents.
/// - `ask` forces a prompt for matching calls (unless the mode auto-accepts).
/// - `allow` pre-approves matching calls (security layers still run).
#[derive(Debug, Clone, Default)]
pub struct AgentPermissionRules {
    pub deny: Vec<PermissionRule>,
    pub allow: Vec<PermissionRule>,
    pub ask: Vec<PermissionRule>,
}

impl AgentPermissionRules {
    /// Compile rules from raw `Tool(pattern)` strings.
    pub fn from_lists(allow: &[String], deny: &[String], ask: &[String]) -> Self {
        Self {
            deny: deny
                .iter()
                .filter_map(|s| parse_rule(s, RuleAction::Deny))
                .collect(),
            allow: allow
                .iter()
                .filter_map(|s| parse_rule(s, RuleAction::Allow))
                .collect(),
            ask: ask
                .iter()
                .filter_map(|s| parse_rule(s, RuleAction::Ask))
                .collect(),
        }
    }

    /// Merge inherited parent denies (raw strings) into the deny veto.
    /// Duplicates are harmless — the veto is an OR over all denies.
    pub fn merge_denies(&mut self, denies: &[String]) {
        for s in denies {
            if let Some(rule) = parse_rule(s, RuleAction::Deny) {
                self.deny.push(rule);
            }
        }
    }

    /// Whether this rule set carries no restrictions at all.
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.allow.is_empty() && self.ask.is_empty()
    }
}

/// The rule set — holds all permission rules.
pub struct RuleSet {
    rules: Vec<PermissionRule>,
    mode: RwLock<PermissionMode>,
}

impl Clone for RuleSet {
    /// Clones the rules and the CURRENT mode (read-lock snapshot) — used by
    /// the scheduler runner to gate unattended commands without sharing the
    /// live mode lock.
    fn clone(&self) -> Self {
        let mode = *self.mode.read().unwrap_or_else(|e| e.into_inner());
        Self {
            rules: self.rules.clone(),
            mode: RwLock::new(mode),
        }
    }
}

impl RuleSet {
    pub fn new(config: &PermissionsSection) -> Self {
        let mut rules = Vec::new();

        // Parse allow rules
        for rule_str in &config.allow {
            if let Some(rule) = parse_rule(rule_str, RuleAction::Allow) {
                rules.push(rule);
            }
        }

        // Parse deny rules
        for rule_str in &config.deny {
            if let Some(rule) = parse_rule(rule_str, RuleAction::Deny) {
                rules.push(rule);
            }
        }

        // Parse ask rules
        for rule_str in &config.ask {
            if let Some(rule) = parse_rule(rule_str, RuleAction::Ask) {
                rules.push(rule);
            }
        }

        Self {
            rules,
            mode: RwLock::new(PermissionMode::from_str(&config.mode)),
        }
    }

    /// Check a tool call against the rules.
    ///
    /// `read_only` is the tool's own classification (`Tool::is_read_only`),
    /// passed in by the dispatcher so the read-only classification lives in
    /// exactly one place instead of duplicated per-mode lists here. Read-only
    /// mode trusts it; default mode auto-approves read-only tools.
    /// The rule decision under an EXPLICIT mode — used by the dispatcher
    /// when a session-scoped override (e.g. a worker spawned from a plan
    /// parent) differs from the global mode; `check` callers pass the global
    /// mode, the dispatcher passes the session's effective mode.
    pub fn check_with_mode(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        read_only: bool,
        mode: PermissionMode,
    ) -> RuleAction {
        // In read-only mode, deny all non-read tools (read-only tools allowed).
        if mode.is_read_only() {
            return if read_only {
                RuleAction::Allow
            } else {
                RuleAction::Deny
            };
        }

        // In auto-accept mode, allow everything (except explicitly denied)
        if mode.auto_accepts() {
            for rule in &self.rules {
                if rule.action == RuleAction::Deny && rule.matches(tool_name, args) {
                    return RuleAction::Deny;
                }
            }
            return RuleAction::Allow;
        }

        // AcceptEdits mode: auto-approve file-edit tools, everything else
        // falls through to the normal rule/ask logic. This is mode semantics
        // (edits only), so the tool list stays here on purpose. Deliberately
        // EXCLUDES destructive operations (delete/rename/remove) — "accept
        // edits" must never silently delete or move files.
        if mode == PermissionMode::AcceptEdits && Self::is_accept_edits_tool(tool_name) {
            return RuleAction::Allow;
        }

        // Default mode: check rules in order — deny > ask > allow (the
        // documented priority; project and agent layers already follow it,
        // settings now does too). An explicit `ask` rule therefore beats a
        // broader `allow` rule: the user's narrower concern wins.
        // Check deny rules first.
        for rule in &self.rules {
            if rule.action == RuleAction::Deny && rule.matches(tool_name, args) {
                return RuleAction::Deny;
            }
        }

        // Check ask rules before allow rules.
        for rule in &self.rules {
            if rule.action == RuleAction::Ask && rule.matches(tool_name, args) {
                return RuleAction::Ask;
            }
        }

        // Check allow rules.
        for rule in &self.rules {
            if rule.action == RuleAction::Allow && rule.matches(tool_name, args) {
                return RuleAction::Allow;
            }
        }

        // Default: allow read operations, ask for everything else.
        if read_only {
            RuleAction::Allow
        } else {
            RuleAction::Ask
        }
    }

    /// Whether a tool belongs to the accept-edits auto-approve set.
    ///
    /// Every edit-evidence tool the loop tracks (edit_file / write_file /
    /// search_replace / apply_patch) plus directory creation. Destructive
    /// operations (delete/rename/remove) are deliberately excluded — "accept
    /// edits" must never silently delete or move files.
    pub(crate) fn is_accept_edits_tool(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "write_file" | "edit_file" | "search_replace" | "apply_patch" | "create_dir"
        )
    }

    /// Get the current permission mode.
    pub fn mode(&self) -> PermissionMode {
        *self.mode.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether an EXPLICIT `ask` rule matches this call.
    ///
    /// Only meaningful when [`RuleSet::check`] already returned
    /// [`RuleAction::Ask`] — it distinguishes "asked by an explicit rule"
    /// (user policy, must prompt) from the default-mode fallback ask (which
    /// built-in safe-command auto-allow may satisfy).
    pub fn ask_rule_matched(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        self.rules
            .iter()
            .any(|r| r.action == RuleAction::Ask && r.matches(tool_name, args))
    }

    /// Set the permission mode.
    pub fn set_mode(&self, mode: PermissionMode) {
        *self.mode.write().unwrap_or_else(|e| e.into_inner()) = mode;
    }

    /// Whether a bare (whole-tool) deny rule removes `tool_name` from the
    /// model's tool list entirely. A bare deny is a rule whose pattern is
    /// `*` (both `Bash` and `Bash(*)` parse that way) — scoped patterns
    /// like `Bash(rm *)` stay call-level denials.
    pub fn has_bare_tool_deny(&self, tool_name: &str) -> bool {
        self.rules.iter().any(|r| {
            r.action == RuleAction::Deny
                && r.pattern == "*"
                && r.tool.eq_ignore_ascii_case(tool_name)
        })
    }
}

impl PermissionRule {
    /// Check if this rule matches a tool call.
    ///
    /// Rule prefixes are FAMILY filters, not exact tool names — `Read(...)`
    /// must match `read_file`, `Edit(...)` must match `write_file`/`edit_file`/
    /// `apply_patch`/… (mirrors Claude Code's Read/Edit/Bash/… categories).
    /// The concrete tool name is what drives argument-text extraction, so the
    /// pattern is evaluated per concrete name. Unknown prefixes fall back to
    /// exact tool-name matching.
    pub(crate) fn matches(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        match self.tool.to_ascii_lowercase().as_str() {
            // MCP tools: names carry the server prefix (`server__tool` or
            // `mcp:server__tool`), so any `__`-qualified name counts.
            "mcp" | "mcptool" => {
                let n = tool_name.to_ascii_lowercase();
                if !(n.contains("__") || n.starts_with("mcp")) {
                    return false;
                }
                self.pattern == "*"
                    || crate::core::pattern::tool_pattern_matches(
                        &format!("{}({})", tool_name, self.pattern),
                        tool_name,
                        args,
                    )
            }
            family => match family_tool_names(family) {
                Some(names) => names.iter().any(|n| {
                    crate::core::pattern::tool_pattern_matches(
                        &format!("{}({})", n, self.pattern),
                        tool_name,
                        args,
                    )
                }),
                None => crate::core::pattern::tool_pattern_matches(
                    &format!("{}({})", self.tool, self.pattern),
                    tool_name,
                    args,
                ),
            },
        }
    }

}

/// The concrete tool names a rule-family prefix expands to.
///
/// These map the family prefixes (`Read`, `Edit`, `Grep`, `WebFetch`, …)
/// onto every concrete tool that belongs to that category. The mapping is
/// the single source of truth for rule matching — a new read/edit tool must
/// be added here (or by exact name) or rules silently miss it.
fn family_tool_names(family: &str) -> Option<&'static [&'static str]> {
    match family {
        "read" | "notebookread" => Some(&[
            "read_file",
            "read_file_pdf",
            "read_file_image",
            "read_file_document",
            // Depwork document readers share the same Read semantics.
            "doc_read",
            "doc_consistency",
            "docx_search",
        ]),
        "edit" | "write" | "notebookedit" => Some(&[
            "write_file",
            "edit_file",
            "search_replace",
            "apply_patch",
            "delete_file",
            "rename_file",
            "create_dir",
            "remove_dir",
            // Depwork file writers share the same Edit semantics.
            "docx_edit",
            "docx_generate",
            "ppt_generate",
            "xlsx_generate",
            "pdf_generate",
            "research_report",
            "card_generate",
            "citation_link",
            "live_doc_write",
            "chart_generate",
            "media_probe",
            "media_convert",
            "pdf_tools",
            "table_process",
            "batch_file",
            "content_pack",
        ]),
        "bash" => Some(&["bash"]),
        "grep" | "glob" => Some(&["grep", "glob", "code_search"]),
        "listdir" | "list" => Some(&["list_dir"]),
        "webfetch" => Some(&["web_fetch", "web_fetch_depwork"]),
        "websearch" => Some(&["web_search"]),
        _ => None,
    }
}

/// Parse a rule string like "Read(*)" or "Bash(npm *)".
pub(crate) fn parse_rule(s: &str, action: RuleAction) -> Option<PermissionRule> {
    let s = s.trim();
    if let Some(open) = s.find('(') {
        if s.ends_with(')') {
            let tool = s[..open].trim();
            let pattern = s[open + 1..s.len() - 1].trim();
            return Some(PermissionRule {
                tool: tool.to_string(),
                pattern: pattern.to_string(),
                action,
            });
        }
    }
    // No parentheses — treat the whole string as a tool name with wildcard pattern
    Some(PermissionRule {
        tool: s.to_string(),
        pattern: "*".to_string(),
        action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(s: &str, action: RuleAction) -> PermissionRule {
        parse_rule(s, action).unwrap()
    }

    #[test]
    fn read_family_matches_all_read_tools() {
        let r = rule("Read(**/src/**)", RuleAction::Allow);
        assert!(r.matches("read_file", &json!({ "path": "a/src/b.rs" })));
        assert!(r.matches("read_file_pdf", &json!({ "path": "x/src/y.pdf" })));
        assert!(!r.matches("read_file", &json!({ "path": "a/lib/b.rs" })));
        // Must NOT match non-read tools.
        assert!(!r.matches("bash", &json!({ "command": "read_file" })));
        assert!(!r.matches("write_file", &json!({ "path": "a/src/b.rs" })));
    }

    #[test]
    fn family_rules_match_exact_path_patterns_on_variant_tools() {
        // The argument extraction must use the tool's PATH field (not the
        // raw JSON) so exact/suffix patterns work for read_file_pdf,
        // apply_patch and Depwork tools — previously these only matched
        // accidentally when `*`/`**` swallowed the JSON wrapper.
        let deny_env = rule("Read(**/.env)", RuleAction::Deny);
        assert!(deny_env.matches("read_file_pdf", &json!({ "path": "a/.env" })));
        assert!(deny_env.matches("read_file_image", &json!({ "path": "a/.env" })));
        assert!(deny_env.matches("doc_read", &json!({ "path": "a/.env" })));
        assert!(!deny_env.matches("read_file_pdf", &json!({ "path": "a/report.pdf" })));

        let deny_exact = rule("Edit(src/secrets.json)", RuleAction::Deny);
        assert!(deny_exact.matches(
            "apply_patch",
            &json!({ "path": "src/secrets.json", "patch": "x" })
        ));
        assert!(deny_exact.matches(
            "docx_edit",
            &json!({ "path": "src/secrets.json", "action": "replace" })
        ));
        assert!(!deny_exact.matches(
            "apply_patch",
            &json!({ "path": "src/other.rs", "patch": "x" })
        ));
    }

    #[test]
    fn settings_ask_beats_allow() {
        // Module contract: deny > ask > allow. An explicit ask rule must
        // win over a broader allow rule in the settings layer too.
        let section = crate::core::config::PermissionsSection {
            allow: vec!["Bash(git *)".to_string()],
            ask: vec!["Bash(git push *)".to_string()],
            ..Default::default()
        };
        let rs = RuleSet::new(&section);
        assert_eq!(
            rs.check_with_mode(
                "bash",
                &json!({ "command": "git status" }),
                false,
                rs.mode()
            ),
            RuleAction::Allow
        );
        assert_eq!(
            rs.check_with_mode(
                "bash",
                &json!({ "command": "git push origin main" }),
                false,
                rs.mode()
            ),
            RuleAction::Ask
        );
    }

    #[test]
    fn edit_family_matches_edit_tools() {
        let r = rule("Edit(*)", RuleAction::Deny);
        for (tool, args) in [
            ("write_file", json!({ "path": "a.rs" })),
            ("edit_file", json!({ "path": "a.rs" })),
            ("search_replace", json!({ "path": "a.rs" })),
            ("apply_patch", json!({ "path": "a.rs" })),
            ("delete_file", json!({ "path": "a.rs" })),
            ("rename_file", json!({ "path": "a.rs" })),
        ] {
            assert!(r.matches(tool, &args), "{tool} must match Edit(*)");
        }
        assert!(!r.matches("read_file", &json!({ "path": "a.rs" })));
        assert!(!r.matches("bash", &json!({ "command": "mv a.rs b.rs" })));
    }

    #[test]
    fn bash_family_is_exact() {
        let r = rule("Bash(git *)", RuleAction::Allow);
        assert!(r.matches("bash", &json!({ "command": "git status" })));
        assert!(!r.matches("bash", &json!({ "command": "gitpush x" })));
        assert!(!r.matches("sh", &json!({ "command": "git status" })));
    }

    #[test]
    fn mcp_family_matches_mcp_tool_names() {
        let r = rule("MCPTool(*)", RuleAction::Deny);
        assert!(r.matches("mcp:linear__list", &json!({})));
        assert!(r.matches("notion__fetch", &json!({})));
        assert!(!r.matches("read_file", &json!({})));
    }

    #[test]
    fn unknown_prefix_falls_back_to_exact_name() {
        let r = rule("ListDir(*)", RuleAction::Allow);
        assert!(r.matches("list_dir", &json!({ "path": "/tmp" })));
        assert!(!r.matches("read_file", &json!({ "path": "/tmp" })));
    }

    #[test]
    fn glob_family_covers_grep_and_glob() {
        let r = rule("Glob(src/**)", RuleAction::Allow);
        assert!(r.matches("glob", &json!({ "pattern": "src/a/b.rs" })));
        assert!(r.matches("grep", &json!({ "pattern": "src/x.rs" })));
        assert!(!r.matches("glob", &json!({ "pattern": "lib/a.rs" })));
    }

    #[test]
    fn accept_edits_auto_approves_edits_not_destruction() {
        let section = crate::core::config::PermissionsSection {
            mode: "accept_edits".to_string(),
            ..Default::default()
        };
        let rs = RuleSet::new(&section);
        // Edits are auto-approved…
        assert_eq!(
            rs.check_with_mode("edit_file", &json!({ "path": "a.rs" }), false, rs.mode()),
            RuleAction::Allow
        );
        assert_eq!(
            rs.check_with_mode("write_file", &json!({ "path": "a.rs" }), false, rs.mode()),
            RuleAction::Allow
        );
        // search_replace / apply_patch are file edits too — the accept-edits
        // set must cover every edit-evidence tool the loop tracks.
        assert_eq!(
            rs.check_with_mode(
                "search_replace",
                &json!({ "path": "a.rs" }),
                false,
                rs.mode()
            ),
            RuleAction::Allow
        );
        assert_eq!(
            rs.check_with_mode("apply_patch", &json!({ "path": "a.rs" }), false, rs.mode()),
            RuleAction::Allow
        );
        // …but destructive operations must still ask.
        assert_eq!(
            rs.check_with_mode("delete_file", &json!({ "path": "a.rs" }), false, rs.mode()),
            RuleAction::Ask
        );
        assert_eq!(
            rs.check_with_mode("rename_file", &json!({ "path": "a.rs" }), false, rs.mode()),
            RuleAction::Ask
        );
        assert_eq!(
            rs.check_with_mode("remove_dir", &json!({ "path": "a" }), false, rs.mode()),
            RuleAction::Ask
        );
    }

    #[test]
    fn agent_rules_compile_from_lists() {
        let rules = AgentPermissionRules::from_lists(
            &["Read(**)".into(), "Bash(git *)".into()],
            &["Bash(rm *)".into()],
            &["WebFetch(*)".into()],
        );
        assert_eq!(rules.allow.len(), 2);
        assert_eq!(rules.deny.len(), 1);
        assert_eq!(rules.ask.len(), 1);
        assert!(rules.deny[0].matches("bash", &json!({ "command": "rm -rf src" })));
        assert!(rules.allow[0].matches("read_file", &json!({ "path": "src/a.rs" })));
        assert!(!rules.is_empty());
    }

    #[test]
    fn agent_rules_merge_inherited_denies() {
        let mut rules = AgentPermissionRules::default();
        assert!(rules.is_empty());
        rules.merge_denies(&["Bash(rm *)".into(), "Edit(*.env)".into()]);
        assert_eq!(rules.deny.len(), 2);
        assert!(rules.deny[0].matches("bash", &json!({ "command": "rm x" })));
        // Lenient parsing never panics: a bare string becomes a
        // tool-name rule that simply never matches real tools.
        rules.merge_denies(&["not-a-rule".into()]);
        assert_eq!(rules.deny.len(), 3);
        assert!(!rules.deny[2].matches("bash", &json!({ "command": "ls" })));
    }
}
