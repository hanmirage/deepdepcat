//! Agent handler modules — modular handlers for specific agent lifecycle events.
//!
//! Only the session-lifecycle handler remains (the idle-reaper's dormancy
//! notification). The `model_switch` and `workspaces` handlers were removed
//! as unwired dead code.

pub mod session;
