use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DebugEvent {
    AgentTurnStart {
        session_id: String,
        turn: u32,
        mode: String,
        timestamp: f64,
    },
    AgentTurnEnd {
        session_id: String,
        turn: u32,
        duration_ms: u64,
        timestamp: f64,
    },
    LlmCallStart {
        session_id: String,
        model: String,
        message_count: u32,
        timestamp: f64,
    },
    LlmCallEnd {
        session_id: String,
        model: String,
        duration_ms: u64,
        usage: DebugUsage,
        timestamp: f64,
    },
    ToolDispatch {
        session_id: String,
        tool_name: String,
        arguments: String,
        timestamp: f64,
    },
    ToolResult {
        session_id: String,
        tool_name: String,
        duration_ms: u64,
        is_error: bool,
        timestamp: f64,
    },
    MemorySearch {
        session_id: String,
        query: String,
        results_count: u32,
        duration_ms: u64,
        timestamp: f64,
    },
    MemoryInject {
        session_id: String,
        memories_count: u32,
        timestamp: f64,
    },
    PermissionCheck {
        session_id: String,
        resource: String,
        action: String,
        allowed: bool,
        timestamp: f64,
    },
    HookTrigger {
        session_id: String,
        event: String,
        timestamp: f64,
    },
    HookExecute {
        session_id: String,
        event: String,
        hook_id: String,
        duration_ms: u64,
        timestamp: f64,
    },
    Compaction {
        session_id: String,
        compacted_tokens: u32,
        summary: String,
        timestamp: f64,
    },
    SubagentSpawn {
        session_id: String,
        subagent_id: String,
        agent_type: String,
        depth: u32,
        timestamp: f64,
    },
    SubagentResult {
        session_id: String,
        subagent_id: String,
        duration_ms: u64,
        success: bool,
        timestamp: f64,
    },
}

impl DebugEvent {
    fn now() -> f64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }

    pub fn agent_turn_start(session_id: &str, turn: u32, mode: &str) -> Self {
        Self::AgentTurnStart {
            session_id: session_id.into(),
            turn,
            mode: mode.into(),
            timestamp: Self::now(),
        }
    }

    pub fn llm_call_start(session_id: &str, model: &str, message_count: u32) -> Self {
        Self::LlmCallStart {
            session_id: session_id.into(),
            model: model.into(),
            message_count,
            timestamp: Self::now(),
        }
    }

    pub fn llm_call_end(
        session_id: &str,
        model: &str,
        duration_ms: u64,
        usage: super::info::TokenUsage,
    ) -> Self {
        Self::LlmCallEnd {
            session_id: session_id.into(),
            model: model.into(),
            duration_ms,
            usage: DebugUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
            },
            timestamp: Self::now(),
        }
    }

    pub fn tool_dispatch(session_id: &str, tool_name: &str, arguments: &str) -> Self {
        Self::ToolDispatch {
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            arguments: arguments.into(),
            timestamp: Self::now(),
        }
    }

    pub fn tool_result(
        session_id: &str,
        tool_name: &str,
        duration_ms: u64,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            duration_ms,
            is_error,
            timestamp: Self::now(),
        }
    }

    pub fn hook_trigger(session_id: &str, event: &str) -> Self {
        Self::HookTrigger {
            session_id: session_id.into(),
            event: event.into(),
            timestamp: Self::now(),
        }
    }
}

pub fn emit_debug_trace(app: &tauri::AppHandle, debug_mode: bool, event: DebugEvent) {
    if debug_mode {
        use tauri::Emitter;
        let _ = app.emit("debug-trace", event);
    }
}
