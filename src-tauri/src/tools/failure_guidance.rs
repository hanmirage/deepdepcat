//! Failure guidance — "switch strategy" hints appended to failed tool
//! results so the model corrects itself instead of retrying blindly.
//!
//! The agent already sees tool errors in context, but gets no structured
//! nudge on *how* to recover. This stateless evaluator produces a short,
//! tool-specific recovery hint (wrapped in `<system-reminder>` by the caller)
//! for every `is_error` result. It never fires on success.

use serde_json::Value;

/// Stateless failure-to-guidance evaluator.
pub struct FailureGuidance;

impl FailureGuidance {
    /// Produce a recovery hint for a failed tool call, or `None`.
    ///
    /// `output` is the tool result text (the failed error message / exit
    /// details). The `is_error` flag is the authoritative failure signal —
    /// text matching only picks the *category* of guidance, never decides
    /// whether the call failed.
    pub fn evaluate(
        tool_name: &str,
        _args: &Value,
        output: &str,
        is_error: bool,
    ) -> Option<String> {
        if !is_error {
            return None;
        }

        // User-interaction / mode tools never need a recovery nudge.
        if matches!(tool_name, "ask_user" | "enter_plan_mode" | "exit_plan_mode") {
            return None;
        }

        match tool_name {
            "edit_file" | "search_replace" => Self::edit_guidance(output),
            "bash" => Self::bash_guidance(output),
            "web_fetch" | "web_search" => Some(
                "The network request failed. Verify the URL and connectivity, then retry \
                 or use a different source."
                    .to_string(),
            ),
            "grep" if output.contains("Invalid regex") => {
                Some("The regex pattern is invalid. Fix the pattern syntax and retry.".to_string())
            }
            _ => Some(
                "The tool failed. Re-read the relevant file(s) to confirm current state, then \
                 adjust the arguments or switch to a different approach."
                    .to_string(),
            ),
        }
    }

    /// Guidance for file-edit failures (text not found / ambiguous match).
    fn edit_guidance(output: &str) -> Option<String> {
        let lower = output.to_lowercase();
        if lower.contains("text not found") || lower.contains("not found") {
            Some(
                "The edit failed: the target text was not found. Re-read the file to get its \
                 current content, then retry with the exact current text (including whitespace)."
                    .to_string(),
            )
        } else if lower.contains("multiple locations") || lower.contains("occurrences") {
            Some(
                "The target text matched multiple locations. Add more surrounding context to \
                 make the match unique, or read the file and target a smaller unique snippet."
                    .to_string(),
            )
        } else {
            Some(
                "The edit failed. Re-read the file to confirm its current content, then adjust \
                 the edit arguments."
                    .to_string(),
            )
        }
    }

    /// Guidance for shell command failures (timeout vs non-zero exit).
    fn bash_guidance(output: &str) -> Option<String> {
        if output.contains("timed out") {
            Some(
                "The command timed out. Shorten it, reduce the workload, raise the timeout \
                 argument, or split it into smaller steps."
                    .to_string(),
            )
        } else {
            Some(
                "The command failed (non-zero exit). Inspect the STDERR above, correct the \
                 command (paths, syntax, quoting), and retry."
                    .to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_guidance_on_success() {
        assert_eq!(
            FailureGuidance::evaluate("bash", &Value::Null, "done", false),
            None
        );
        assert_eq!(
            FailureGuidance::evaluate("edit_file", &Value::Null, "ok", false),
            None
        );
    }

    #[test]
    fn edit_not_found_guidance() {
        let g = FailureGuidance::evaluate(
            "edit_file",
            &Value::Null,
            "Text not found in file: 'foo'",
            true,
        )
        .unwrap();
        assert!(g.contains("Re-read the file"));
    }

    #[test]
    fn edit_multiple_occurrences_guidance() {
        let g = FailureGuidance::evaluate(
            "edit_file",
            &Value::Null,
            "Found 3 occurrences of the text. Please provide more context",
            true,
        )
        .unwrap();
        assert!(g.contains("make the match unique"));
    }

    #[test]
    fn bash_timeout_guidance() {
        let g = FailureGuidance::evaluate(
            "bash",
            &Value::Null,
            "Command timed out after 120 seconds",
            true,
        )
        .unwrap();
        assert!(g.contains("timed out"));
        assert!(g.contains("split it into smaller steps"));
    }

    #[test]
    fn bash_nonzero_guidance() {
        let g =
            FailureGuidance::evaluate("bash", &Value::Null, "STDERR:\nboom\n\nExit code: 1", true)
                .unwrap();
        assert!(g.contains("non-zero exit"));
        assert!(g.contains("STDERR"));
    }

    #[test]
    fn web_fetch_guidance() {
        let g = FailureGuidance::evaluate(
            "web_fetch",
            &Value::Null,
            "Failed to fetch URL: timeout",
            true,
        )
        .unwrap();
        assert!(g.contains("Verify the URL"));
    }

    #[test]
    fn web_search_guidance() {
        let g =
            FailureGuidance::evaluate("web_search", &Value::Null, "Search request failed", true)
                .unwrap();
        assert!(g.contains("network request failed"));
    }

    #[test]
    fn grep_invalid_regex_guidance() {
        let g = FailureGuidance::evaluate(
            "grep",
            &Value::Null,
            "Invalid regex pattern: [unclosed",
            true,
        )
        .unwrap();
        assert!(g.contains("Fix the pattern syntax"));
    }

    #[test]
    fn write_file_falls_back_to_generic() {
        let g = FailureGuidance::evaluate(
            "write_file",
            &Value::Null,
            "Permission denied (os error 13)",
            true,
        )
        .unwrap();
        assert!(g.contains("Re-read the relevant file"));
    }

    #[test]
    fn user_interaction_tools_are_gated() {
        for name in ["ask_user", "enter_plan_mode", "exit_plan_mode"] {
            assert_eq!(
                FailureGuidance::evaluate(name, &Value::Null, "failed", true),
                None,
                "{name} should never get recovery guidance"
            );
        }
    }

    #[test]
    fn glob_empty_success_not_flagged() {
        // Empty glob results are success-state (not is_error), so no guidance.
        assert_eq!(
            FailureGuidance::evaluate("glob", &Value::Null, "No files found", false),
            None
        );
    }
}
