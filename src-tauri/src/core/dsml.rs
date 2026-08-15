//! DeepSeek DSML tool-call support.
//!
//! DeepSeek V3.2/V4 models natively express function calls in DSML, an
//! XML-like markup delimited by `｜DSML｜` tokens:
//!
//! ```text
//! <｜DSML｜tool_calls>
//!   <｜DSML｜invoke name="edit_file">
//!     <｜DSML｜parameter name="path" string="true">admin.html</｜DSML｜parameter>
//!   </｜DSML｜invoke>
//! </｜DSML｜tool_calls>
//! ```
//!
//! Some deployments stream that markup as plain assistant text instead of
//! structured `tool_calls` (real session 7a1dd319, message 143). The agent
//! loop must parse it back into real tool calls and strip it from visible
//! text; otherwise the model's intended work silently never runs.

use regex::Regex;
use serde_json::{Map, Value};

use crate::core::ids::tool_call_id;
use crate::core::types::tool::ToolCall;

/// Canonical DSML delimiter (U+FF5C fullwidth vertical bars).
const DSML: &str = "｜DSML｜";
/// Double-bar variant emitted by deepseek-v4-flash in real sessions
/// (2026-08-11: `<＜＜DSML＞＞tool_calls>` — TWO U+FF5C per side). The
/// storage/parse layers normalize it; the live stream guard matches it.
const DSML_DOUBLE: &str = "＜＜DSML＞＞";
/// ASCII-pipe variant emitted by some deployments (`<||DSML||...>`).
const DSML_ASCII: &str = "||DSML||";
/// Hangzhou-numeral variant seen in the wild (`〡DSML〡`).
const DSML_HANGZHOU: &str = "〡DSML〡";

/// Whether `text` contains DSML tool-call markup in any known variant.
pub fn has_markup(text: &str) -> bool {
    normalized(text).contains(DSML)
}

/// Parse DSML tool calls embedded in assistant text into structured
/// `ToolCall`s. Returns an empty vector when no parseable call exists
/// (including truncated blocks — a call without its closer is never
/// dispatched with half-built arguments).
pub fn parse_tool_calls(text: &str) -> Vec<ToolCall> {
    let source = normalized(text);
    if !source.contains(DSML) {
        return Vec::new();
    }

    let root_re = root_regex();
    let invoke_re = invoke_regex();
    let mut calls = Vec::new();
    let mut bodies: Vec<&str> = root_re
        .captures_iter(&source)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    if bodies.is_empty() && source.contains(&format!("<{DSML}invoke")) {
        // Some deployments emit `<｜DSML｜invoke>` without a root wrapper.
        bodies.push(&source);
    }

    for body in bodies {
        let mut rest = body;
        while let Some(cap) = invoke_re.captures(rest) {
            let whole_match = cap.get(0);
            let whole = whole_match.map(|m| m.as_str()).unwrap_or("");
            let name = cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
            let self_closing = cap.get(2).is_some();
            let inner = cap.get(3).map(|m| m.as_str()).unwrap_or("");

            // Long-form invokes must be closed; a truncated one (closer
            // missing) is protocol garbage and must never dispatch.
            if !self_closing {
                let opener_end =
                    whole_match.map_or(0, |m| m.start()) + whole.find('>').unwrap_or(whole.len());
                let closer = format!("</{DSML}invoke>");
                if !rest[opener_end..].contains(&closer) {
                    break;
                }
            }
            if name.is_empty() {
                break;
            }

            let arguments = if self_closing {
                "{}".to_string()
            } else {
                let value = parse_arguments(inner);
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
            };
            calls.push(ToolCall {
                id: tool_call_id(),
                name: name.to_string(),
                arguments,
            });
            rest = &rest[whole_match.map_or(0, |m| m.end())..];
        }
    }
    calls
}

/// Remove DSML protocol markup from text, keeping any surrounding prose.
/// A truncated trailing block is dropped to the end — like the existing
/// `<tool_calls>` handling, a cut-off block carries no recoverable prose.
pub fn strip_markup(text: &str) -> String {
    let source = normalized(text);
    if !source.contains(DSML) {
        return text.to_string();
    }

    let mut out = root_regex().replace_all(&source, " ").to_string();
    out = invoke_long_regex().replace_all(&out, " ").to_string();
    out = invoke_self_regex().replace_all(&out, " ").to_string();
    out = parameter_regex().replace_all(&out, " ").to_string();

    // Truncated tail (unclosed opener) or a stray fragment: drop from the
    // tag start onward.
    if let Some(pos) = out.find(DSML) {
        let start = out[..pos].rfind('<').unwrap_or(pos);
        out.truncate(start);
    }

    // Any remaining orphan fragments (bare closers, partial tags).
    let fragment = fragment_regex();
    out = fragment.replace_all(&out, " ").to_string();
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized(text: &str) -> String {
    let text = text
        .replace(DSML_DOUBLE, DSML)
        .replace(DSML_ASCII, DSML)
        .replace(DSML_HANGZHOU, DSML);
    normalize_plain_xml(&text)
}

/// Rewrite the plain-ASCII XML tool-call variant (`<tool_calls><invoke…>`)
/// into canonical DSML tags. Only fires when the text actually carries
/// tool-protocol tags, so normal prose is never touched. DeepSeek V4-Flash
/// (Responses) emitted this variant in a real benchmark run (2026-08-08):
/// without it the calls were never dispatched and the markup leaked.
fn normalize_plain_xml(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("<tool_calls>")
        || lower.contains("<function_calls>")
        || lower.contains("<invoke "))
    {
        return text.to_string();
    }
    text.replace("<tool_calls>", &format!("<{DSML}tool_calls>"))
        .replace("</tool_calls>", &format!("</{DSML}tool_calls>"))
        .replace("<function_calls>", &format!("<{DSML}function_calls>"))
        .replace("</function_calls>", &format!("</{DSML}function_calls>"))
        .replace("<invoke ", &format!("<{DSML}invoke "))
        .replace("<invoke>", &format!("<{DSML}invoke>"))
        .replace("</invoke>", &format!("</{DSML}invoke>"))
        .replace("<parameter ", &format!("<{DSML}parameter "))
        .replace("</parameter>", &format!("</{DSML}parameter>"))
}

fn parse_arguments(body: &str) -> Value {
    let trimmed = body.trim();
    // Format 2: direct JSON body.
    if trimmed.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return value;
        }
    }
    // Format 1: `<｜DSML｜parameter ...>` tags.
    let mut map = Map::new();
    for cap in parameter_regex().captures_iter(body) {
        let name = cap[1].trim().to_string();
        let is_string = cap[2].eq_ignore_ascii_case("true");
        let raw = cap[3].trim();
        let value = if is_string {
            Value::String(raw.to_string())
        } else {
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
        };
        map.insert(name, value);
    }
    Value::Object(map)
}

fn root_regex() -> Regex {
    let d = regex::escape(DSML);
    Regex::new(&format!(
        r"(?s)<{d}(?:tool_calls|function_calls)>(.*?)</{d}(?:tool_calls|function_calls)>"
    ))
    .expect("dsml root regex")
}

fn invoke_regex() -> Regex {
    let d = regex::escape(DSML);
    Regex::new(&format!(
        r#"(?s)<{d}invoke\s+name="([^"]+)"(?:(/>)|>(.*?)(?:</{d}invoke>|$))"#
    ))
    .expect("dsml invoke regex")
}

fn invoke_long_regex() -> Regex {
    let d = regex::escape(DSML);
    Regex::new(&format!(r"(?s)<{d}invoke[^>]*>.*?</{d}invoke>")).expect("dsml invoke long regex")
}

fn invoke_self_regex() -> Regex {
    let d = regex::escape(DSML);
    Regex::new(&format!(r"<{d}invoke[^>]*/>")).expect("dsml invoke self regex")
}

fn parameter_regex() -> Regex {
    let d = regex::escape(DSML);
    Regex::new(&format!(
        r#"(?s)<{d}parameter\s+name="([^"]+)"\s+string="([^"]+)"\s*>(.*?)</{d}parameter>"#
    ))
    .expect("dsml parameter regex")
}

fn fragment_regex() -> Regex {
    let d = regex::escape(DSML);
    Regex::new(&format!(r"</?{d}[^>]*>")).expect("dsml fragment regex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xml_parameter_format() {
        let text = concat!(
            "E2E 全部通过。 <｜DSML｜tool_calls>",
            "<｜DSML｜invoke name=\"edit_file\">",
            "<｜DSML｜parameter name=\"path\" string=\"true\">admin.html</｜DSML｜parameter>",
            "<｜DSML｜parameter name=\"new_text\" string=\"true\">hello</｜DSML｜parameter>",
            "</｜DSML｜invoke>",
            "</｜DSML｜tool_calls>"
        );
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "edit_file");
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["path"], "admin.html");
        assert_eq!(args["new_text"], "hello");
    }

    #[test]
    fn parses_json_body_format() {
        let text = concat!(
            "<｜DSML｜function_calls>",
            "<｜DSML｜invoke name=\"bash\">{ \"command\": \"node --check server.js\" }</｜DSML｜invoke>",
            "</｜DSML｜function_calls>"
        );
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["command"], "node --check server.js");
    }

    #[test]
    fn parses_multiple_invokes_and_non_string_param() {
        let text = concat!(
            "<｜DSML｜tool_calls>",
            "<｜DSML｜invoke name=\"bash\"><｜DSML｜parameter name=\"timeout_ms\" string=\"false\">5000</｜DSML｜parameter></｜DSML｜invoke>",
            "<｜DSML｜invoke name=\"list_dir\"/><｜DSML｜invoke name=\"bash\"><｜DSML｜parameter name=\"command\" string=\"true\">ls</｜DSML｜parameter></｜DSML｜invoke>",
            "</｜DSML｜tool_calls>"
        );
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].name, "bash");
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["timeout_ms"], 5000);
        assert_eq!(calls[1].name, "list_dir");
        assert_eq!(calls[1].arguments, "{}");
        assert_eq!(calls[2].name, "bash");
    }

    #[test]
    fn parses_ascii_pipe_variant() {
        let text = concat!(
            "<||DSML||tool_calls>",
            "<||DSML||invoke name=\"edit_file\">",
            "<||DSML||parameter name=\"path\" string=\"true\">a.txt</||DSML||parameter>",
            "</||DSML||invoke>",
            "</||DSML||tool_calls>"
        );
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["path"], "a.txt");
    }

    #[test]
    fn parses_invoke_without_root_wrapper() {
        let text = concat!(
            "need fix. <｜DSML｜invoke name=\"edit_file\">",
            "<｜DSML｜parameter name=\"path\" string=\"true\">x</｜DSML｜parameter>",
            "</｜DSML｜invoke>"
        );
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "edit_file");
    }

    #[test]
    fn skips_truncated_invoke() {
        let text = concat!(
            "<｜DSML｜tool_calls>",
            "<｜DSML｜invoke name=\"edit_file\">",
            "<｜DSML｜parameter name=\"path\" string=\"true\">x</｜DSML｜parameter>"
        );
        assert!(parse_tool_calls(text).is_empty());
    }

    #[test]
    fn plain_text_yields_no_calls() {
        assert!(parse_tool_calls("plain prose").is_empty());
        assert!(parse_tool_calls("<tool_calls><bash>x</bash></tool_calls>").is_empty());
    }

    #[test]
    fn strip_keeps_prose_and_removes_blocks() {
        let text = concat!(
            "E2E 全部通过。",
            "<｜DSML｜tool_calls><｜DSML｜invoke name=\"edit_file\"><｜DSML｜parameter name=\"path\" string=\"true\">a</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜tool_calls>",
            "完成。"
        );
        let stripped = strip_markup(text);
        assert!(!stripped.contains(DSML));
        assert!(!stripped.contains("invoke"));
        assert!(stripped.contains("E2E 全部通过"));
        assert!(stripped.contains("完成"));
    }

    #[test]
    fn strip_drops_truncated_tail() {
        let text = concat!(
            "prose <｜DSML｜tool_calls><｜DSML｜invoke name=\"edit_file\">",
            "<｜DSML｜parameter name=\"path\" string=\"true\">x</｜DSML｜parameter>"
        );
        let stripped = strip_markup(text);
        assert!(!stripped.contains(DSML));
        assert_eq!(stripped, "prose");
    }

    #[test]
    fn strip_handles_ascii_variant() {
        let text = "a <||DSML||tool_calls>x</||DSML||tool_calls> b";
        let stripped = strip_markup(text);
        assert!(!stripped.contains(DSML_ASCII));
        assert!(stripped.contains('a'));
        assert!(stripped.contains('b'));
    }

    #[test]
    fn parses_and_strips_double_bar_variant() {
        // deepseek-v4-flash emitted DOUBLE fullwidth bars per side
        // (`<＜＜DSML＞＞tool_calls>`, two U+FF5C) in a real session.
        let text = concat!(
            "前 <＜＜DSML＞＞tool_calls>",
            "<＜＜DSML＞＞invoke name=\"read_file\">",
            "<＜＜DSML＞＞parameter name=\"path\" string=\"true\">a.css</＜＜DSML＞＞parameter>",
            "</＜＜DSML＞＞invoke>",
            "</＜＜DSML＞＞tool_calls> 后"
        );
        assert!(has_markup(text));
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        let stripped = strip_markup(text);
        assert!(stripped.contains('前'));
        assert!(stripped.contains('后'));
        assert!(!stripped.contains("DSML"));
        assert!(!stripped.contains("invoke"));
    }

    #[test]
    fn all_known_dsml_variants_parse_and_strip() {
        // Regression matrix — every delimiter variant seen in the wild:
        // 1. canonical single fullwidth bars
        // 2. ASCII pipes (||DSML||)
        // 3. Hangzhou numerals (〡DSML〡)
        // 4. double fullwidth bars (＜＜DSML＞＞ — deepseek-v4-flash 2026-08-11)
        // 5. plain XML with NO DSML delimiter (v4-flash Responses 2026-08-08)
        // A new variant MUST be added here so the three consumers
        // (parse / storage strip / live stream guard) stay in sync.
        let variants = [
            "<｜DSML｜tool_calls><｜DSML｜invoke name=\"read_file\">\
             <｜DSML｜parameter name=\"path\" string=\"true\">a</｜DSML｜parameter>\
             </｜DSML｜invoke></｜DSML｜tool_calls>",
            "<||DSML||tool_calls><||DSML||invoke name=\"read_file\"/></||DSML||tool_calls>",
            "<〡DSML〡tool_calls><〡DSML〡invoke name=\"read_file\"/></〡DSML〡tool_calls>",
            "<＜＜DSML＞＞tool_calls><＜＜DSML＞＞invoke name=\"read_file\"/></＜＜DSML＞＞tool_calls>",
            "<tool_calls><invoke name=\"read_file\">\
             <parameter name=\"path\" string=\"true\">a</parameter>\
             </invoke></tool_calls>",
        ];
        for (i, text) in variants.iter().enumerate() {
            assert!(has_markup(text), "variant {i} must be detected");
            let calls = parse_tool_calls(text);
            assert_eq!(calls.len(), 1, "variant {i} must parse one call");
            assert_eq!(calls[0].name, "read_file", "variant {i} call name");
            let stripped = strip_markup(text);
            assert!(!stripped.contains("DSML"), "variant {i} strips DSML");
            assert!(!stripped.contains("invoke"), "variant {i} strips invoke");
        }
    }

    #[test]
    fn strip_handles_orphan_fragments() {
        assert_eq!(strip_markup("done </｜DSML｜tool_calls>"), "done");
        assert_eq!(strip_markup("x <｜DSML｜invoke name=\"y\"/> z"), "x z");
        assert_eq!(strip_markup("no markup"), "no markup");
    }

    #[test]
    fn parses_plain_xml_variant() {
        // Real 2026-08-08 benchmark shape (DeepSeek V4-Flash Responses):
        // `<tool_calls><invoke><parameter>` with NO DSML delimiters.
        let text = concat!(
            "我需要检查工作区。 <tool_calls>",
            "<invoke name=\"bash\">",
            "<parameter name=\"command\" string=\"true\">Get-ChildItem \"..\\work-src\"</parameter>",
            "</invoke>",
            "</tool_calls>"
        );
        assert!(has_markup(text), "plain XML variant must be detected");
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["command"], "Get-ChildItem \"..\\work-src\"");
        let stripped = strip_markup(text);
        assert!(!stripped.contains("tool_calls"));
        assert!(!stripped.contains("<invoke"));
        assert!(stripped.contains("我需要检查工作区"));
    }

    #[test]
    fn plain_xml_self_closing_and_function_calls() {
        let text = "<function_calls><invoke name=\"list_dir\"/></function_calls>";
        assert!(has_markup(text));
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[0].arguments, "{}");
    }

    #[test]
    fn plain_prose_without_protocol_tags_untouched() {
        let text = "普通文本 <div>invoke something</div> 没有工具标签";
        assert!(!has_markup(text));
        assert!(parse_tool_calls(text).is_empty());
        assert_eq!(strip_markup(text), text);
    }
}
