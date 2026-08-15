use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    LocalBash,
    LocalAgent,
    RemoteAgent,
    LocalWorkflow,
    MonitorMcp,
    Dream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Killed,
}

impl TaskStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Killed)
    }

    /// Stable lowercase wire string — mirrors the serde rename_all used for
    /// JSON (used for SQLite persistence keys).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoworkTask {
    pub id: String,
    pub description: String,
    pub status: TaskStatus,
    pub context_paths: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}
