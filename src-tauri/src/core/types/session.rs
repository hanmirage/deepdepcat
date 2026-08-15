use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextChip {
    /// `data_url` carries the image bytes for an attached picture (pasted /
    /// picked via the frontend clipboard) — present only for image chips.
    /// Images never reach the model as paths: they are transcribed to text
    /// on send. File chips without a data URL keep a filesystem path.
    File {
        name: String,
        path: String,
        // The frontend ContextChip type serializes this field as `dataUrl`;
        // serde keeps the Rust-side name `data_url` (snake_case), so accept
        // both spellings at the wire boundary.
        #[serde(default, alias = "dataUrl")]
        data_url: Option<String>,
    },
    Folder {
        name: String,
        path: String,
    },
    Url {
        name: String,
        path: String,
    },
}

impl ContextChip {
    /// The data URL of an attached image, when the frontend supplied one
    /// (pasted/picked images). `None` for non-image chips.
    pub fn data_url(&self) -> Option<&str> {
        match self {
            Self::File { data_url, .. } => data_url.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub args_summary: String,
    pub session_id: String,
    /// The session that spawned the executing agent as a SUBAGENT — lets
    /// the frontend route the prompt to the parent conversation the user
    /// is actually looking at (`None` for a main session).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// The grant identity an "always allow" would record (`cmd:git`,
    /// `path:...`, `mcp:server`, or `*` for whole tool). The dialog shows
    /// it so the user knows exactly what will be remembered.
    pub grant_pattern: String,
    /// Human-readable scope of [`Self::grant_pattern`] for the dialog.
    pub grant_scope: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SessionStatus {
    #[default]
    Active,
    Idle,
    Archived,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub model: String,
    pub provider: String,
    /// Context window (tokens) captured from the provider's real model
    /// metadata at session creation. 0 = unknown → fall back to the built-in
    /// catalog when the ChatState is rebuilt.
    #[serde(default)]
    pub context_window: u64,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    pub total_usage: super::info::TokenUsage,
    pub turn_count: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system_prompt: String,
    /// Product mode this session belongs to: "code" | "depwork".
    /// Drives frontend session restore (which store/view to open).
    #[serde(default = "default_work_mode")]
    pub work_mode: String,
    /// Per-session permission mode ("" = inherit the global default).
    /// Code and Depwork use different mode sets; the value is one of the
    /// canonical `PermissionMode` wire strings.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
    /// True when the user pinned this session to the top of the sidebar list.
    #[serde(default)]
    pub pinned: bool,
    /// Short preview of the session's last message (sidebar row subtitle).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_message: String,
    #[serde(skip)]
    pub is_streaming: bool,
}

fn default_work_mode() -> String {
    "code".to_string()
}

impl Session {
    pub fn new(model: impl Into<String>, provider: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: "New Session".to_string(),
            model: model.into(),
            provider: provider.into(),
            context_window: 0,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            workspace_path: None,
            total_usage: super::info::TokenUsage::default(),
            turn_count: 0,
            system_prompt: String::new(),
            work_mode: default_work_mode(),
            permission_mode: String::new(),
            pinned: false,
            last_message: String::new(),
            is_streaming: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AgentStatus {
    #[default]
    Idle,
    Thinking,
    ToolRunning,
    Connecting,
    Error,
    Paused,
}

impl AgentStatus {
    /// Wire string form (snake_case, identical to the serde serialization
    /// used on the agent-status-changed event channel) — the invoke channel
    /// uses the same encoding now (one contract, not u8↔string dual).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Thinking => "thinking",
            Self::ToolRunning => "tool_running",
            Self::Connecting => "connecting",
            Self::Error => "error",
            Self::Paused => "paused",
        }
    }

    pub fn from_str(v: &str) -> Self {
        match v {
            "thinking" => Self::Thinking,
            "tool_running" => Self::ToolRunning,
            "connecting" => Self::Connecting,
            "error" => Self::Error,
            "paused" => Self::Paused,
            _ => Self::Idle,
        }
    }

    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Thinking => 1,
            Self::ToolRunning => 2,
            Self::Connecting => 3,
            Self::Error => 4,
            Self::Paused => 5,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Thinking,
            2 => Self::ToolRunning,
            3 => Self::Connecting,
            4 => Self::Error,
            5 => Self::Paused,
            _ => Self::Idle,
        }
    }
}

impl From<u8> for AgentStatus {
    fn from(v: u8) -> Self {
        Self::from_u8(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_chip_file_accepts_frontend_data_url_key() {
        // The frontend ContextChip serializes image bytes under `dataUrl`
        // (camelCase). The Rust field is `data_url` — the alias keeps the
        // pasted/picked image pipeline working across the Tauri boundary.
        let json = r#"{
            "type": "file",
            "name": "clip.png",
            "path": "data:image/png;base64,AAAA",
            "dataUrl": "data:image/png;base64,AAAA"
        }"#;
        let chip: ContextChip = serde_json::from_str(json).expect("parses with camelCase key");
        match chip {
            ContextChip::File {
                name,
                path,
                data_url,
            } => {
                assert_eq!(name, "clip.png");
                assert_eq!(path, "data:image/png;base64,AAAA");
                assert_eq!(data_url.as_deref(), Some("data:image/png;base64,AAAA"));
            }
            _ => panic!("expected File variant"),
        }
    }

    #[test]
    fn context_chip_file_snake_case_still_works() {
        let json = r#"{
            "type": "file",
            "name": "doc.txt",
            "path": "/tmp/doc.txt"
        }"#;
        let chip: ContextChip = serde_json::from_str(json).expect("parses without data url");
        match chip {
            ContextChip::File { path, data_url, .. } => {
                assert_eq!(path, "/tmp/doc.txt");
                assert!(data_url.is_none());
            }
            _ => panic!("expected File variant"),
        }
    }
}
