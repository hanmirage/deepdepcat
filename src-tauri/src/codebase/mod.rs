//! Codebase indexing — file dependency graph and multi-language symbol extraction.
//!
//! Provides:
//! - **Symbol index** — extracts function, struct, class, interface definitions
//!   from Rust, TypeScript, and Python source files
//! - **Dependency graph** — builds a file-level import dependency DAG using
//!   regex-based import extraction
//! - **Cognition** — module-level aggregation of the index (the project map
//!   injected into the agent's context for long-task planning)
//!
//! The `navigation` (go-to-definition) and `scope_graph` modules were removed
//! as unwired dead code — production uses `symbols` + `dependency` only.

pub mod cognition;
pub mod dependency;
pub mod symbols;
