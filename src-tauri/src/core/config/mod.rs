//! Configuration system — loads, merges, and provides access to all
//! application settings.
//!
//! Configuration layers (highest priority first):
//! 1. Environment variables (`DEEPDEPCAT_*`)
//! 2. Project config (`<workspace>/.deepdepcat/config.toml`)
//! 3. User config (`~/.deepdepcat/config.toml`)
//! 4. Built-in defaults

mod sections;

pub use sections::*;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::core::error::{AppError, AppResult};

/// The root configuration structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppConfig {
    /// General application settings.
    pub app: AppSection,
    /// LLM provider settings.
    pub llm: LlmSection,
    /// Agent runtime settings.
    pub agent: AgentSection,
    /// Tool system settings.
    pub tools: ToolsSection,
    /// Permission system settings.
    pub permissions: PermissionsSection,
    /// Storage/database settings.
    pub storage: StorageSection,
    /// MCP server configurations.
    pub mcp: McpSection,
    /// Hook configurations.
    pub hooks: HooksSection,
    /// Memory system settings.
    pub memory: MemorySection,
    /// Skill system settings (ecosystem compat gates).
    pub skills: SkillsSection,
    /// UI-related settings (forwarded to frontend).
    pub ui: UiSection,
    /// Telemetry/tracing settings.
    pub telemetry: TelemetrySection,
    /// Vision model configuration (image transcription for text-only models).
    pub vision: VisionSection,
}

impl AppConfig {
    /// Load configuration from all sources and merge them.
    ///
    /// Priority: env vars > project config > user config > defaults.
    ///
    /// Merging is performed at the JSON level so that individual fields from
    /// higher-priority sources override only the fields they explicitly set,
    /// while absent fields fall through to lower-priority sources.
    pub fn load(app_data_dir: &Path, workspace: Option<&Path>) -> AppResult<Self> {
        let mut config_json = serde_json::to_value(Self::default())
            .map_err(|e| AppError::Config(format!("Failed to serialize default config: {}", e)))?;

        // 1. User-level config: ~/.deepdepcat/config.toml
        let user_config_path = app_data_dir.join("config.toml");
        if user_config_path.exists() {
            match Self::load_file_json(&user_config_path) {
                Ok(user_json) => {
                    info!("Loaded user config from {:?}", user_config_path);
                    deep_merge_json(&mut config_json, &user_json);
                }
                Err(e) => {
                    warn!("Failed to parse user config {:?}: {}", user_config_path, e);
                }
            }
        }

        // 2. Project-level config: <workspace>/.deepdepcat/config.toml
        if let Some(ws) = workspace {
            let project_config_path = ws.join(".deepdepcat").join("config.toml");
            if project_config_path.exists() {
                match Self::load_file_json(&project_config_path) {
                    Ok(proj_json) => {
                        info!("Loaded project config from {:?}", project_config_path);
                        deep_merge_json(&mut config_json, &proj_json);
                    }
                    Err(e) => {
                        warn!(
                            "Failed to parse project config {:?}: {}",
                            project_config_path, e
                        );
                    }
                }
            }
        }

        // Deserialize merged JSON into typed config
        let mut config: Self = serde_json::from_value(config_json)
            .map_err(|e| AppError::Config(format!("Failed to deserialize merged config: {}", e)))?;

        // 3. Environment variable overrides
        config.apply_env_overrides();

        Ok(config)
    }

    /// Load a single TOML config file as raw JSON (preserving only explicitly set fields).
    fn load_file_json(path: &Path) -> AppResult<serde_json::Value> {
        let content = std::fs::read_to_string(path)?;
        let value: serde_json::Value = toml::from_str(&content)?;
        Ok(value)
    }

    /// Apply environment variable overrides.
    fn apply_env_overrides(&mut self) {
        // API key overrides
        for provider in &mut self.llm.providers {
            if !provider.api_key_env.is_empty() {
                if let Ok(key) = std::env::var(&provider.api_key_env) {
                    provider.api_key = Some(key);
                }
            }
        }

        // Default model/provider overrides
        if let Ok(model) = std::env::var("DEEPDEPCAT_MODEL") {
            self.app.default_model = model;
        }
        if let Ok(provider) = std::env::var("DEEPDEPCAT_PROVIDER") {
            self.app.default_provider = provider;
        }

        // Workspace override
        if let Ok(ws) = std::env::var("DEEPDEPCAT_WORKSPACE") {
            self.app.workspace = Some(PathBuf::from(ws));
        }

        // AMap web API key override
        if let Ok(key) = std::env::var("AMAP_WEB_KEY") {
            self.tools.amap_web_key = key;
        }

        // Telemetry override
        if let Ok(mode) = std::env::var("DEEPDEPCAT_TELEMETRY") {
            self.telemetry.mode = mode;
        }
    }

    /// Save the configuration to the user config file.
    ///
    /// Written atomically (tmp + rename, same pattern as the grant store):
    /// a crash/power loss mid-write used to truncate config.toml, and the
    /// next launch silently fell back to defaults — losing every setting
    /// with no error surface.
    pub fn save(&self, app_data_dir: &Path) -> AppResult<()> {
        let config_path = app_data_dir.join("config.toml");
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("Failed to serialize config: {}", e)))?;
        let tmp_path = config_path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, content)?;
        std::fs::rename(&tmp_path, &config_path)?;
        info!("Saved config to {:?}", config_path);
        Ok(())
    }
}

/// Recursively merge `override_val` into `base`. For objects, keys present in
/// `override_val` win; absent keys fall through to `base`. Non-object values
/// from `override_val` replace `base` entirely.
///
/// Public: cloud settings pull (sync_cmd) merges the remote blob into the
/// local config with the same semantics as config updates.
pub fn deep_merge_json(base: &mut serde_json::Value, override_val: &serde_json::Value) {
    match (base, override_val) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(override_map)) => {
            for (key, val) in override_map {
                match base_map.get_mut(key) {
                    Some(base_val) => deep_merge_json(base_val, val),
                    None => {
                        base_map.insert(key.clone(), val.clone());
                    }
                }
            }
        }
        (base, override_val) => {
            *base = override_val.clone();
        }
    }
}

/// Get the application data directory.
/// On Windows: %APPDATA%/deepdepcat
/// On macOS: ~/Library/Application Support/deepdepcat
/// On Linux: ~/.local/share/deepdepcat
///
/// Overridable via `DEEPDEPCAT_DATA_DIR` — the backend E2E test suite uses
/// this to isolate its database/config from the real app data.
pub fn get_app_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("DEEPDEPCAT_DATA_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = dirs::data_dir() {
        dir.join("deepdepcat")
    } else {
        PathBuf::from(".deepdepcat")
    }
}
