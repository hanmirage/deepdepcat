//! Plugin policy layer — JSON policy restricting which plugins may install.
//!
//! Mirrors OpenAI Codex's plugin policy (AVAILABLE / NOT_AVAILABLE): a
//! blocked plugin id cannot be installed. Default = available for anything
//! not listed. Persisted as `plugin_policy.json` in the app data dir.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// `plugins: { "<id>": "available" | "blocked" }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginPolicy {
    pub plugins: HashMap<String, String>,
}

impl PluginPolicy {
    pub fn is_blocked(&self, plugin_id: &str) -> bool {
        self.plugins.get(plugin_id).map(|s| s.as_str()) == Some("blocked")
    }
}

/// Loads and persists the policy file atomically (tmp + rename).
pub struct PluginPolicyStore {
    policy: std::sync::RwLock<PluginPolicy>,
    path: PathBuf,
}

impl PluginPolicyStore {
    pub fn load(app_data_dir: &std::path::Path) -> Self {
        let path = app_data_dir.join("plugin_policy.json");
        let policy = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<PluginPolicy>(&raw).ok())
            .unwrap_or_default();
        Self {
            policy: std::sync::RwLock::new(policy),
            path,
        }
    }

    pub fn is_blocked(&self, plugin_id: &str) -> bool {
        self.policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_blocked(plugin_id)
    }

    pub fn snapshot(&self) -> PluginPolicy {
        self.policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set a plugin's policy entry (`available` or `blocked`).
    pub fn set(&self, plugin_id: &str, action: &str) -> bool {
        if !matches!(action, "available" | "blocked") {
            return false;
        }
        let mut policy = self.policy.write().unwrap_or_else(|e| e.into_inner());
        policy
            .plugins
            .insert(plugin_id.to_string(), action.to_string());
        drop(policy);
        self.persist();
        true
    }

    fn persist(&self) {
        let policy = self.policy.read().unwrap_or_else(|e| e.into_inner());
        if let Ok(raw) = serde_json::to_string_pretty(&*policy) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, raw).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_plugins_are_rejected_and_persisted() {
        let dir = std::env::temp_dir().join(format!(
            "ddc-policy-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = PluginPolicyStore::load(&dir);
        assert!(!store.is_blocked("anything"));

        assert!(store.set("evil-plugin", "blocked"));
        assert!(store.is_blocked("evil-plugin"));
        assert!(!store.is_blocked("good-plugin"));
        // Invalid actions are rejected.
        assert!(!store.set("x", "banana"));

        let reloaded = PluginPolicyStore::load(&dir);
        assert!(reloaded.is_blocked("evil-plugin"));
    }
}
