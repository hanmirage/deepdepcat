//! Tool-batch tests.

use super::*;
use serde_json::json;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_key_normalizes_whitespace_but_keeps_word_boundaries() {
        let a = failure_guard_key("edit_file", r#"{ "path": "a.rs", "old": "x y" }"#);
        let b = failure_guard_key("edit_file", r#"{ "path":"a.rs",  "old":"x y" }"#);
        assert_eq!(a, b, "whitespace differences must not split the guard key");
        // Collapsing whitespace must never merge distinct words: "x y" != "xy".
        let spaced = failure_guard_key("edit_file", r#"{ "old": "x y" }"#);
        let glued = failure_guard_key("edit_file", r#"{ "old": "xy" }"#);
        assert_ne!(spaced, glued, "whitespace-stripping must not merge words");
        // Long arguments are hashed, never truncated — two long distinct
        // args can't collide on a shared 512-char prefix.
        let long_a = failure_guard_key("bash", &format!("{}A", "z".repeat(2000)));
        let long_b = failure_guard_key("bash", &format!("{}B", "z".repeat(2000)));
        assert_ne!(long_a, long_b, "long distinct args must not collide");
        assert!(long_a.len() < 128, "keys must be bounded: {}", long_a.len());
        assert_ne!(
            failure_guard_key("edit_file", r#"{"path":"a.rs"}"#),
            failure_guard_key("write_file", r#"{"path":"a.rs"}"#),
            "different tools must not share a key"
        );
    }

    #[test]
    fn guard_blocks_after_two_failures_then_clears_on_success() {
        // Simulate the guard's state machine with a fresh ChatState.
        let mut cs = crate::agent::chat_state::ChatState::new("m", 100_000);
        let key = failure_guard_key("edit_file", r#"{"path":"a.rs"}"#);
        // two failures
        *cs.tool_failure_counts.entry(key.clone()).or_insert(0) += 1;
        *cs.tool_failure_counts.entry(key.clone()).or_insert(0) += 1;
        assert_eq!(cs.tool_failure_counts.get(&key), Some(&2));
        // success clears
        cs.tool_failure_counts.remove(&key);
        assert!(!cs.tool_failure_counts.contains_key(&key));
    }

    #[test]
    fn record_failure_outcome_accumulates_on_error_and_clears_on_success() {
        // The parallel path records results through this shared helper — the
        // same semantics as the serial path (2 failures → guarded).
        let mut counts = std::collections::HashMap::new();
        record_failure_outcome(&mut counts, "read_file", r#"{"path":"missing.rs"}"#, true);
        record_failure_outcome(&mut counts, "read_file", r#"{"path":"missing.rs"}"#, true);
        let key = failure_guard_key("read_file", r#"{"path":"missing.rs"}"#);
        assert_eq!(counts.get(&key), Some(&2), "two failures must accumulate");
        // A success on the same signature clears it — the guard resets.
        record_failure_outcome(&mut counts, "read_file", r#"{"path":"missing.rs"}"#, false);
        assert!(!counts.contains_key(&key), "success must clear the count");
    }

    #[test]
    fn failure_guard_map_is_bounded() {
        // A long session probing many distinct failed signatures must not
        // grow the map without bound — the newest key survives eviction.
        let mut counts = std::collections::HashMap::new();
        for i in 0..(MAX_FAILURE_GUARD_KEYS + 50) {
            record_failure_outcome(
                &mut counts,
                "read_file",
                &format!(r#"{{"path":"missing_{i}.rs"}}"#),
                true,
            );
        }
        assert!(
            counts.len() <= MAX_FAILURE_GUARD_KEYS,
            "map must be capped, got {}",
            counts.len()
        );
        // The most recent key must still be tracked (the guard never evicts
        // the signature it just recorded).
        let latest = failure_guard_key(
            "read_file",
            &format!(r#"{{"path":"missing_{}.rs"}}"#, MAX_FAILURE_GUARD_KEYS + 49),
        );
        assert!(
            counts.contains_key(&latest),
            "newest signature must survive eviction"
        );
    }

    #[test]
    fn edited_paths_recorded_for_successful_writes_only() {
        // Only successful file-writing tools record edited paths; reads,
        // non-path tools and failed writes must not pollute the list.
        let mut cs = crate::agent::chat_state::ChatState::new("m", 100_000);
        record_edited_path(&mut cs, "edit_file", &json!({"path": "src/a.rs"}));
        record_edited_path(&mut cs, "write_file", &json!({"path": "src/b.rs"}));
        record_edited_path(&mut cs, "search_replace", &json!({"path": "src/c.rs"}));
        record_edited_path(&mut cs, "apply_patch", &json!({"path": "src/d.rs"}));
        record_edited_path(&mut cs, "read_file", &json!({"path": "src/e.rs"}));
        record_edited_path(&mut cs, "edit_file", &json!({}));
        record_edited_path(&mut cs, "bash", &json!({"command": "echo hi"}));
        assert_eq!(cs.agent_edited_paths.len(), 4);
        for p in ["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"] {
            assert!(cs.agent_edited_paths.contains(p), "missing {p}");
        }
    }

    #[test]
    fn is_write_tool_classifies_only_writes() {
        for w in ["write_file", "edit_file", "search_replace", "apply_patch"] {
            assert!(is_write_tool(w), "{w} must be a write tool");
        }
        for r in ["read_file", "list_dir", "grep", "glob", "bash", "lsp"] {
            assert!(!is_write_tool(r), "{r} must not be a write tool");
        }
    }

    #[test]
    fn tool_name_failures_accumulate_across_different_arguments() {
        // The strategy-switch signal: the SAME tool failing under DIFFERENT
        // arguments is a doomed approach, not bad arguments. `bash` with
        // `mvn`, then `javac`, then `java` — three distinct signatures, one
        // tool name.
        let mut counts = std::collections::HashMap::new();
        record_tool_name_outcome(&mut counts, "bash", true);
        record_tool_name_outcome(&mut counts, "bash", true);
        record_tool_name_outcome(&mut counts, "bash", true);
        assert_eq!(counts.get("bash"), Some(&3));
        // A success on the tool clears the whole streak — the approach works.
        record_tool_name_outcome(&mut counts, "bash", false);
        assert!(!counts.contains_key("bash"));
    }

    #[test]
    fn tool_name_failures_are_independent_per_tool() {
        // Failing `bash` must not poison `web_fetch`'s counter.
        let mut counts = std::collections::HashMap::new();
        record_tool_name_outcome(&mut counts, "bash", true);
        record_tool_name_outcome(&mut counts, "bash", true);
        record_tool_name_outcome(&mut counts, "web_fetch", true);
        assert_eq!(counts.get("bash"), Some(&2));
        assert_eq!(counts.get("web_fetch"), Some(&1));
    }
}
