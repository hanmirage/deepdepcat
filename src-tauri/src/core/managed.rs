//! Server configuration — the backend base URL used by diagnostics flush
//! and feature-flag fetching.
//!
//! The old `ManagedConfig` (server-pushed feature toggles / resource limits /
//! deployment flags) was removed as dead code (#85 audit): all 9 fields were
//! fetched and stored but never consumed — `telemetry_enabled` duplicated the
//! already-wired frontend `diagnosticsEnabled` toggle, and the rest had no
//! consumer. Only the server URL plumbing (used by the diagnostics flush loop)
//! survives.

use serde::{Deserialize, Serialize};

/// Backend server connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Base URL of the DeepDepCat backend server.
    pub base_url: String,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum retries for failed requests.
    pub max_retries: u32,
    /// Retry base delay in milliseconds.
    pub retry_base_delay_ms: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            base_url: "https://deepdepcat.hsmiai.xyz".to_string(),
            timeout_secs: 30,
            max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }
}

/// Apply environment variable overrides to server config.
///
/// Supports:
/// - `DEEPDEPCAT_SERVER_URL` — override the backend server URL
/// - `DEEPDEPCAT_SERVER_TIMEOUT` — override the request timeout
pub fn apply_env_overrides(server_config: &mut ServerConfig) {
    if let Ok(url) = std::env::var("DEEPDEPCAT_SERVER_URL") {
        server_config.base_url = url;
    }
    if let Ok(timeout) = std::env::var("DEEPDEPCAT_SERVER_TIMEOUT") {
        if let Ok(t) = timeout.parse::<u64>() {
            server_config.timeout_secs = t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_overrides_apply() {
        let mut server = ServerConfig::default();
        std::env::set_var("DEEPDEPCAT_SERVER_URL", "https://custom.example.com");
        std::env::set_var("DEEPDEPCAT_SERVER_TIMEOUT", "15");

        apply_env_overrides(&mut server);

        assert_eq!(server.base_url, "https://custom.example.com");
        assert_eq!(server.timeout_secs, 15);

        std::env::remove_var("DEEPDEPCAT_SERVER_URL");
        std::env::remove_var("DEEPDEPCAT_SERVER_TIMEOUT");
    }
}
