//! Unified error type for the entire DeepDepCat backend.
//!
//! Uses `thiserror` for ergonomic error derivation and `From` impls so
//! `?` works seamlessly across subsystem boundaries.

use std::io;
use thiserror::Error;

/// The one error type used everywhere in the backend.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("LLM API error: {source}")]
    LlmApi {
        source: Box<dyn std::error::Error + Send + Sync>,
        status_code: Option<u16>,
    },

    #[error("LLM streaming error: {0}")]
    LlmStreaming(String),

    #[error("LLM rate limited (retry after {retry_after_secs:?}s)")]
    LlmRateLimited { retry_after_secs: Option<u64> },

    #[error("LLM authentication failed: {0}")]
    LlmAuth(String),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Tool execution error in '{tool_name}': {message}")]
    ToolExecution { tool_name: String, message: String },

    #[error("Permission denied for '{tool_name}': {reason}")]
    PermissionDenied { tool_name: String, reason: String },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),

    #[error("Channel send error: {0}")]
    ChannelSend(String),

    #[error("Lock poisoned: {0}")]
    LockPoisoned(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Memory error: {0}")]
    Memory(String),

    #[error("Hook error: {0}")]
    Hook(String),

    #[error("Multi-agent error: {0}")]
    MultiAgent(String),

    #[error("Sandbox violation: {0}")]
    Sandbox(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Path error: {0}")]
    Path(String),

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error("Cancelled by user")]
    Cancelled,

    #[error("Context window overflow: estimated {estimated} > limit {limit}")]
    ContextOverflow { estimated: u64, limit: u64 },

    #[error("Max turns ({0}) exceeded")]
    MaxTurnsExceeded(u32),

    #[error("Prompt too long for context window (max tokens: {max_tokens:?})")]
    PromptTooLong { max_tokens: Option<u64> },

    #[error("Output token limit exceeded: requested {requested}, max {max}")]
    MaxTokensExceeded { requested: u64, max: u64 },

    #[error("Model fallback exhausted: primary={primary}, tried={tried:?}")]
    ModelFallbackExhausted { primary: String, tried: Vec<String> },

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// Create a simple internal error from any Display.
    pub fn internal(msg: impl std::fmt::Display) -> Self {
        Self::Internal(msg.to_string())
    }

    /// Whether this error is retryable (transient network/rate-limit issues).
    pub fn is_retryable(&self) -> bool {
        let transient = matches!(
            self,
            Self::LlmRateLimited { .. } | Self::Http(_) | Self::LlmApi { .. } | Self::Timeout(_)
        );
        // 400 (bad request) and 401 (authentication) are permanent — no
        // retry count or backoff will ever fix them.
        transient && !self.is_client_rejection()
    }

    /// Whether this error is a 400/401-style client rejection that retrying
    /// cannot fix.
    fn is_client_rejection(&self) -> bool {
        match self {
            Self::LlmApi { status_code, .. } => matches!(status_code, Some(400) | Some(401)),
            Self::Http(err) => {
                matches!(err.status().map(|s| s.as_u16()), Some(400) | Some(401))
            }
            _ => false,
        }
    }

    /// Whether this error indicates the prompt exceeded the context window.
    /// The agent loop uses this to trigger emergency compaction.
    pub fn is_prompt_too_long(&self) -> bool {
        matches!(self, Self::PromptTooLong { .. })
    }

    /// Whether this error indicates the output token limit was exceeded.
    /// The agent loop uses this to escalate `max_tokens` on the next attempt.
    pub fn is_max_tokens_exceeded(&self) -> bool {
        matches!(self, Self::MaxTokensExceeded { .. })
    }

    /// Whether this error is a 529 (overloaded) server error.
    /// Used for consecutive-529 tracking to trigger model fallback.
    pub fn is_overloaded(&self) -> bool {
        matches!(
            self,
            Self::LlmApi {
                status_code: Some(529),
                ..
            }
        )
    }

    /// Whether this error is a permission denial.
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, Self::PermissionDenied { .. })
    }

    /// Whether this error indicates the operation was cancelled.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}

impl<T> From<std::sync::PoisonError<T>> for AppError {
    fn from(e: std::sync::PoisonError<T>) -> Self {
        Self::LockPoisoned(e.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e.to_string())
    }
}

impl From<toml::de::Error> for AppError {
    fn from(e: toml::de::Error) -> Self {
        Self::Config(e.to_string())
    }
}

impl From<uuid::Error> for AppError {
    fn from(e: uuid::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

impl From<zip::result::ZipError> for AppError {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Other(format!("Zip error: {e}"))
    }
}

impl From<csv::Error> for AppError {
    fn from(e: csv::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

impl From<lopdf::Error> for AppError {
    fn from(e: lopdf::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for AppError {
    fn from(e: windows::core::Error) -> Self {
        Self::Other(format!("Win32 error: {e}"))
    }
}

/// Convenience type alias.
pub type AppResult<T> = Result<T, AppError>;

/// Convert an AppError to a string for Tauri command results.
/// Tauri commands need `Result<T, String>`, so we implement `From` for that.
impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.to_string()
    }
}
