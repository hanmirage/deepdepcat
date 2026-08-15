//! Feature flag system — remote-controlled feature toggles.
//!
//! Flags are fetched from the server at startup and cached locally.
//! When offline, the cached values are used. Users can override flags
//! locally via the settings UI.
//!
//! Usage in Rust:
//! ```ignore
//! if feature_flag!("coordinator_mode") {
//!     // ...
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// A single feature flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlag {
    pub key: String,
    pub enabled: bool,
    /// Percentage of users that should see this feature (0-100).
    pub rollout_percent: u8,
    /// Description for the settings UI.
    pub description: String,
}

/// Configuration for the feature flag system.
#[derive(Debug, Clone)]
pub struct FeatureFlagConfig {
    /// Full URL of the flags endpoint
    /// (e.g. "https://deepdepcat.hsmiai.xyz/api/v1/config/flags").
    pub server_url: String,
    /// Cache TTL in seconds.
    pub cache_ttl_secs: u64,
}

impl Default for FeatureFlagConfig {
    fn default() -> Self {
        Self {
            server_url: "https://deepdepcat.hsmiai.xyz/api/v1/config/flags".to_string(),
            cache_ttl_secs: 3600,
        }
    }
}

/// Manages feature flags with server sync and local cache.
pub struct FeatureFlagManager {
    config: FeatureFlagConfig,
    flags: RwLock<HashMap<String, FeatureFlag>>,
    last_fetch: RwLock<Option<Instant>>,
}

impl FeatureFlagManager {
    /// Create a new manager with the given configuration.
    pub fn new(config: FeatureFlagConfig) -> Self {
        Self {
            config,
            flags: RwLock::new(HashMap::new()),
            last_fetch: RwLock::new(None),
        }
    }

    /// Set a flag's enabled state locally (user override).
    pub fn set_flag(&self, key: &str, enabled: bool) {
        let mut flags = self.flags.write().unwrap_or_else(|e| e.into_inner());
        flags
            .entry(key.to_string())
            .and_modify(|f| f.enabled = enabled)
            .or_insert(FeatureFlag {
                key: key.to_string(),
                enabled,
                rollout_percent: 100,
                description: String::new(),
            });
    }

    /// Get all flags (for the settings UI).
    pub fn list_flags(&self) -> Vec<FeatureFlag> {
        let flags = self.flags.read().unwrap_or_else(|e| e.into_inner());
        flags.values().cloned().collect()
    }

    /// Fetch flags from the server. Called at startup and periodically.
    ///
    /// On failure, the cached values are kept. Errors are logged, never
    /// propagated — a dead flags endpoint must not break the app.
    pub async fn fetch_flags(&self) {
        if self.config.server_url.is_empty() {
            return;
        }

        // Check cache freshness.
        {
            let last = self.last_fetch.read().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = *last {
                if t.elapsed() < Duration::from_secs(self.config.cache_ttl_secs) {
                    return;
                }
            }
        }

        let client = reqwest::Client::new();
        match client.get(&self.config.server_url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<Vec<FeatureFlag>>().await {
                Ok(new_flags) => {
                    let mut flags = self.flags.write().unwrap_or_else(|e| e.into_inner());
                    flags.clear();
                    for flag in new_flags {
                        flags.insert(flag.key.clone(), flag);
                    }
                    *self.last_fetch.write().unwrap_or_else(|e| e.into_inner()) =
                        Some(Instant::now());
                    info!("Fetched {} feature flags from server", flags.len());
                }
                Err(e) => {
                    warn!("Failed to parse feature flags: {}", e);
                }
            },
            Ok(resp) => {
                warn!("Feature flag server returned {}", resp.status());
            }
            Err(e) => {
                warn!("Failed to fetch feature flags: {}", e);
            }
        }
    }
}

/// Macro to check a feature flag.
#[macro_export]
macro_rules! feature_flag {
    ($manager:expr, $key:expr) => {
        $manager.is_enabled($key)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_flags_returns_all() {
        let mgr = FeatureFlagManager::new(FeatureFlagConfig::default());
        mgr.set_flag("a", true);
        mgr.set_flag("b", false);
        let flags = mgr.list_flags();
        assert_eq!(flags.len(), 2);
    }
}
