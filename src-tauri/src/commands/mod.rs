//! Tauri command handlers — the IPC layer between frontend and backend.
//!
//! All commands return `Result<T, String>` as required by Tauri.

pub mod agent;
pub mod auth_cmd;
pub mod automation_cmd;
pub mod browser_cmd;
pub mod chat;
pub mod chat_capture;
pub mod chat_chips;
pub mod chat_image;
pub mod chat_types;
pub mod cloud_cmd;
pub mod compaction_cmd;
pub mod config_cmd;
pub mod connector;
pub mod crash_cmd;
pub mod feedback_cmd;
pub mod hook_cmd;
pub mod mcp_cmd;
pub mod memory_cmd;
pub mod model_cmd;
pub mod observability_cmd;
pub mod pdf_cmd;
pub mod permission_cmd;
pub mod permission_governance_cmd;
pub mod plan_cmd;
pub mod preview;
pub mod rewind;
pub mod session;
pub mod sync_cmd;
pub mod system;
pub mod task_cmd;
pub mod tools;
pub mod update;
