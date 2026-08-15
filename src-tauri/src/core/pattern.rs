//! Tool pattern DSL — the single implementation of `ToolName(pattern)`
//! matching, shared by permission rules and hook conditions.
//!
//! One grammar, two consumers:
//! - Permission rules: `Read(*)`, `Bash(npm *)`, `Write(src/**)`
//! - Hook `if` conditions: `Bash(git *)`, `Write(src/**)`
//!
//! Both previously had independent parsers with subtly different glob
//! semantics. This module is the canonical implementation: tool name
//! matching is case-insensitive, argument text is extracted per tool
//! (bash → command, read_file → path, ...), and globs support `*`, `?`,
//! `**`, and `~` home expansion.

use serde_json::Value;

/// A parsed `ToolName(pattern)` expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPattern {
    /// Tool name (`*` matches any tool).
    pub tool: String,
    /// Argument pattern (`*` matches any argument text).
    pub pattern: String,
}

/// Parse a `ToolName(pattern)` expression.
///
/// Returns `None` for malformed input (no parentheses or missing `)`).
pub fn parse_tool_pattern(s: &str) -> Option<ToolPattern> {
    let s = s.trim();
    let start = s.find('(')?;
    let end = s.rfind(')')?;
    if end != s.len() - 1 {
        return None;
    }
    let tool = s[..start].trim();
    let pattern = s[start + 1..end].trim();
    if tool.is_empty() {
        return None;
    }
    Some(ToolPattern {
        tool: tool.to_string(),
        pattern: pattern.to_string(),
    })
}

/// Extract the argument text a pattern should match against for a tool.
///
/// Most tools match against a single canonical argument (`bash` → command,
/// file tools → path, `grep`/`glob` → pattern). Unknown tools match against
/// the full JSON serialization so `*` patterns still work. Tool names are
/// matched case-insensitively.
pub fn extract_arg_text(tool_name: &str, args: &Value) -> String {
    let field = match tool_name.to_ascii_lowercase().as_str() {
        "bash" => "command",
        "grep" | "glob" => "pattern",
        _ => tool_path_field(tool_name).unwrap_or(""),
    };
    if field.is_empty() {
        return args.to_string();
    }
    args.get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// The canonical argument field carrying a filesystem path for a tool,
/// when it has one.
///
/// This is the SINGLE source of truth shared by:
/// - permission rule matching (`Tool(path-pattern)` evaluates the path)
/// - filesystem validation (deny zones / traversal / symlinks)
/// - grant identity (path-scoped "always allow")
///
/// A new file-handling tool must be added here (or rules, validation and
/// grants silently miss it). `grep`/`glob` intentionally stay path-scoped
/// for validation/grants but match their search `pattern` text in rules.
pub fn tool_path_field(tool_name: &str) -> Option<&'static str> {
    match tool_name.to_ascii_lowercase().as_str() {
        // Core file tools (reads + writes).
        "read_file"
        | "read_file_pdf"
        | "read_file_image"
        | "read_file_document"
        | "write_file"
        | "edit_file"
        | "search_replace"
        | "apply_patch"
        | "list_dir"
        | "grep"
        | "glob"
        // Depwork file readers/writers.
        | "doc_read"
        | "doc_consistency"
        | "docx_search"
        | "docx_edit"
        | "docx_generate"
        | "ppt_generate"
        | "xlsx_generate"
        | "pdf_generate"
        | "research_report"
        | "card_generate"
        | "citation_link"
        | "live_doc_write" => Some("path"),
        "chart_generate" | "media_probe" | "media_convert" | "pdf_tools" => Some("output"),
        "table_process" => Some("output_path"),
        "batch_file" => Some("dir"),
        "content_pack" => Some("output_dir"),
        _ => None,
    }
}

/// Match a pattern against text with `*`, `?`, `**`, and `~` support.
///
/// Semantics:
/// - `*` matches any sequence of characters, including the empty sequence
///   (so `a*b` matches `ab`).
/// - `?` matches exactly one character.
/// - `**` matches any number of path segments — including none — so
///   `src/**` matches `src`, `src/main.rs`, and `src/a/b/c.rs`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.replace(
        '~',
        &dirs::home_dir()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default(),
    );
    let pattern: Vec<char> = pattern.replace('\\', "/").chars().collect();
    let text: Vec<char> = text.replace('\\', "/").chars().collect();
    // Bound the backtracking search: a multi-star pattern against a long
    // non-matching text is combinatorial (O(n^k)), and a project-supplied
    // rule is evaluated on every tool dispatch. Fail fast past the budget
    // instead of freezing the permission gate.
    let mut budget = GLOB_MATCH_BUDGET;
    glob_chars(&pattern, &text, &mut budget)
}

/// Upper bound on recursive glob-matching work (calls). Far above any
/// legitimate match; stops pathological patterns from hanging dispatch.
const GLOB_MATCH_BUDGET: usize = 200_000;

/// Recursive backtracking glob matcher.
///
/// `*` tries every split point in `text` (including consuming nothing),
/// which fixes the previous greedy implementation's failure on empty
/// matches (`a*b` vs `ab`). `**` matches at any `/`-separated boundary.
///
/// `budget` is decremented on every call; when exhausted the match fails
/// fast — a backtracking search over a hostile pattern must not freeze the
/// caller (ReDoS guard).
fn glob_chars(pat: &[char], txt: &[char], budget: &mut usize) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    if pat.is_empty() {
        return txt.is_empty();
    }

    // `**` — match any number of path segments (including none). The
    // immediately-following `/` is folded into the match so `**/*.rs`
    // matches `src/a.rs` even with a leading segment. (`src/**` matches
    // everything inside `src`, not `src` itself — gitignore semantics.)
    if pat.starts_with(&['*', '*']) {
        let mut rest = &pat[2..];
        if rest.first() == Some(&'/') {
            rest = &rest[1..];
        }
        // Zero segments: rest must match the whole remaining text.
        if glob_chars(rest, txt, budget) {
            return true;
        }
        // Consume up to each `/` boundary and match rest from there.
        for (i, &c) in txt.iter().enumerate() {
            if c == '/' && glob_chars(rest, &txt[i + 1..], budget) {
                return true;
            }
        }
        // `**` alone consumes everything (rest is empty).
        return rest.is_empty();
    }

    match pat[0] {
        // `*` matches any sequence including empty — try consuming nothing
        // first, then backtrack consuming one character at a time.
        '*' => {
            if glob_chars(&pat[1..], txt, budget) {
                return true;
            }
            if txt.is_empty() {
                return false;
            }
            glob_chars(pat, &txt[1..], budget)
        }
        '?' => {
            if txt.is_empty() {
                return false;
            }
            glob_chars(&pat[1..], &txt[1..], budget)
        }
        c => {
            if txt.is_empty() || txt[0] != c {
                return false;
            }
            glob_chars(&pat[1..], &txt[1..], budget)
        }
    }
}

/// Evaluate a `ToolName(pattern)` expression against a tool call.
///
/// `expr` may be the full `Bash(git *)` form or a bare tool name (matches
/// any arguments). Returns `false` for malformed expressions.
pub fn tool_pattern_matches(expr: &str, tool_name: &str, args: &Value) -> bool {
    let parsed = match parse_tool_pattern(expr) {
        Some(p) => p,
        None => {
            // Bare tool name with wildcard pattern.
            let tool = expr.trim();
            if tool.is_empty() {
                return false;
            }
            ToolPattern {
                tool: tool.to_string(),
                pattern: "*".to_string(),
            }
        }
    };

    if parsed.tool != "*" && !parsed.tool.eq_ignore_ascii_case(tool_name) {
        return false;
    }
    if parsed.pattern == "*" {
        return true;
    }

    let arg_text = extract_arg_text(tool_name, args);
    glob_match(&parsed.pattern, &arg_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_tool_pattern() {
        let p = parse_tool_pattern("Bash(git *)").unwrap();
        assert_eq!(p.tool, "Bash");
        assert_eq!(p.pattern, "git *");
        assert!(parse_tool_pattern("Bash(git *").is_none());
        assert!(parse_tool_pattern("()").is_none());
    }

    #[test]
    fn extracts_per_tool_args() {
        assert_eq!(
            extract_arg_text("bash", &json!({"command": "git status"})),
            "git status"
        );
        assert_eq!(
            extract_arg_text("read_file", &json!({"path": "src/main.rs"})),
            "src/main.rs"
        );
        assert_eq!(
            extract_arg_text("unknown_tool", &json!({"a": 1})),
            "{\"a\":1}"
        );
    }

    #[test]
    fn path_field_covers_variant_and_depwork_tools() {
        // Rule matching must evaluate the PATH argument, not the raw JSON,
        // for every path-bearing tool (core variants, apply_patch, Depwork
        // readers/writers) — otherwise path rules silently miss them.
        for (tool, key) in [
            ("read_file_pdf", "path"),
            ("read_file_image", "path"),
            ("read_file_document", "path"),
            ("apply_patch", "path"),
            ("doc_read", "path"),
            ("docx_generate", "path"),
            ("ppt_generate", "path"),
            ("xlsx_generate", "path"),
            ("pdf_generate", "path"),
            ("docx_edit", "path"),
            ("research_report", "path"),
            ("card_generate", "path"),
            ("live_doc_write", "path"),
            ("chart_generate", "output"),
            ("media_probe", "output"),
            ("media_convert", "output"),
            ("pdf_tools", "output"),
            ("table_process", "output_path"),
            ("batch_file", "dir"),
        ] {
            let mut args = serde_json::Map::new();
            args.insert(
                key.to_string(),
                serde_json::Value::String("a/b.file".into()),
            );
            assert_eq!(
                extract_arg_text(tool, &serde_json::Value::Object(args)),
                "a/b.file",
                "{tool} must extract its {key} argument"
            );
        }
    }

    #[test]
    fn glob_matches_simple() {
        assert!(glob_match("git *", "git status"));
        assert!(!glob_match("git *", "rm -rf /"));
        assert!(glob_match("src/**", "src/a/b/c.rs"));
        assert!(!glob_match("src/**", "lib/a.rs"));
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.py"));
    }

    #[test]
    fn pathological_multi_star_pattern_returns_quickly() {
        // A pattern with many `*` against a long NON-matching text is
        // combinatorial (O(n^k)) — without a budget the backtracking would
        // effectively hang the permission gate. The budget must fail fast.
        let pattern = "*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b";
        let text = "the quick brown fox jumps over the lazy dog repeatedly";
        // The budget bound makes this return (false) in bounded time. A
        // naive implementation would take exponentially long.
        let start = std::time::Instant::now();
        let result = glob_match(pattern, text);
        assert!(!result, "pathological pattern must not match");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "pathological glob must fail fast, took {:?}",
            start.elapsed()
        );
        // Legitimate patterns still match normally.
        assert!(glob_match("*a*a*b", "xaaxaab"));
    }

    #[test]
    fn glob_star_matches_empty_segment() {
        // Regression: the greedy implementation consumed at least one char
        // before checking the rest, so `a*b` never matched `ab`.
        assert!(glob_match("a*b", "ab"));
        assert!(glob_match("*b", "b"));
        assert!(glob_match("a*", "a"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn glob_star_matches_mid_segment() {
        assert!(glob_match("a*b", "axxb"));
        assert!(glob_match("*.rs", "src/main.rs"));
        assert!(!glob_match("*.rs", "main.rs.old"));
    }

    #[test]
    fn glob_double_star_matches_across_segments() {
        // Regression: the suffix was compared literally, so `**/*.rs`
        // (suffix `*.rs`) failed to match `src/a.rs`.
        assert!(glob_match("**/*.rs", "src/a.rs"));
        assert!(glob_match("**/*.rs", "a.rs"));
        assert!(!glob_match("**/*.rs", "src/a.py"));
        assert!(glob_match("src/**/*.rs", "src/a/b/c.rs"));
        assert!(glob_match("src/**", "src/deep/nested/file.rs"));
        // `**` alone matches everything.
        assert!(glob_match("**", "any/path/at/all"));
    }

    #[test]
    fn glob_question_mark_matches_single_char() {
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(!glob_match("a?c", "abbc"));
    }

    #[test]
    fn matches_tool_expression() {
        let args = json!({"command": "git status"});
        assert!(tool_pattern_matches("Bash(git *)", "bash", &args));
        assert!(!tool_pattern_matches("Bash(rm *)", "bash", &args));
        assert!(tool_pattern_matches("*", "bash", &args));
        assert!(!tool_pattern_matches("Bash(git *)", "read_file", &args));
        // Case-insensitive tool name.
        assert!(tool_pattern_matches("bash(git *)", "Bash", &args));
        // Bare tool name matches any args.
        assert!(tool_pattern_matches(
            "Bash",
            "bash",
            &json!({"command": "ls"})
        ));
    }

    #[test]
    fn rejects_malformed_expr() {
        let args = json!({});
        assert!(!tool_pattern_matches("Bash(git *", "bash", &args));
        assert!(!tool_pattern_matches("", "bash", &args));
    }
}
