//! Hook System — 20+ lifecycle events with 4 execution types.
//!
//! Events: SessionStart, SessionEnd, AgentLoopStart, AgentLoopEnd,
//! PreToolUse, PostToolUse, ToolError, PreLLMCall, PostLLMCall,
//! LLMStreamStart, LLMStreamEnd, UserMessage, AssistantMessage,
//! PreCompaction, PostCompaction, Error, FatalError, FileChanged,
//! PermissionDenied, MemoryStored, McpServerConnected, etc.
//!
//! Execution types: Command, Prompt, Agent, Http.

pub mod discovery;
pub mod env_expand;
pub mod eval;
pub mod executor;
pub mod json_directive;
pub mod registry;
pub mod ssrf;
pub mod trust;
pub mod types;

pub use executor::HookExecutor;
pub use registry::HookRegistry;
pub use types::{HookContext, HookEvent};
