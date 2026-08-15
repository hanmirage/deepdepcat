//! Hook registry — stores and matches hook definitions.

use crate::hooks::types::{HookDefinition, HookEvent};
use std::collections::HashMap;

/// The hook registry — maps events to hook definitions.
pub struct HookRegistry {
    /// Hooks keyed by event.
    hooks: HashMap<HookEvent, Vec<HookDefinition>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    /// Register a hook definition.
    pub fn register(&mut self, definition: HookDefinition) {
        let event = definition.event.clone();
        self.hooks.entry(event).or_default().push(definition);
    }

    /// Get all hooks for a given event.
    pub fn get_hooks(&self, event: &HookEvent) -> &[HookDefinition] {
        self.hooks.get(event).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Remove ALL hooks (used when reloading config from disk).
    pub fn clear_all(&mut self) {
        self.hooks.clear();
    }

    /// Remove every hook for `event` whose dedup key matches — used by
    /// `once` hooks after their single execution. Identical duplicates are
    /// deduplicated at execution time anyway, so key-based removal matches
    /// the runtime semantics.
    pub fn remove_hooks_by_key(&mut self, event: &HookEvent, key: &str) {
        if let Some(list) = self.hooks.get_mut(event) {
            list.retain(|h| h.dedup_key() != key);
        }
    }
}
