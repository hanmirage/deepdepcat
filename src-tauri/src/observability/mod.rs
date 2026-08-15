//! Observability — structured tracing, token tracking, and event correlation.
//!
//! Provides:
//! - **Session-level token tracking** — accumulates usage across turns, tools, and LLM calls
//! - **Structured tracing spans** — every operation (LLM call, tool execution, hook, subagent)
//!   creates a `tracing::Span` with contextual fields (session_id, message_id, tool_name)
//! - **Event correlation** — IDs propagated through spans for distributed-trace-style analysis
//! - **Metrics aggregation** — turn-level deltas emitted as `StreamEvent::Usage`

pub mod diagnostics;
pub mod event_log;
pub mod heartbeat;
pub mod usage;
