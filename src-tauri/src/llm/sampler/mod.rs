//! LLM sampling helpers — doom-loop detection shared with the agent loop.
//!
//! The `actor` sampling wrapper (metrics tracking + automatic recovery
//! orchestration), its `config` and `metrics` modules, and `model_pool.rs`
//! (multi-key rotation) were removed as unwired dead code. What remains is
//! `doom_loop`, whose detector and recovery prompt ARE consumed by the agent
//! loop (`agent_loop/mod.rs`, `agent_loop/recovery.rs`).

pub mod doom_loop;

pub use doom_loop::{recovery_prompt, DoomLoopDetector, DoomLoopSignal};
