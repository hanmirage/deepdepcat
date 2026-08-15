//! Toolkit — 工具契约层（能力层的公共类型）。
//!
//! 这是 `Tool` trait 与其配套类型的中立归属：`tools/`（工具实现）与
//! `agent/`（harness 消费方）都依赖这里，但 toolkit 不依赖它们。
//! 属于分层图的 capability 底座：只依赖 core / workspace / observability 等 infra。
//!
//! - `scope` — WorkMode（Code/Depwork 产品面）+ ToolScope（工具可用范围）
//! - `tool` — Tool trait、ToolContext、ToolResult、PermissionDecision、流式类型

pub mod scope;
pub mod tool;

pub use scope::{ToolScope, WorkMode};
pub use tool::{
    PermissionDecision, Tool, ToolBehaviorVersion, ToolContext, ToolImage, ToolProgress,
    ToolResult, ToolStreamItem, tool_to_definition,
};
