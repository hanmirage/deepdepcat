//! Config commands — get and set configuration.

use crate::core::config::AppConfig;
use crate::bootstrap::AppState;
use serde_json::Value;
use tauri::State;

/// Get the full configuration.
#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<Value, String> {
    let config = state.config().map_err(|e| e.to_string())?;
    serde_json::to_value(&*config).map_err(|e| e.to_string())
}

/// Update configuration.
#[tauri::command]
pub async fn update_config(new_config: Value, state: State<'_, AppState>) -> Result<(), String> {
    let config: AppConfig = serde_json::from_value(new_config).map_err(|e| e.to_string())?;
    config
        .save(&state.app_data_dir)
        .map_err(|e| e.to_string())?;

    // Read the weights before `config` is moved into the in-memory store.
    let weights = crate::memory::search::SearchWeights {
        bm25: config.memory.search_weight_bm25,
        cosine: config.memory.search_weight_cosine,
        recency: config.memory.search_weight_recency,
    };
    let temperature = config.memory.search_recency_temperature;

    // Update in-memory config
    {
        let mut current = state.config_write().map_err(|e| e.to_string())?;
        *current = config;
    }
    // Hot-apply provider changes (API keys, base URLs) to the shared LLM
    // client so running agents and subagents pick them up immediately.
    state.refresh_llm_providers();

    // Hot-apply memory search weights so the UI sliders take effect
    // immediately (no restart needed). Both the searcher and the
    // auto-injection searcher must see the new weights.
    state.memory_searcher.update_weights(weights.clone());
    state.memory_injector.update_weights(weights);
    state.memory_searcher.set_recency_temperature(temperature);
    state
        .memory_injector
        .searcher()
        .set_recency_temperature(temperature);

    Ok(())
}
