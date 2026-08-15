//! Batch tool execution with PreToolUse/PostToolUse hook gates.
//!
//! Tool calls are partitioned into two groups:
//! - **Parallel-safe** — tools where `is_concurrency_safe()` returns `true`
//!   (read-only tools like `read_file`, `grep`, `glob`). These execute
//!   concurrently via `join_all`, then results are pushed to `chat_state`
//!   in original order.
//! - **Serial** — side-effecting tools (`write_file`, `bash`, etc.). These
//!   execute one-at-a-time with full hook gating.
//!
//! Each tool call is gated by PreToolUse hooks. If a hook denies, the deny
//! reason is fed back as the tool result (`is_error = true`) and the turn
//! continues so the model can adapt — the deny does NOT cancel the turn.
//!
//! After each tool executes, PostToolUse hooks fire (observe-only).

mod concurrent;
mod orchestrate;
mod parallel;
mod serial;
mod support;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use support::*;
