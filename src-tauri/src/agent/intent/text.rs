//! Shared text helpers for intent heuristics.

pub(crate) fn contains_file_path(text: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".py", ".js", ".json", ".toml", ".md", ".css", ".html",
    ];
    if EXTENSIONS.iter().any(|e| text.contains(e)) {
        return true;
    }
    text.contains("src/") || has_drive_path(text) || has_separator_path(text)
}

/// A Windows-style absolute path start: `C:\` or `C:/`.
pub(crate) fn has_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .windows(3)
        .any(|w| w[0].is_ascii_alphabetic() && w[1] == b':' && (w[2] == b'\\' || w[2] == b'/'))
}

/// A path-like token: a separator plus a dotted filename (`src\util.rs`,
/// `docs/guide.md`). A bare backslash ("a\b") is not a path.
pub(crate) fn has_separator_path(text: &str) -> bool {
    text.split_whitespace().any(|tok| {
        let sep = tok.contains('\\') || tok.contains('/');
        let ext = tok.rsplit_once('.');
        sep && matches!(
            ext,
            Some((_, e))
                if !e.is_empty() && e.len() <= 10 && !e.contains('\\') && !e.contains('/')
        )
    })
}

/// Draft a goal from the user message: first meaningful line, truncated.
pub(crate) fn contains_any(text: &str, needles: &[&str]) -> bool {
    let lower = text.to_lowercase();
    needles.iter().any(|n| lower.contains(n))
}

/// Whether `text` contains `needle` as a whole word (bounded by non-alphanumeric
/// characters on both sides), not as a substring of a longer word. `hi` must
/// not match inside "this"/"which", and `ty` must not match inside "type"/
/// "style". Case-insensitive.
pub(crate) fn contains_word(text: &str, needle: &str) -> bool {
    let lower = text.to_lowercase();
    let needle = needle.to_lowercase();
    let bytes = lower.as_bytes();
    let nlen = needle.len();
    for (start, _) in lower.char_indices() {
        if start + nlen > bytes.len() {
            break;
        }
        if !lower.is_char_boundary(start + nlen) {
            continue;
        }
        if lower[start..start + nlen] == needle {
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let after_ok = start + nlen == bytes.len() || !bytes[start + nlen].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

/// Whether a task description is large enough that decomposition is likely
/// to help (P3-7 automatic task decomposition heuristic).
///
/// Signals: at least 3 numbered sub-tasks ("1. … 2. … 3. …"), or a long
/// actionable message with multiple delimiter-separated clauses. Returns
/// the number of detected sub-task signals; 0 = keep as one task.
pub(crate) fn count_file_refs(text: &str) -> usize {
    // Longest extensions first so `.html` wins over `.h` at the same spot.
    const EXTS: &[&str] = &[
        "html", "tsx", "cpp", "json", "toml", "css", "vue", "java", "jsx", "rs", "ts", "py", "js",
        "md", "h", "c", "go",
    ];
    let bytes = text.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' && i > 0 {
            let prev = bytes[i - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'-' {
                for e in EXTS {
                    let end = i + 1 + e.len();
                    // `end` must be a char boundary too: matching ".14 " prose
                    // can otherwise slice into a multi-byte character.
                    if end <= bytes.len()
                        && text.is_char_boundary(end)
                        && text[i + 1..end].eq_ignore_ascii_case(e)
                    {
                        count += 1;
                        break;
                    }
                }
            }
        }
        i += 1;
    }
    if text.to_lowercase().contains("readme") {
        count += 1;
    }
    count
}

/// Whether the message asks to CHANGE existing material (vs. create or
/// investigate something new).
pub(crate) fn is_edit_request(text: &str) -> bool {
    const VERBS: &[&str] = &[
        // zh — change verbs (create-new verbs are deliberately absent:
        // "写一个" is inventing, not editing).
        "修改",
        "改一下",
        "改下",
        "调整",
        "更新",
        "替换",
        "美化",
        "优化",
        "改成",
        "换成",
        "修复",
        "修一下",
        "加个",
        // en
        "change",
        "update",
        "edit",
        "fix",
        "replace",
        "modify",
        "restyle",
        "make it",
        "look like",
        "add a",
        "add an",
    ];
    let lower = text.to_lowercase();
    VERBS.iter().any(|v| lower.contains(v))
}
