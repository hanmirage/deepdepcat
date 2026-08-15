//! Diff preview helper — inline per-file diff appended to editing-tool
//! success results so the agent can see the exact change it made.
//!
//! The diff is computed from the file content the tool already holds before
//! writing (no filesystem round-trip, no dependency on the rewind tracker's
//! turn lifecycle). Output is a compact, line-anchored unified diff that
//! fits inside the tool-result budget.
//!
//! No external diff dependency: a deterministic greedy line diff (LCS-based)
//! is plenty for the "what did my edit change" feedback loop, and it keeps
//! builds offline-safe on this machine.

/// How many of the leading changed lines may appear before the diff is
/// truncated. Keeps tool output bounded for large files while preserving
/// the first affected lines the model most needs.
const DIFF_CONTEXT_LINES: usize = 6;

/// Cap on the LCS table size (`before_lines × after_lines`). Above this the
/// full quadratic table would allocate hundreds of MB (a one-line edit to a
/// 40k-line file → ~12 GB) for a diff that is rendered down to 6 lines anyway;
/// fall back to a linear prefix/suffix diff instead.
const MAX_DIFF_CELLS: usize = 4_000_000;

/// A single changed line within a diff hunk.
struct DiffLine {
    /// '+' for added, '-' for removed, ' ' for context.
    kind: char,
    /// 1-based line number in the AFTER content ('-' lines carry the
    /// number of the removed line; the after numbering is the anchor).
    number: usize,
    text: String,
}

/// Compute a compact diff between `before` and `after`.
///
/// Returns the changes as lines of a unified-diff-style block. Lines outside
/// the changed regions are omitted entirely, so a whole-file rewrite reads
/// as a bounded list of changes rather than a huge scrollback. Returns
/// `None` when the two contents are identical.
pub fn compute_diff(before: &str, after: &str) -> Option<String> {
    let diff_lines = diff_lines(before, after)?;
    if diff_lines.is_empty() {
        return None;
    }

    let total = diff_lines.len();
    let mut rendered = String::new();
    for line in diff_lines.into_iter().take(DIFF_CONTEXT_LINES) {
        rendered.push_str(&format!(" {} {} {}\n", line.number, line.kind, line.text));
    }

    let mut result = format!("--- {} line(s) changed:\n", total);
    result.push_str(&rendered);
    if total > DIFF_CONTEXT_LINES {
        result.push_str(&format!(
            "… ({} more line(s) changed — shown up to {})\n",
            total - DIFF_CONTEXT_LINES,
            DIFF_CONTEXT_LINES
        ));
    }
    Some(result)
}

/// Compute the changed lines between two file contents (LCS-based greedy).
///
/// Aligns the two line sequences by longest-common-subsequence and emits the
/// removals and insertions that fall between matched lines, in order.
/// Removed lines are numbered by their position in the AFTER file (the
/// anchor) so the model can locate each change in the file as it now is.
fn diff_lines(before: &str, after: &str) -> Option<Vec<DiffLine>> {
    let before_lines = split_lines(before);
    let after_lines = split_lines(after);

    // Directly equal lines short-circuit the LCS table.
    if before_lines == after_lines {
        return None;
    }
    if before_lines.is_empty() {
        return Some(
            after_lines
                .iter()
                .enumerate()
                .map(|(i, text)| DiffLine {
                    kind: '+',
                    number: i + 1,
                    text: text.clone(),
                })
                .collect(),
        );
    }
    if after_lines.is_empty() {
        return Some(
            before_lines
                .iter()
                .enumerate()
                .map(|(i, _)| DiffLine {
                    kind: '-',
                    number: i + 1,
                    text: String::new(),
                })
                .collect(),
        );
    }

    // Size guard: the LCS table is O(n·m) memory/time. A small edit to a huge
    // generated/minified file would allocate the full quadratic table (up to
    // ~12 GB for 40k×40k) for a diff that is later rendered down to 6 lines.
    // Fall back to a linear prefix/suffix diff that still shows the changed
    // region without the quadratic allocation.
    let n = before_lines.len();
    let m = after_lines.len();
    if n.saturating_mul(m) > MAX_DIFF_CELLS {
        return diff_lines_bounded(&before_lines, &after_lines);
    }

    // LCS table: dp[i][j] = LCS length of before_lines[..i] and
    // after_lines[..j]. Simple but fine for typical file sizes; large
    // outputs are capped downstream by the render limit anyway.
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if before_lines[i] == after_lines[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < n && j < m {
        if before_lines[i] == after_lines[j] {
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(DiffLine {
                kind: '-',
                number: j + 1,
                text: before_lines[i].clone(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                kind: '+',
                number: j + 1,
                text: after_lines[j].clone(),
            });
            j += 1;
        }
    }
    while i < n {
        out.push(DiffLine {
            kind: '-',
            number: j + 1,
            text: before_lines[i].clone(),
        });
        i += 1;
    }
    while j < m {
        out.push(DiffLine {
            kind: '+',
            number: j + 1,
            text: after_lines[j].clone(),
        });
        j += 1;
    }
    Some(out)
}

/// Split content into lines, preserving trailing empty line (so an appended
/// trailing newline counts as a change when the file gains/loses one).
fn split_lines(content: &str) -> Vec<String> {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    if content.ends_with('\n') {
        lines.push(String::new());
    }
    lines
}

/// Linear prefix/suffix diff — the fallback for files too large for the LCS
/// table. Finds the common leading and trailing lines, then reports the whole
/// changed middle as removals followed by additions. Not a minimal edit
/// script, but O(n+m) and correct for the common "small edit in a huge file"
/// case (which is exactly when this path runs).
fn diff_lines_bounded(before: &[String], after: &[String]) -> Option<Vec<DiffLine>> {
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before.len().saturating_sub(prefix)
        && suffix < after.len().saturating_sub(prefix)
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut out = Vec::new();
    for line in &before[prefix..before.len() - suffix] {
        out.push(DiffLine {
            kind: '-',
            number: prefix + 1,
            text: line.clone(),
        });
    }
    for (number, line) in (prefix + 1..).zip(after[prefix..after.len() - suffix].iter()) {
        out.push(DiffLine {
            kind: '+',
            number,
            text: line.clone(),
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_returns_none() {
        assert!(compute_diff("a\nb\nc", "a\nb\nc").is_none());
    }

    #[test]
    fn reports_changed_line_count() {
        let before = "line1\nline2\nline3";
        let after = "line1\nLINE2\nline3";
        let diff = compute_diff(before, after).unwrap();
        assert!(diff.starts_with("--- 2 line(s) changed:\n"));
        assert!(diff.contains(" 2 - line2"));
        assert!(diff.contains(" 2 + LINE2"));
    }

    #[test]
    fn insert_shift_corrects_line_numbers() {
        let before = "a\nb\nc";
        let after = "a\nNEW\nb\nc";
        let diff = compute_diff(before, after).unwrap();
        assert!(diff.contains(" 2 + NEW"), "diff:\n{diff}");
    }

    #[test]
    fn delete_shifts_down() {
        let before = "a\nb\nc\nd";
        let after = "a\nd";
        let diff = compute_diff(before, after).unwrap();
        // 'a' matches, 'd' matches — the middle lines are removed.
        assert!(diff.contains(" - b"), "diff:\n{diff}");
        assert!(diff.contains(" - c"), "diff:\n{diff}");
        // d is unchanged, so no '+' for it.
        assert!(!diff.contains(" + d"), "diff:\n{diff}");
    }

    #[test]
    fn no_crlf_artifacts() {
        let before = "a\r\nb\r\n";
        let after = "a\r\nc\r\n";
        let diff = compute_diff(before, after).unwrap();
        assert!(!diff.contains('\r'));
    }

    #[test]
    fn large_diff_is_truncated() {
        let before: String = (1..=100).map(|i| format!("line{i}\n")).collect();
        let after: String = (1..=100).map(|i| format!("changed{i}\n")).collect();
        let diff = compute_diff(&before, &after).unwrap();
        assert!(diff.contains("more line(s) changed"));
        assert!(diff.len() < 1500);
    }

    #[test]
    fn whitespace_only_edit() {
        let diff = compute_diff("foo  ", "foo").unwrap();
        assert!(diff.starts_with("--- "));
    }

    #[test]
    fn bounded_diff_finds_prefix_suffix_change() {
        // The linear fallback (used for files too large for the LCS table)
        // must still report the changed middle and skip the unchanged
        // prefix/suffix.
        let before: Vec<String> =
            vec!["a".into(), "b".into(), "OLD".into(), "c".into(), "d".into()];
        let after: Vec<String> =
            vec!["a".into(), "b".into(), "NEW".into(), "c".into(), "d".into()];
        let diff = diff_lines_bounded(&before, &after).unwrap();
        assert!(diff.iter().any(|l| l.kind == '-' && l.text == "OLD"));
        assert!(diff.iter().any(|l| l.kind == '+' && l.text == "NEW"));
        assert!(!diff.iter().any(|l| l.text == "a" || l.text == "d"));
    }
}
