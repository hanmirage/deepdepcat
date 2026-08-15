//! Permission System — Claude-aligned permission design.
//!
//! Decision priority: **deny > ask > allow**. The checker evaluates a tool
//! call through nine ordered phases (see `checker::check_single` for the
//! authoritative order and rationale):
//!
//! 1. Agent contract — deny veto (an agent's own rules are a hard boundary)
//! 2. Project rules (`settings.json` / `settings.local.json`)
//! 3. Agent allows
//! 4. Settings rules + mode (ChatOnly / Plan / Default / AutoAccept /
//!    AcceptEdits)
//! 5. Agent asks
//! 6. Filesystem validation (deny/ask zones, traversal)
//! 7. Sensitive-file preflight (secret files always Ask, even in bypass)
//! 8. Bash security (unified `security::bash`) + network policy
//! 9. Default allow
//!
//! Bash compound commands are split into statements first, so a safe head
//! can never carry a destructive tail. A returned Allow still passes the
//! denial-cooldown (repeated denials pause for ~60s; recovery tools stay
//! exempt). Modes are resolved per session in `bootstrap::mode`
//! (`effective_session_mode`).
//!
//! Module layout: `checker` orchestrates the phases; `rules`/`mode`/
//! `denial`/`grant_store` hold decision state; `security/` holds the
//! defense-in-depth checks that run after rules (rules can allow, security
//! never trusts the allow).

// Re-export submodules
pub mod auto_review;
pub mod checker;
pub mod denial;
pub mod grant_store;
pub mod mode;
pub mod plan;
pub mod plugin_policy;
pub mod result;
pub mod rules;
pub mod security;

#[cfg(test)]
mod integration_tests;

/// Serializes tests that set the process-global `DEEPDEPCAT_DATA_DIR` env
/// var and initialize an AppState (permissions integration tests + the
/// tool-schema budget test). The env var is global, so concurrent tests can
/// overwrite each other's data dir and two `AppState::initialize` calls land
/// on the same DB → `UNIQUE constraint failed: _migrations.version`.
#[cfg(test)]
pub(crate) static DATA_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// Re-export key types from checker + security
pub use checker::PermissionChecker;
pub use security::{filesystem, network, sensitive};
