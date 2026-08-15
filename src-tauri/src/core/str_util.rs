//! String utilities for safe UTF-8 operations.

/// Truncate a string at a character boundary, never panicking on multi-byte chars.
///
/// Returns a slice of `s` that is at most `max_bytes` bytes long, adjusted
/// backwards to the nearest UTF-8 character boundary. If `max_bytes` exceeds
/// the string length, the entire string is returned.
pub fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Default cap for tool results injected into the conversation (characters).
pub const TOOL_OUTPUT_MAX_CHARS: usize = 32_000;

/// Tail hint appended after a truncated tool result so the model knows
/// output was cut, not that the tool returned less.
pub const TOOL_OUTPUT_TRUNCATED_HINT: &str = "...(output truncated)";

/// Prefix of the hint appended when an over-limit tool output is spilled to a
/// temp file instead of truncated — the model can read the full output back
/// via read_file on the returned absolute path.
pub const TOOL_OUTPUT_SPILLED_HINT: &str = "[Full output spilled to";

/// Characters reserved out of [`TOOL_OUTPUT_MAX_CHARS`] for the tail hint, so
/// the capped result always stays at or under the budget. Covers outputs up
/// to ~10^100 characters, far beyond any real tool result.
const TOOL_OUTPUT_TAIL_BUDGET: usize = 128;

/// Cap a tool result before it is injected into the conversation.
///
/// Many tools truncate internally (bash, grep, web_fetch), but external
/// sources — most notably MCP tool calls whose `structured_content` or
/// resource reads are rendered verbatim — can return unbounded output that
/// would silently swallow the token budget. This is the centralized guard at
/// the conversation-injection boundary: output over [`TOOL_OUTPUT_MAX_CHARS`]
/// is cut at a UTF-8 boundary, keeping the head (where errors and key results
/// usually are) and appending a tail hint. Truncation always shrinks the
/// result, so even a just-over-limit output is strictly reduced.
pub fn truncate_tool_output(output: &str) -> String {
    let total_chars = output.chars().count();
    if total_chars <= TOOL_OUTPUT_MAX_CHARS {
        return output.to_string();
    }
    let head_budget = TOOL_OUTPUT_MAX_CHARS - TOOL_OUTPUT_TAIL_BUDGET;
    let head: String = output.chars().take(head_budget).collect();
    format!(
        "{head}\n\n{TOOL_OUTPUT_TRUNCATED_HINT} (showing first {head_budget} of {total_chars} chars)"
    )
}

/// Like [`truncate_tool_output`], but spills the full output to a private
/// temp file instead of dropping the tail — the model can read the rest back
/// via read_file (an absolute path resolves as-is) when the key results are
/// NOT only in the head (large greps, file dumps). Falls back to plain
/// truncation when the write fails, so the guard never becomes a failure
/// point. Use this ONLY for tool results; transient system guidance keeps
/// [`truncate_tool_output`] because the model never needs to read that back.
pub fn spill_tool_output(output: &str) -> String {
    let total_chars = output.chars().count();
    if total_chars <= TOOL_OUTPUT_MAX_CHARS {
        return output.to_string();
    }
    if let Ok(path) = spill_output_to_file(output) {
        let head_budget = TOOL_OUTPUT_MAX_CHARS - TOOL_OUTPUT_TAIL_BUDGET;
        let head: String = output.chars().take(head_budget).collect();
        return format!(
            "{head}\n\n{TOOL_OUTPUT_SPILLED_HINT} {path} — {total_chars} chars total. \
             Use read_file on that path to read the rest.]"
        );
    }
    truncate_tool_output(output)
}

/// Write a full tool output to a private per-process spill directory under
/// the OS temp dir and return the absolute path. Files are named with a
/// fresh id so concurrent spills never collide.
fn spill_output_to_file(content: &str) -> std::io::Result<String> {
    let dir = std::env::temp_dir().join("deepdepcat-spill");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("tool-output-{}.txt", crate::core::ids::generate_id()));
    std::fs::write(&path, content)?;
    Ok(path.display().to_string())
}

/// Cap a tool result before it is injected into the conversation with a
/// caller-supplied budget (the dispatcher's configurable cap, vs. the fixed
/// [`TOOL_OUTPUT_MAX_CHARS`] of [`truncate_tool_output`]).
///
/// The budget is counted in CHARACTERS (`chars().count()`), never bytes —
/// multi-byte text was previously measured in bytes while the hint claimed
/// "chars". The head is preserved (errors and key results live there) and a
/// tail hint reports the true totals.
pub fn truncate_content(content: &str, max_chars: usize, tool_name: &str) -> String {
    let total_chars = content.chars().count();
    if total_chars <= max_chars {
        return content.to_string();
    }
    tracing::warn!(
        tool = tool_name,
        len = total_chars,
        max = max_chars,
        "Tool output truncated"
    );
    let head: String = content.chars().take(max_chars).collect();
    format!("{head}\n\n...(output truncated, {max_chars} of {total_chars} chars shown)")
}

/// Strip tool-call protocol markup embedded in an assistant message's raw
/// text. Providers can render a turn's tool calls as `<tool_calls>` /
/// `<tool_call>` blocks INSIDE the text content (e.g. XML tool-calling
/// mode); that markup is internal protocol — it must never surface in
/// user-visible text or be re-sent to the model as prose. Handles complete
/// blocks, orphan openers/closers, and blocks cut off mid-stream (an
/// unclosed opener drops the rest — a truncated block carries no
/// recoverable prose, only protocol garbage like `<bash>` leaves).
pub fn strip_tool_call_markup(text: &str) -> String {
    // Harness frames (system-reminder / app-guidance / task-notification /
    // evaluator & goal review blocks) occasionally get echoed into a model's
    // visible reply. They are instructions for the model, never chat
    // content — strip them here so neither storage nor the UI shows them
    // (real session 2d02f3dc: the user saw a raw skill-injection
    // <system-reminder> block in the stream).
    let text = strip_internal_frames(text);
    // DeepSeek V3.2/V4 emits its native DSML tool-call markup (`<｜DSML｜...>`)
    // inside text; strip it first so neither the block nor its `tool_call`
    // substring survives into user-visible prose.
    let text = super::dsml::strip_markup(&text);
    let bytes = text.as_bytes();
    let has_markup = bytes
        .windows(b"tool_call".len())
        .any(|w| w.eq_ignore_ascii_case(b"tool_call"));
    if !has_markup {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(pos) = find_tool_call_tag(rest) {
        out.push_str(&rest[..pos]);
        out.push(' ');
        let after = &rest[pos..];
        let is_closer = after
            .as_bytes()
            .get(..9)
            .is_some_and(|b| b.eq_ignore_ascii_case(b"</tool_ca"));
        let end = if is_closer {
            // Orphan closer — skip through its '>'.
            after.find('>').map_or(after.len(), |p| p + 1)
        } else {
            // Opener — skip through the next closer (case-insensitive); a
            // cut-off block has none and is dropped to the end.
            match find_closer_after(after) {
                Some(p) => p + 1,
                None => after.len(),
            }
        };
        rest = &after[end..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove harness frame blocks that a model echoes into its visible reply.
///
/// Known internal frames (opener, closer). An opener without a closer drops
/// the rest of the text — leaking a half-frame is worse than cutting prose.
fn strip_internal_frames(text: &str) -> String {
    const FRAMES: &[(&str, &str)] = &[
        ("<system-reminder", "</system-reminder>"),
        ("<app-guidance", "</app-guidance>"),
        ("<task-notification", "</task-notification>"),
        ("<evaluator-review", "</evaluator-review>"),
        ("<goal-review", "</goal-review>"),
        ("<coordinator_phase", "</coordinator_phase>"),
        ("<current-goal", "</current-goal>"),
        ("<environment-context", "</environment-context>"),
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let mut earliest: Option<(usize, &'static str, &'static str)> = None;
        for (opener, closer) in FRAMES {
            if let Some(pos) = rest.to_ascii_lowercase().find(opener) {
                let better = earliest.as_ref().map(|(p, _, _)| pos < *p).unwrap_or(true);
                if better {
                    earliest = Some((pos, opener, closer));
                }
            }
        }
        let Some((pos, _, closer)) = earliest else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        match after.to_ascii_lowercase().find(closer) {
            Some(end) => {
                rest = &after[end + closer.len()..];
            }
            None => {
                // Unclosed frame — drop everything from the opener.
                break;
            }
        }
    }
    out
}

/// Find a literal `needle` in `text`, tolerating CRLF line endings.
///
/// Exact matching fails on files whose line endings differ from what the
/// model produced ("Text not found" while the text is visibly there —
/// session 2d02f3dc). This helper treats each `\n` in the needle as matching
/// either `\n` or `\r\n` in the text. Returns the byte range of the match
/// in the ORIGINAL text. Fast path: plain exact `str::find`.
pub fn find_literal_with_crlf(text: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    if let Some(pos) = text.find(needle) {
        return Some((pos, pos + needle.len()));
    }
    if !needle.contains('\n') {
        return None;
    }
    let text_bytes = text.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut start = 0usize;
    while start < text_bytes.len() {
        let mut ni = 0usize;
        let mut ti = start;
        let mut matched = true;
        while ni < needle_bytes.len() {
            let nb = needle_bytes[ni];
            if nb == b'\r' && needle_bytes.get(ni + 1) == Some(&b'\n') {
                // Needle CRLF: match text CRLF or lone LF.
                if ti + 1 < text_bytes.len()
                    && text_bytes[ti] == b'\r'
                    && text_bytes[ti + 1] == b'\n'
                {
                    ni += 2;
                    ti += 2;
                } else if ti < text_bytes.len() && text_bytes[ti] == b'\n' {
                    ni += 2;
                    ti += 1;
                } else {
                    matched = false;
                    break;
                }
            } else if nb == b'\n' {
                // Needle LF: match text CRLF or lone LF.
                if ti + 1 < text_bytes.len()
                    && text_bytes[ti] == b'\r'
                    && text_bytes[ti + 1] == b'\n'
                {
                    ni += 1;
                    ti += 2;
                } else if ti < text_bytes.len() && text_bytes[ti] == b'\n' {
                    ni += 1;
                    ti += 1;
                } else {
                    matched = false;
                    break;
                }
            } else if ti < text_bytes.len() && text_bytes[ti] == nb {
                ni += 1;
                ti += 1;
            } else {
                matched = false;
                break;
            }
        }
        if matched {
            return Some((start, ti));
        }
        start += 1;
    }
    None
}

/// Stateful guard that hides provider tool-call protocol blocks from the
/// LIVE text stream. Storage is sanitized separately (strip_tool_call_markup);
/// this guard prevents the raw `<tool_calls> ... </tool_calls>` / DSML
/// (`<｜DSML｜...>`) markup from flashing in the UI while the model is still
/// generating (real session 92a42f15: the model streamed its structured tool
/// calls as text alongside the real tool_calls).
///
/// Semantics mirror strip_tool_call_markup: from the first recognized
/// opener (`<tool_calls` / `<tool_call` / `<invoke` / any DSML tag) the text
/// is hidden until the matching closer brings the tag depth back to zero.
/// Nested protocol tags are balanced; unrelated XML/HTML inside parameter
/// values (e.g. `<section>`) is ignored. Tags split across stream deltas are
/// picked up once complete; an incomplete trailing tag leaves a tiny visible
/// prefix, which is a transient cosmetic artifact, not persisted data.
pub struct StreamMarkupGuard {
    depth: usize,
    scan_from: usize,
}

/// The slice(s) of one stream delta that are safe to emit live.
#[derive(Debug)]
pub struct VisibleDelta<'a> {
    /// Text before the first protocol block opener in this delta.
    pub before: &'a str,
    /// Text after the protocol block closed again in this delta.
    pub after: &'a str,
}

impl StreamMarkupGuard {
    pub fn new() -> Self {
        Self {
            depth: 0,
            scan_from: 0,
        }
    }

    /// Feed the full accumulated text (with `delta` already appended) and
    /// receive the visible portion(s) of `delta`.
    pub fn visible<'a>(&mut self, accumulated: &'a str, delta: &'a str) -> VisibleDelta<'a> {
        let delta_start = accumulated.len() - delta.len();
        let was_in_block = self.depth > 0;
        let mut i = self.scan_from.min(accumulated.len());
        let mut block_start: Option<usize> = None; // first opener in this delta
        let mut resume_at: Option<usize> = None; // last close that emptied depth

        while i < accumulated.len() {
            let (rel, tag_end_rel, kind) = match find_protocol_tag(&accumulated[i..]) {
                Some(found) => found,
                None => break,
            };
            let pos = i + rel;
            let end = i + tag_end_rel;
            match kind {
                TagKind::Incomplete => {
                    // `<` seen without a complete tag yet — keep scanning
                    // from here on the next delta so a split tag is caught.
                    self.scan_from = pos;
                    break;
                }
                TagKind::Open => {
                    if self.depth == 0 && block_start.is_none() {
                        block_start = Some(pos);
                    }
                    self.depth += 1;
                    self.scan_from = end;
                }
                TagKind::Close => {
                    if self.depth == 1 {
                        resume_at = Some(end);
                    }
                    self.depth = self.depth.saturating_sub(1);
                    self.scan_from = end;
                }
                TagKind::SelfClose | TagKind::Other => {
                    self.scan_from = end;
                }
            }
            i = end;
        }
        if self.scan_from >= i {
            self.scan_from = i;
        }

        let before = match block_start {
            Some(pos) => &delta[..(pos.saturating_sub(delta_start)).min(delta.len())],
            None if was_in_block => "",
            None => delta,
        };
        let after = match (block_start, resume_at) {
            (Some(start), Some(end)) if end > start => {
                &delta[(end.saturating_sub(delta_start)).min(delta.len())..]
            }
            (None, Some(end)) if was_in_block => {
                &delta[(end.saturating_sub(delta_start)).min(delta.len())..]
            }
            _ => "",
        };
        VisibleDelta { before, after }
    }
}

impl Default for StreamMarkupGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Open,
    Close,
    SelfClose,
    Other,
    Incomplete,
}

/// Find the next protocol-relevant XML tag in `text`.
///
/// Returns `(start, end, kind)` relative to `text`. `end` is just past the
/// tag's `>`. Non-protocol tags (any HTML/XML not in the tool-call family)
/// are returned as `Other` so scanning can skip past them quickly.
fn find_protocol_tag(text: &str) -> Option<(usize, usize, TagKind)> {
    let start = text.find('<')?;
    let after = &text[start..];
    let is_close = after.starts_with("</");
    let Some(tag_end_rel) = after.find('>') else {
        return Some((start, after.len(), TagKind::Incomplete));
    };
    let end = start + tag_end_rel + 1;
    let raw = &after[..=tag_end_rel];
    let self_close = raw.ends_with("/>");
    let inner = raw
        .trim_start_matches("</")
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    let name = inner
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("");
    let is_protocol = name.eq_ignore_ascii_case("tool_calls")
          || name.eq_ignore_ascii_case("tool_call")
          || name.eq_ignore_ascii_case("invoke")
        || name.starts_with("｜DSML｜")
        // Any DSML variant — single-bar, double-bar (`＜＜DSML＞＞`,
        // real deepseek-v4-flash sessions), ASCII pipes, Hangzhou.
        || name.contains("DSML");
    let kind = if !is_protocol {
        TagKind::Other
    } else if self_close {
        TagKind::SelfClose
    } else if is_close {
        TagKind::Close
    } else {
        TagKind::Open
    };
    Some((start, end, kind))
}

/// Find the byte index of the next `<tool_call`/`</tool_call` tag start
/// (case-insensitive), or `None`.
fn find_tool_call_tag(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (0..bytes.len()).find(|&i| {
        bytes[i] == b'<'
            && (ci_starts_with(bytes, b"<tool_call", i) || ci_starts_with(bytes, b"</tool_call", i))
    })
}

/// Find the byte index just past the next `</tool_call` closer
/// (case-insensitive) in `after`, or `None`.
fn find_closer_after(after: &str) -> Option<usize> {
    let bytes = after.as_bytes();
    (0..bytes.len())
        .find(|&i| bytes[i] == b'<' && ci_starts_with(bytes, b"</tool_call", i))
        .map(|p| {
            let close_end = p + b"</tool_call".len();
            after[close_end..]
                .find('>')
                .map(|q| close_end + q + 1)
                .unwrap_or(after.len())
        })
}

fn ci_starts_with(haystack: &[u8], needle: &[u8], offset: usize) -> bool {
    haystack.len() >= offset + needle.len()
        && haystack[offset..offset + needle.len()].eq_ignore_ascii_case(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_truncation() {
        assert_eq!(truncate_at_char_boundary("hello world", 5), "hello");
    }

    #[test]
    fn multibyte_truncation_at_boundary() {
        // '中' is 3 bytes, '文' is 3 bytes
        assert_eq!(truncate_at_char_boundary("中文", 3), "中");
    }

    #[test]
    fn multibyte_truncation_mid_char() {
        // '中' is 3 bytes. Truncating at 4 bytes would split the second char.
        // Should back off to 3.
        assert_eq!(truncate_at_char_boundary("中文", 4), "中");
    }

    #[test]
    fn multibyte_truncation_at_1_byte() {
        // 1 byte is in the middle of '中' (3 bytes), so back off to 0.
        assert_eq!(truncate_at_char_boundary("中文", 1), "");
    }

    #[test]
    fn no_truncation_needed() {
        assert_eq!(truncate_at_char_boundary("hello", 100), "hello");
    }

    #[test]
    fn empty_string() {
        assert_eq!(truncate_at_char_boundary("", 10), "");
    }

    #[test]
    fn emoji_truncation() {
        // '🎉' is 4 bytes
        assert_eq!(truncate_at_char_boundary("a🎉b", 2), "a");
        assert_eq!(truncate_at_char_boundary("a🎉b", 5), "a🎉");
    }

    #[test]
    fn internal_frames_are_stripped_from_model_output() {
        // A model echoing a skill-injection reminder must not reach storage
        // or the UI (session 2d02f3dc leak).
        let input = "这个不需要你区弄 <system-reminder> Active project skills apply to this work: \
                     ## Skill: Code Review ... </system-reminder> 回到你的问题。";
        let out = strip_internal_frames(input);
        assert!(!out.contains("system-reminder"), "frame removed: {out}");
        assert!(out.contains("这个不需要你区弄"), "leading prose kept");
        assert!(out.contains("回到你的问题"), "trailing prose kept");
    }

    #[test]
    fn unclosed_internal_frame_drops_the_rest() {
        let input = "前面正常 <system-reminder> 没有闭合的注入块";
        let out = strip_internal_frames(input);
        assert!(
            out.starts_with("前面正常"),
            "prose before frame kept: {out}"
        );
        assert!(!out.contains("没有闭合"), "unclosed frame tail dropped");
    }

    #[test]
    fn plain_text_and_multiline_pass_through_frames() {
        let input = "第一行\n第二行 <tool_calls>x</tool_calls> 第三行";
        let out = strip_internal_frames(input);
        assert_eq!(out, input, "no internal frames → untouched");
        let with_app = "<app-guidance>这是内部提示</app-guidance> 内容";
        assert!(
            strip_internal_frames(with_app).ends_with("内容"),
            "frame removed, trailing prose kept"
        );
    }

    #[test]
    fn crlf_tolerant_find_matches_across_line_endings() {
        // LF needle against CRLF file (Windows checkout) — the session
        // 2d02f3dc "Text not found" failure mode.
        let crlf_text = "line one\r\nline two\r\nline three";
        let needle = "line one\nline two";
        let (start, end) = find_literal_with_crlf(crlf_text, needle).expect("LF matches CRLF");
        assert_eq!(&crlf_text[start..end], "line one\r\nline two");
        // CRLF needle against LF file.
        let lf_text = "line one\nline two";
        let (start, end) =
            find_literal_with_crlf(lf_text, "line one\r\nline two").expect("CRLF matches LF");
        assert_eq!(&lf_text[start..end], "line one\nline two");
        // Exact match still wins on the fast path.
        let (start, end) = find_literal_with_crlf("abc def", "def").expect("exact");
        assert_eq!(&"abc def"[start..end], "def");
        // No match stays None.
        assert!(find_literal_with_crlf("abc", "xyz").is_none());
        assert!(find_literal_with_crlf("", "x").is_none());
        assert!(find_literal_with_crlf("abc", "").is_none());
    }

    #[test]
    fn tool_output_under_limit_passes_through() {
        assert_eq!(truncate_tool_output("short"), "short");
        assert_eq!(truncate_tool_output(""), "");
        let exact = "x".repeat(TOOL_OUTPUT_MAX_CHARS);
        assert_eq!(truncate_tool_output(&exact), exact);
    }

    #[test]
    fn tool_output_over_limit_is_truncated() {
        let big = "a".repeat(TOOL_OUTPUT_MAX_CHARS + 1);
        let out = truncate_tool_output(&big);
        assert!(out.len() < big.len(), "truncated output must be smaller");
        assert!(out.contains(TOOL_OUTPUT_TRUNCATED_HINT), "{out}");
        // Head preserved, tail hint present.
        assert!(out.starts_with("a"));
    }

    #[test]
    fn tool_output_truncation_preserves_head_and_reports_total() {
        let head = "HEAD_ERROR: boom\n";
        let filler = "f".repeat(TOOL_OUTPUT_MAX_CHARS + 50);
        let input = format!("{head}{filler}");
        let out = truncate_tool_output(&input);
        assert!(out.starts_with(head), "head must be preserved");
        let total = input.chars().count();
        let head_budget = TOOL_OUTPUT_MAX_CHARS - TOOL_OUTPUT_TAIL_BUDGET;
        assert!(out.contains(&format!("showing first {head_budget} of {total} chars")));
    }

    #[test]
    fn tool_output_truncation_never_splits_multibyte() {
        // 3-byte chars at the boundary: truncation must not produce invalid UTF-8.
        let big = "中".repeat(TOOL_OUTPUT_MAX_CHARS + 1);
        let out = truncate_tool_output(&big);
        assert!(out.contains(TOOL_OUTPUT_TRUNCATED_HINT));
        // The preserved head (before the "\n\n" separator) must be all full chars.
        let before_hint = out.split(TOOL_OUTPUT_TRUNCATED_HINT).next().unwrap();
        let head = before_hint
            .strip_suffix("\n\n")
            .expect("head has separator");
        let byte_len = head.len();
        assert_eq!(byte_len % 3, 0, "must not split multibyte chars");
        assert_eq!(head.chars().count() * 3, byte_len);
    }

    #[test]
    fn spill_tool_output_writes_full_output_to_file() {
        let big = "a".repeat(TOOL_OUTPUT_MAX_CHARS + 1);
        let out = spill_tool_output(&big);
        assert!(out.contains(TOOL_OUTPUT_SPILLED_HINT), "{out}");
        assert!(out.starts_with("a"));
        // Bounded: head + hint stays well under the full output — the +1
        // case keeps the head nearly full, so the bound is generous.
        assert!(
            out.len() < TOOL_OUTPUT_MAX_CHARS + 4096,
            "spilled result must stay bounded: {}",
            out.len()
        );
    }

    #[test]
    fn spill_tool_output_preserves_head() {
        let head = "HEAD_ERROR: boom\n";
        let filler = "f".repeat(TOOL_OUTPUT_MAX_CHARS + 50);
        let input = format!("{head}{filler}");
        let out = spill_tool_output(&input);
        assert!(out.starts_with(head), "head must be preserved");
        assert!(out.contains(TOOL_OUTPUT_SPILLED_HINT));
    }

    #[test]
    fn truncate_content_counts_chars_not_bytes() {
        // Multi-byte text: the budget must be measured in characters, and
        // the hint must report the true char totals (not byte lengths).
        let content = "中".repeat(100);
        let out = truncate_content(&content, 10, "read_file");
        assert!(out.contains("10 of 100 chars shown"));
        assert_eq!(out.lines().next().unwrap().chars().count(), 10);
        assert!(truncate_content("short", 100, "read_file").ends_with("short"));
    }

    #[test]
    fn tool_call_markup_passes_through_when_absent() {
        assert_eq!(strip_tool_call_markup("plain prose"), "plain prose");
        assert_eq!(strip_tool_call_markup(""), "");
        assert_eq!(strip_tool_call_markup("a < b and c > d"), "a < b and c > d");
    }

    #[test]
    fn tool_call_markup_full_block_removed() {
        let input = "need to check FAQ first. <tool_calls> <tool_call> <name>bash</name> </tool_call> </tool_calls> done";
        assert_eq!(
            strip_tool_call_markup(input),
            "need to check FAQ first. done"
        );
    }

    #[test]
    fn tool_call_markup_cut_off_block_drops_rest() {
        // The stream was truncated inside the block — the remainder is
        // protocol garbage (`<bash>` leaves) with no recoverable prose.
        let input = "need to check FAQ first. <tool_calls> <bash>grep -o";
        assert_eq!(strip_tool_call_markup(input), "need to check FAQ first.");
    }

    #[test]
    fn tool_call_markup_orphan_closer_removed() {
        assert_eq!(strip_tool_call_markup("done </tool_calls>"), "done");
        assert_eq!(strip_tool_call_markup("</tool_calls>"), "");
    }

    #[test]
    fn tool_call_markup_case_insensitive() {
        let input = "left <TOOL_CALLS> <bash>x</bash> </TOOL_CALLS> right";
        assert_eq!(strip_tool_call_markup(input), "left right");
    }

    #[test]
    fn tool_call_markup_keeps_middle_text() {
        let input = "first <tool_calls>x</tool_calls> second <tool_call>y</tool_call> third";
        assert_eq!(strip_tool_call_markup(input), "first second third");
    }

    #[test]
    fn tool_call_markup_whitespace_collapsed() {
        let input = "a  \n  <tool_calls>\n\nz\n</tool_calls>   b";
        assert_eq!(strip_tool_call_markup(input), "a b");
    }

    #[test]
    fn strip_cleans_plain_xml_tool_draft_like_reasoning() {
        // DeepSeek thinking mode often drafts the real tool-call XML inside
        // reasoning_content before emitting structured calls. The stored
        // reasoning must be cleaned exactly like visible text.
        let input = concat!(
            "思考：改样式 <tool_calls><invoke name=\"edit_file\">",
            "<parameter name=\"path\" string=\"true\">css/style.css</parameter>",
            "<parameter name=\"new_text\" string=\"true\">a</parameter>",
            "</invoke></tool_calls> 然后检查"
        );
        assert_eq!(strip_tool_call_markup(input), "思考：改样式 然后检查");
    }

    #[test]
    fn strip_cleans_bare_invoke_block_without_root() {
        let input = concat!(
            "草稿 <invoke name=\"edit_file\">",
            "<parameter name=\"path\" string=\"true\">css/style.css</parameter>",
            "</invoke> 收尾"
        );
        assert_eq!(strip_tool_call_markup(input), "草稿 收尾");
    }

    #[test]
    fn stream_guard_hides_block_across_deltas() {
        let mut guard = StreamMarkupGuard::new();
        let d1 = "E2E 通过。 ";
        let acc1 = d1.to_string();
        let v1 = guard.visible(&acc1, d1);
        assert_eq!(v1.before, d1);
        assert_eq!(v1.after, "");

        let d2 = "<tool_calls> <invoke name=\"bash\">";
        let mut acc2 = acc1.clone();
        acc2.push_str(d2);
        let v2 = guard.visible(&acc2, d2);
        assert_eq!(v2.before, "");
        assert_eq!(v2.after, "");

        let d3 =
            "<parameter name=\"command\" string=\"true\">ls</parameter> </invoke> </tool_calls>";
        acc2.push_str(d3);
        let v3 = guard.visible(&acc2, d3);
        assert_eq!(v3.before, "");
        assert_eq!(v3.after, "");

        let d4 = " 完成";
        acc2.push_str(d4);
        let v4 = guard.visible(&acc2, d4);
        assert_eq!(v4.before, " 完成");
        assert_eq!(v4.after, "");
    }

    #[test]
    fn stream_guard_emits_prefix_before_opener_in_same_delta() {
        let mut guard = StreamMarkupGuard::new();
        let d = "先看一下 <tool_calls><invoke name=\"x\"/>";
        let acc = d.to_string();
        let v = guard.visible(&acc, d);
        assert_eq!(v.before, "先看一下 ");
        assert_eq!(v.after, "");
    }

    #[test]
    fn stream_guard_resumes_after_closer_in_same_delta() {
        let mut guard = StreamMarkupGuard::new();
        let d1 = "<tool_calls><bash>ls</bash></tool_calls>";
        let mut acc = d1.to_string();
        let v1 = guard.visible(&acc, d1);
        assert_eq!(v1.before, "");
        assert_eq!(v1.after, "");

        let d2 = "好";
        acc.push_str(d2);
        let v2 = guard.visible(&acc, d2);
        assert_eq!(v2.before, "好");
    }

    #[test]
    fn stream_guard_balances_nested_protocol_tags_and_ignores_html() {
        let mut guard = StreamMarkupGuard::new();
        let d = concat!(
            "<｜DSML｜tool_calls><｜DSML｜invoke name=\"edit_file\">",
            "<｜DSML｜parameter name=\"path\" string=\"true\">",
            "<section>a</section>",
            "</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜tool_calls>"
        );
        let acc = d.to_string();
        let v = guard.visible(&acc, d);
        assert_eq!(v.before, "");
        assert_eq!(v.after, "");

        let d2 = "后文";
        let mut acc2 = acc;
        acc2.push_str(d2);
        let v2 = guard.visible(&acc2, d2);
        assert_eq!(v2.before, "后文");
    }

    #[test]
    fn stream_guard_handles_self_closing_invoke() {
        let mut guard = StreamMarkupGuard::new();
        let d = "<tool_calls><invoke name=\"list_dir\"/></tool_calls>ok";
        let acc = d.to_string();
        let v = guard.visible(&acc, d);
        assert_eq!(v.before, "");
        assert_eq!(v.after, "ok");
    }

    #[test]
    fn stream_guard_hides_double_bar_dsml_block() {
        // The double fullwidth-bar variant must not leak into the live
        // stream either (it previously flashed as raw markup in the chat).
        let mut guard = StreamMarkupGuard::new();
        let d = concat!(
            "<＜＜DSML＞＞tool_calls>",
            "<＜＜DSML＞＞invoke name=\"read_file\">",
            "<＜＜DSML＞＞parameter name=\"path\" string=\"true\">a</＜＜DSML＞＞parameter>",
            "</＜＜DSML＞＞invoke>",
            "</＜＜DSML＞＞tool_calls>后文"
        );
        let acc = d.to_string();
        let v = guard.visible(&acc, d);
        assert_eq!(v.before, "");
        assert_eq!(v.after, "后文");
    }

    #[test]
    fn stream_guard_catches_tag_split_across_deltas() {
        let mut guard = StreamMarkupGuard::new();
        let d1 = "前 <tool_cal";
        let mut acc = d1.to_string();
        let v1 = guard.visible(&acc, d1);
        assert_eq!(v1.before, "前 <tool_cal"); // incomplete opener leaks prefix

        let d2 = "ls><bash>ls</bash></tool_calls>";
        acc.push_str(d2);
        let v2 = guard.visible(&acc, d2);
        assert_eq!(v2.before, "");
        assert_eq!(v2.after, "");
    }

    #[test]
    fn stream_guard_ignores_orphan_closer_and_plain_text() {
        let mut guard = StreamMarkupGuard::new();
        let d1 = "a </tool_calls> b";
        let acc1 = d1.to_string();
        let v1 = guard.visible(&acc1, d1);
        assert_eq!(v1.before, d1);

        let d2 = "普通正文 <b>粗体</b>";
        let mut acc2 = acc1.clone();
        acc2.push_str(d2);
        let v2 = guard.visible(&acc2, d2);
        assert_eq!(v2.before, d2);
    }
}
