//! user_profile — read/write the user's profile file (`~/.deepdepcat/USER.md`).
//!
//! The profile is injected into the static system prompt (`## User Profile`).
//! It uses the managed-section pattern (Qwen-style): the agent may only
//! rewrite the content between `<!-- managed:user-profile -->` markers; the
//! user's own hand-written text outside the markers is preserved verbatim.
//! Writes are atomic (temp file + rename) so a crash mid-write can never
//! corrupt the profile.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use crate::workspace::project_files::user_deepdepcat_dir;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

/// Managed-section markers — only text between these is agent-writable.
pub const MANAGED_START: &str = "<!-- managed:user-profile -->";
pub const MANAGED_END: &str = "<!-- /managed:user-profile -->";

/// Replace the managed section in the profile with `new_content`, preserving
/// everything outside the markers. When no markers exist, the managed section
/// is appended at the end.
pub fn rewrite_managed_section(original: &str, new_content: &str) -> String {
    let trimmed_content = new_content.trim();
    if let (Some(s), Some(e)) = (original.find(MANAGED_START), original.find(MANAGED_END)) {
        let end = e + MANAGED_END.len();
        format!(
            "{}{}\n{}{}\n{}",
            &original[..s],
            MANAGED_START,
            trimmed_content,
            MANAGED_END,
            &original[end..]
        )
    } else {
        format!(
            "{}\n\n{}\n{}\n{}\n",
            original.trim_end(),
            MANAGED_START,
            trimmed_content,
            MANAGED_END
        )
    }
}

/// Atomic write: temp file in the same directory, then rename over the target.
fn atomic_write(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub struct UserProfileTool;

impl UserProfileTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for UserProfileTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::All
    }

    fn name(&self) -> &str {
        "user_profile"
    }

    fn description(&self) -> &str {
        "Read or update the user's profile file (~/.deepdepcat/USER.md), which is \
         injected into the system prompt as '## User Profile'. The agent may only \
         rewrite the section between the `<!-- managed:user-profile -->` markers; \
         the user's own hand-written content outside the markers is preserved. \
         Use it to remember stable user preferences, identity, and long-term \
         constraints across sessions. Pass `read: true` to return the current \
         profile, or `content` to update the managed section."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "read": {
                    "type": "boolean",
                    "description": "When true, return the current profile content without writing."
                },
                "content": {
                    "type": "string",
                    "description": "New content for the managed section (replaces everything between the markers)."
                }
            }
        })
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let read_only = args.get("read").and_then(|v| v.as_bool()).unwrap_or(false);
        let path = user_deepdepcat_dir().join("USER.md");

        let current = if path.exists() {
            crate::core::encoding::decode_native_output(&std::fs::read(&path)?)
        } else {
            String::new()
        };

        if read_only || !args.get("content").and_then(|c| c.as_str()).is_some() {
            if current.trim().is_empty() {
                return Ok(ToolResult::success(
                    "No user profile yet. Write one with `content` — the managed section is created automatically.",
                ));
            }
            return Ok(ToolResult::success(current));
        }

        let new_content = args
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();
        let updated = rewrite_managed_section(&current, &new_content);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write(&path, &updated)
            .map_err(|e| format!("Failed to write user profile {}: {e}", path.display()))?;
        info!(path = %path.display(), "User profile updated");

        Ok(ToolResult::success(format!(
            "User profile updated ({} chars, managed section rewritten, your hand-written content preserved).",
            updated.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_managed_section_only() {
        let original = "My name is Alice.\n\n<!-- managed:user-profile -->\nold agent notes\n<!-- /managed:user-profile -->\n\nUser tail";
        let out = rewrite_managed_section(original, "new notes");
        assert!(out.contains("My name is Alice."));
        assert!(out.contains("User tail"));
        assert!(out.contains("new notes"));
        assert!(!out.contains("old agent notes"));
    }

    #[test]
    fn appends_section_when_absent() {
        let out = rewrite_managed_section("just user text", "agent notes");
        assert!(out.contains("just user text"));
        assert!(out.contains(MANAGED_START));
        assert!(out.contains(MANAGED_END));
        assert!(out.contains("agent notes"));
    }

    #[test]
    fn multiple_rewrites_stay_stable() {
        let mut s = String::new();
        for i in 0..3 {
            s = rewrite_managed_section(&s, &format!("notes {i}"));
        }
        assert_eq!(s.matches(MANAGED_START).count(), 1);
        assert_eq!(s.matches(MANAGED_END).count(), 1);
        assert!(s.contains("notes 2"));
        assert!(!s.contains("notes 0"));
    }
}
