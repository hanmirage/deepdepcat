//! Workspace module — live submodules only.
//!
//! The legacy `Workspace` aggregation facade (fs/checkpoints/recovery),
//! `notify` file-watcher, `hunk_tracker`, `permission` policies, crash
//! recovery and `discovery` wrapper were removed as dead code — `AppState`
//! keeps workspace as a bare path, file I/O lives in the tools, permissions
//! in `src/permissions`, and session persistence in SQLite `SessionManager`.
//!
//! ## Submodules
//!
//! - `checkpoint` — `FileStateTracker`, per-turn file snapshots for rewind
//! - `isolation` — git worktree isolation for subagent execution
//! - `project_files` — `DEEPDEPCAT.md` instruction & memory discovery
//! - `project_structure` — workspace layout snapshot for context injection

pub mod checkpoint;
pub mod isolation;
pub mod project_files;
pub mod project_structure;
