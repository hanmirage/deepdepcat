//! ID generation utilities.

use uuid::Uuid;

/// Generate a random UUID v4 string.
pub fn generate_id() -> String {
    Uuid::new_v4().to_string()
}

/// Generate a turn ID — single identifier for all events within one
/// conversation turn (across LLM calls and tool executions).
pub fn turn_id() -> String {
    format!("turn-{}", &Uuid::new_v4().to_string()[..8])
}

/// Generate a trace ID — one identifier for a whole agent run, propagated
/// through stream events (chat-stream / ACP / SSE) and the event log so a
/// single task can be followed across protocols.
pub fn trace_id() -> String {
    format!("trace-{}", &Uuid::new_v4().to_string()[..12])
}

/// Generate a tool call ID (must start with "call_" for some APIs).
pub fn tool_call_id() -> String {
    format!("call_{}", &Uuid::new_v4().to_string()[..12])
}

/// Generate a task ID with a type prefix.
pub fn task_id(prefix: &str) -> String {
    format!("{}_{}", prefix, &Uuid::new_v4().to_string()[..8])
}
