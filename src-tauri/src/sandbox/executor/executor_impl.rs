use std::path::PathBuf;

use super::profile::SandboxProfile;

/// The sandbox executor — runs commands with configurable isolation.
pub struct SandboxExecutor {
    /// The active sandbox profile.
    pub(crate) profile: SandboxProfile,
}

impl SandboxExecutor {
    /// Create a new sandbox executor with the given profile and workspace.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile: SandboxProfile,
        _workspace_root: Option<PathBuf>,
        _app_data_dir: Option<PathBuf>,
    ) -> Self {
        Self { profile }
    }

    /// Get the current sandbox profile.
    pub fn profile(&self) -> SandboxProfile {
        self.profile
    }
}
