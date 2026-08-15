//! Hook JSON output protocol — Claude-compatible structured directives.
//!
//! A hook may return JSON on stdout (command / prompt / agent) or in the
//! HTTP response body to express more than plain allow/deny:
//!
//! ```json
//! {
//!   "hookSpecificOutput": {
//!     "hookEventName": "PreToolUse",
//!     "permissionDecision": "allow",
//!     "permissionDecisionReason": "safe",
//!     "updatedInput": { "command": "rm --dry-run *" },
//!     "additionalContext": "linter reports 3 warnings"
//!   }
//! }
//! ```
//!
//! A flat variant (`{"allow": true, "reason": "...", "updatedInput": ...,
//! "additionalContext": ...}`) is also accepted for HTTP-style hooks.
//! Event-scoped fields (`permissionDecision`, `updatedInput`) only apply to
//! PreToolUse; `additionalContext` applies to every event.

use super::types::HookEvent;
use serde_json::Value;

/// The permission verdict carried by a hook JSON directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPermissionDecision {
    Allow,
    Deny(String),
}

/// Structured directives parsed from a hook's JSON output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HookDirectives {
    pub permission_decision: Option<HookPermissionDecision>,
    pub updated_input: Option<Value>,
    pub additional_context: Option<String>,
}

/// Parse hook output into structured directives.
///
/// Returns `None` when the output is not directive JSON (plain text, or
/// JSON that names a different event). Empty directives produce a default
/// (all `None`) value — an empty JSON object is valid but inert.
pub fn parse_directives(output: &str, event: &HookEvent) -> Option<HookDirectives> {
    let trimmed = output.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let Ok(root) = serde_json::from_str::<Value>(trimmed) else {
        return None;
    };
    let root_obj = root.as_object()?;

    // Claude style nests the payload under hookSpecificOutput; the flat
    // form uses the root object directly.
    let (body, named_event) = match root_obj.get("hookSpecificOutput") {
        Some(Value::Object(specific)) => {
            let named = specific
                .get("hookEventName")
                .and_then(Value::as_str)
                .map(str::to_owned);
            (specific, named)
        }
        _ => (root_obj, None),
    };

    // When the payload names an event it must match the event the hook was
    // registered for — a mismatched payload is ignored (plain output).
    if let Some(ref name) = named_event {
        if !event_name_matches(name, event) {
            return None;
        }
    }

    let mut directives = HookDirectives::default();

    if event.is_pre_tool() {
        directives.permission_decision = parse_permission_decision(body);
        directives.updated_input = body
            .get("updatedInput")
            .filter(|v| v.is_object())
            .cloned();
    }

    directives.additional_context = body
        .get("additionalContext")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    Some(directives)
}

/// Extract the permission verdict from a directive body.
///
/// Supports Claude's `permissionDecision: "allow"|"deny"` plus the flat
/// `allow: bool` form used by HTTP-style hooks.
fn parse_permission_decision(body: &serde_json::Map<String, Value>) -> Option<HookPermissionDecision> {
    if let Some(decision) = body.get("permissionDecision").and_then(Value::as_str) {
        return match decision.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(HookPermissionDecision::Allow),
            "deny" => Some(HookPermissionDecision::Deny(
                body.get("permissionDecisionReason")
                    .and_then(Value::as_str)
                    .unwrap_or("Denied by hook")
                    .to_string(),
            )),
            _ => None,
        };
    }
    match body.get("allow").and_then(Value::as_bool) {
        Some(true) => Some(HookPermissionDecision::Allow),
        Some(false) => Some(HookPermissionDecision::Deny(
            body.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("Denied by hook")
                .to_string(),
        )),
        None => None,
    }
}

/// Whether a payload's declared event name matches the firing event.
///
/// Comparison is case-insensitive and tolerant of whitespace/separators;
/// `UserMessage` also accepts the historical aliases `UserPromptSubmit` and
/// `BeforeSubmitPrompt` (the discovery layer maps them all to UserMessage).
fn event_name_matches(name: &str, event: &HookEvent) -> bool {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let target = normalize(event.as_str());
    if normalize(name) == target {
        return true;
    }
    event == &HookEvent::UserMessage
        && matches!(
            normalize(name).as_str(),
            "userpromptsubmit" | "beforesubmitprompt"
        )
}

/// Apply parsed directives onto a hook result.
pub fn apply_directives(result: &mut super::types::HookResult, directives: HookDirectives) {
    match directives.permission_decision {
        Some(HookPermissionDecision::Allow) => {
            result.allow = true;
            result.deny_reason = None;
        }
        Some(HookPermissionDecision::Deny(reason)) => {
            result.allow = false;
            result.deny_reason = Some(reason);
        }
        None => {}
    }
    if let Some(input) = directives.updated_input {
        result.updated_input = Some(input);
    }
    if let Some(context) = directives.additional_context {
        result.additional_context = Some(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre_tool() -> HookEvent {
        HookEvent::PreToolUse
    }

    #[test]
    fn claude_style_updated_input_and_allow() {
        let out = r#"{
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": {"command": "rm --dry-run *"}
            }
        }"#;
        let d = parse_directives(out, &pre_tool()).expect("directives");
        assert_eq!(d.permission_decision, Some(HookPermissionDecision::Allow));
        assert_eq!(
            d.updated_input,
            Some(serde_json::json!({"command": "rm --dry-run *"}))
        );
        assert_eq!(d.additional_context, None);
    }

    #[test]
    fn claude_style_deny_with_reason() {
        let out = r#"{
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "rm -rf is prohibited"
            }
        }"#;
        let d = parse_directives(out, &pre_tool()).expect("directives");
        assert_eq!(
            d.permission_decision,
            Some(HookPermissionDecision::Deny(
                "rm -rf is prohibited".to_string()
            ))
        );
    }

    #[test]
    fn flat_form_allow_false_denies() {
        let out = r#"{"allow": false, "reason": "policy"}"#;
        let d = parse_directives(out, &pre_tool()).expect("directives");
        assert_eq!(
            d.permission_decision,
            Some(HookPermissionDecision::Deny("policy".to_string()))
        );
    }

    #[test]
    fn additional_context_applies_to_observe_events() {
        let out = r#"{
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": "linter: 3 warnings"
            }
        }"#;
        let d = parse_directives(out, &HookEvent::PostToolUse).expect("directives");
        assert_eq!(d.additional_context.as_deref(), Some("linter: 3 warnings"));
        // Permission fields are PreToolUse-only — ignored on observe events.
        assert_eq!(d.permission_decision, None);
        assert_eq!(d.updated_input, None);
    }

    #[test]
    fn mismatched_event_name_is_ignored() {
        let out = r#"{
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "permissionDecision": "deny"
            }
        }"#;
        assert!(parse_directives(out, &pre_tool()).is_none());
    }

    #[test]
    fn plain_text_is_not_directives() {
        assert!(parse_directives("just a line", &pre_tool()).is_none());
        assert!(parse_directives("", &pre_tool()).is_none());
    }

    #[test]
    fn user_message_aliases_match() {
        for name in ["UserPromptSubmit", "BeforeSubmitPrompt"] {
            assert!(
                event_name_matches(name, &HookEvent::UserMessage),
                "alias must match: {name}"
            );
        }
        assert!(!event_name_matches("Stop", &HookEvent::UserMessage));
    }

    #[test]
    fn empty_json_is_inert_but_valid() {
        let d = parse_directives("{}", &pre_tool()).expect("directives");
        assert_eq!(d, HookDirectives::default());
    }

    #[test]
    fn updated_input_must_be_object() {
        let out = r#"{"updatedInput": "not-an-object"}"#;
        let d = parse_directives(out, &pre_tool()).expect("directives");
        assert_eq!(d.updated_input, None);
    }

    #[test]
    fn apply_directives_rewrites_result() {
        let mut result = super::super::types::HookResult::allow();
        let directives = HookDirectives {
            permission_decision: Some(HookPermissionDecision::Deny("nope".to_string())),
            updated_input: Some(serde_json::json!({"command": "ls"})),
            additional_context: Some("context".to_string()),
        };
        apply_directives(&mut result, directives);
        assert!(!result.allow);
        assert_eq!(result.deny_reason.as_deref(), Some("nope"));
        assert_eq!(result.updated_input, Some(serde_json::json!({"command": "ls"})));
        assert_eq!(result.additional_context.as_deref(), Some("context"));
    }
}
