//! Hook trust store — hash-based trust for hook definitions.
//!
//! Industry model (Codex 2026): before a non-managed hook runs, its
//! definition must be reviewed and trusted; trust is recorded against the
//! CURRENT content hash, so any edit to the hook invalidates the trust and
//! requires a fresh review. Untrusted hooks are SKIPPED (never executed)
//! — for blocking events that means the operation proceeds without the
//! hook's gate (fail-open, matching "skipped until trusted" semantics).

use crate::hooks::types::HookDefinition;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tracing::warn;

/// Trust store file inside the app data dir.
const TRUST_FILE: &str = "hook-trust.json";

/// Content fingerprint of a hook definition — any field change (command,
/// condition, shell, timeout, payload) changes the fingerprint and revokes
/// the trust.
pub fn fingerprint(hook: &HookDefinition) -> String {
    let content = match &hook.hook_type {
        crate::hooks::types::HookType::Command => hook.command.as_deref().unwrap_or(""),
        crate::hooks::types::HookType::Prompt | crate::hooks::types::HookType::Agent => {
            hook.prompt.as_deref().unwrap_or("")
        }
        crate::hooks::types::HookType::Http => hook.url.as_deref().unwrap_or(""),
    };
    let mut hasher = Sha256::new();
    hasher.update(hook.event.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(format!("{:?}", hook.hook_type).as_bytes());
    hasher.update([0u8]);
    hasher.update(content.as_bytes());
    hasher.update([0u8]);
    hasher.update(hook.condition.as_deref().unwrap_or("").as_bytes());
    hasher.update([0u8]);
    hasher.update(hook.shell.as_deref().unwrap_or("").as_bytes());
    hasher.update([0u8]);
    hasher.update(
        hook.timeout_ms
            .map(|t| t.to_string())
            .unwrap_or_default()
            .as_bytes(),
    );
    hex_encode(&hasher.finalize())
}

/// Persistent, hash-keyed hook trust store.
#[derive(Debug, Default)]
pub struct HookTrustStore {
    trusted: RwLock<HashSet<String>>,
    path: Option<PathBuf>,
}

impl HookTrustStore {
    /// Load from disk (missing/corrupt file = empty store).
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join(TRUST_FILE);
        let trusted = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .map(|list| list.into_iter().collect())
            .unwrap_or_default();
        Self {
            trusted: RwLock::new(trusted),
            path: Some(path),
        }
    }

    /// Whether a hook definition is trusted (its current fingerprint is in
    /// the store).
    pub fn is_trusted(&self, hook: &HookDefinition) -> bool {
        self.trusted
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&fingerprint(hook))
    }

    /// Trust a fingerprint and persist (best-effort).
    pub fn trust(&self, fp: &str) {
        self.trusted
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fp.to_string());
        self.persist();
    }

    /// Revoke trust for a fingerprint and persist (best-effort).
    pub fn untrust(&self, fp: &str) {
        self.trusted
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(fp);
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let list: Vec<String> = self
            .trusted
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();
        let tmp = path.with_extension("json.tmp");
        let ok = std::fs::write(&tmp, serde_json::to_string_pretty(&list).unwrap_or_default())
            .and_then(|()| std::fs::rename(&tmp, path))
            .is_ok();
        if !ok {
            warn!(path = %path.display(), "Failed to persist hook trust store");
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::types::{HookEvent, HookType};

    fn hook(command: &str) -> HookDefinition {
        HookDefinition {
            event: HookEvent::PreToolUse,
            hook_type: HookType::Command,
            command: Some(command.to_string()),
            prompt: None,
            url: None,
            condition: None,
            timeout_ms: Some(5000),
            shell: None,
            async_hook: false,
            async_rewake: false,
            once: false,
            enabled: true,
        }
    }

    #[test]
    fn fingerprint_changes_when_content_changes() {
        assert_ne!(fingerprint(&hook("echo a")), fingerprint(&hook("echo b")));
        assert_eq!(fingerprint(&hook("echo a")), fingerprint(&hook("echo a")));
    }

    #[test]
    fn fingerprint_changes_when_condition_or_timeout_change() {
        let mut a = hook("echo a");
        a.condition = Some("bash".to_string());
        let mut b = hook("echo a");
        b.condition = Some("edit_file".to_string());
        assert_ne!(fingerprint(&a), fingerprint(&b));

        let mut c = hook("echo a");
        c.timeout_ms = Some(6000);
        assert_ne!(fingerprint(&a), fingerprint(&c));
    }

    #[test]
    fn trust_store_roundtrip_and_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let store = HookTrustStore::load(dir.path());
        let h = hook("echo trusted");
        let fp = fingerprint(&h);
        assert!(!store.is_trusted(&h));
        store.trust(&fp);
        assert!(store.is_trusted(&h));

        // Reload from disk — trust survives.
        let reloaded = HookTrustStore::load(dir.path());
        assert!(reloaded.is_trusted(&h));

        let revoke = HookTrustStore::load(dir.path());
        revoke.untrust(&fp);
        assert!(!revoke.is_trusted(&h));
    }

    #[test]
    fn editing_hook_revokes_trust() {
        let dir = tempfile::tempdir().unwrap();
        let store = HookTrustStore::load(dir.path());
        let original = hook("echo v1");
        store.trust(&fingerprint(&original));
        assert!(store.is_trusted(&original));
        let edited = hook("echo v2");
        assert!(
            !store.is_trusted(&edited),
            "edited hooks must require a fresh review"
        );
    }
}
