//! Sandbox executor — carries the isolation profile applied at the spawn
//! site.
//!
//! The actual process isolation is platform-specific and lives at the bash
//! spawn site (`tools/builtin/bash.rs`):
//! - **Windows**: Job Object process-tree isolation — `Strict`/`ReadOnly`
//!   profiles additionally use a restricted-token job (admin SIDs stripped,
//!   SeDebugPrivilege removed), see `core::proc::win::job::JobObject`.
//! - **Linux / macOS**: not implemented.
//!
//! Five built-in profiles control the level of isolation.

mod executor_impl;
mod profile;

#[cfg(test)]
mod tests;

pub use executor_impl::SandboxExecutor;
pub use profile::SandboxProfile;
