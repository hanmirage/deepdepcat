//! Task notification — structured `<task-notification>` XML messages.
//!
//! Background task completions (subagents, workflows) are injected into the
//! parent agent's conversation as a single XML block the model can parse
//! reliably, instead of free-form text. Mirrors the upstream protocol:
//!
//! ```xml
//! <task-notification>
//!   <task-id>a_abc12345</task-id>
//!   <description>Analyze codebase</description>
//!   <status>completed</status>
//!   <result>Found 3 modules...</result>
//! </task-notification>
//! ```

use serde::{Deserialize, Serialize};

/// A structured task completion notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotification {
    /// Task ID (type-prefixed, e.g. `a_abc12345` for agents).
    pub task_id: String,
    /// Human-readable task description.
    pub description: String,
    /// Terminal status: completed | failed | killed.
    pub status: String,
    /// Result summary (empty for failures without detail).
    pub result: String,
}

impl TaskNotification {
    /// Render the notification as a `<task-notification>` XML block.
    pub fn to_xml(&self) -> String {
        let status = xml_escape(&self.status);
        let description = xml_escape(&self.description);
        let result = xml_escape(&self.result);
        format!(
            "<task-notification>\n\
             \x20 <task-id>{}</task-id>\n\
             \x20 <description>{}</description>\n\
             \x20 <status>{}</status>\n\
             \x20 <result>{}</result>\n\
             </task-notification>",
            xml_escape(&self.task_id),
            description,
            status,
            result
        )
    }
}

/// Escape XML special characters.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build a task notification from a background subagent result shape.
pub fn from_background_result(
    task_id: &str,
    task: &str,
    success: bool,
    response: &str,
) -> TaskNotification {
    TaskNotification {
        task_id: task_id.to_string(),
        description: task.to_string(),
        status: if success { "completed" } else { "failed" }.to_string(),
        result: response.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_well_formed_xml() {
        let n = TaskNotification {
            task_id: "a_abc12345".to_string(),
            description: "分析代码库结构".to_string(),
            status: "completed".to_string(),
            result: "发现 3 个模块".to_string(),
        };
        let xml = n.to_xml();
        assert!(xml.starts_with("<task-notification>"));
        assert!(xml.contains("<task-id>a_abc12345</task-id>"));
        assert!(xml.contains("<status>completed</status>"));
        assert!(xml.ends_with("</task-notification>"));
    }

    #[test]
    fn escapes_special_characters() {
        let n = TaskNotification {
            task_id: "a_1".to_string(),
            description: "a < b & c > d".to_string(),
            status: "failed".to_string(),
            result: "error: 'quotes' \"double\"".to_string(),
        };
        let xml = n.to_xml();
        assert!(xml.contains("a &lt; b &amp; c &gt; d"));
        assert!(!xml.contains("<description>a < b"));
    }

    #[test]
    fn builds_from_background_result() {
        let n = from_background_result("a_9", "task", false, "boom");
        assert_eq!(n.status, "failed");
        assert_eq!(n.result, "boom");
    }
}
