//! Sandbox — command isolation and deny path enforcement.
//!
//! The `patterns` (command classification), `deny` (glob deny-list) and
//! `profiles` (TOML custom-profile loading) modules were removed as dead
//! code — the sandbox executor runs with the built-in `SandboxProfile`
//! (executor/profile.rs) and no production path consumed the removed layers.
//!
//! Actual isolation today (cross-platform reality): the only live mechanism
//! is Windows — a Job Object + restricted-token filter applied at the bash
//! spawn site (`core::proc::win::job::JobObject`), giving process-tree
//! isolation + admin-SID stripping for Strict/ReadOnly profiles. Linux
//! `bwrap` and macOS `sandbox-exec` are NOT implemented (earlier doc links
//! were aspirational and removed).
//!
//! `SandboxExecutor` is a thin carrier for the profile; the bash tool reads
//! it and applies the platform mechanism. See `docs/SANDBOX_BOUNDARIES.md`.

pub mod executor;
