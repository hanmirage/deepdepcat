use serde::{Deserialize, Serialize};

/// The sandbox profile — determines the level of isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SandboxProfile {
    /// Default — workspace read-write, system read-only, network allowed.
    #[default]
    Workspace,
    /// Read-only access to everything, no writes, no network.
    ReadOnly,
    /// Maximum isolation — workspace writable, system read-only,
    /// no network, no PID namespace sharing.
    Strict,
    /// Sandbox disabled — run commands directly.
    Off,
}

impl SandboxProfile {
    /// Whether the sandbox is active.
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Off)
    }
}
