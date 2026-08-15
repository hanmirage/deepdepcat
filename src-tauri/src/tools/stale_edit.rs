//! Stale-edit guard — stops the agent from overwriting changes it has not
//! seen.
//!
//! Every successful `read_file` of a text file records an FNV-1a fingerprint
//! of the file bytes for the session. The write tools (`write_file`,
//! `edit_file`, `search_replace`, `apply_patch`) compare the file ON DISK at
//! write time against that fingerprint: a mismatch means the file changed
//! since the agent last saw it (the user edited it, a formatter/hook rewrote
//! it, ...) and the write is refused with a corrective hint. Overwriting
//! unseen changes is the single most common destructive agent bug.
//!
//! The fingerprint is refreshed after every successful write, so the agent's
//! own consecutive edits never trip the guard. Files the agent never read
//! are never guarded (no record = no check) — the model is free to create or
//! first-touch any file. `bash`-driven writes are untracked by design (the
//! agent explicitly told the shell what to do).
//!
//! Best-effort throughout: bookkeeping or a missing file must never break
//! the write path.

use crate::bootstrap::AppState;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// FNV-1a 64-bit — the same primitive the cache-shape diagnostic uses.
/// Collision resistance is more than sufficient here: this is a
/// changed/unchanged signal, not a security boundary.
pub(crate) fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash = FNV64_OFFSET;
    for &b in bytes {
        fnv64_step(&mut hash, b);
    }
    hash
}

/// FNV-1a 64-bit offset basis.
pub(crate) const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Fold one byte into an in-progress FNV-1a hash (streaming variant —
/// lets `read_file` hash a huge file without holding it in memory).
fn fnv64_step(hash: &mut u64, byte: u8) {
    *hash ^= u64::from(byte);
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
}

/// Streaming fold for callers that read a file in chunks.
pub(crate) fn fnv64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        fnv64_step(&mut hash, b);
    }
    hash
}

/// Normalize the guard key: the record is scoped per (session, workspace,
/// path) so sub-agents writing into a parent's workspace and parallel
/// workers in different workspaces never share (or collide on) a record.
/// The session stays the OUTER key so `cleanup_session` (state/mode.rs)
/// can drop a whole session's records with one `remove`.
fn guard_key(workspace: Option<&Path>, path: &Path) -> (PathBuf, PathBuf) {
    (
        workspace.map(Path::to_path_buf).unwrap_or_default(),
        path.to_path_buf(),
    )
}

/// Record the content the agent last saw (or wrote) for `path` in `session`.
/// Called by `read_file` after a successful text read and by the write tools
/// after a successful write — the record always reflects the agent's newest
/// knowledge of the file.
pub async fn record_seen(
    app: &AppHandle,
    session_id: &str,
    workspace: Option<&Path>,
    path: &Path,
    content: &[u8],
) {
    record_seen_hash(app, session_id, workspace, path, fnv64(content)).await;
}

/// Like [`record_seen`], but takes a precomputed fingerprint — the caller
/// may have hashed the file in a single streaming pass (bounded memory).
pub async fn record_seen_hash(
    app: &AppHandle,
    session_id: &str,
    workspace: Option<&Path>,
    path: &Path,
    hash: u64,
) {
    let state = app.state::<AppState>();
    let mut map = state.file_seen_hashes.lock().await;
    map.entry(session_id.to_string())
        .or_default()
        .insert(guard_key(workspace, path), hash);
}

/// Check whether the file changed on disk since the agent last saw it.
///
/// Returns the corrective hint when the file is STALE (the agent had seen it
/// before AND the disk content no longer matches); `None` when safe to write
/// or when the agent never saw the file (first touch in the session).
/// A missing file is NOT stale — the write tool reports its own error.
pub async fn check_stale(
    app: &AppHandle,
    session_id: &str,
    workspace: Option<&Path>,
    path: &Path,
) -> Option<String> {
    let state = app.state::<AppState>();
    let seen = {
        let map = state.file_seen_hashes.lock().await;
        map.get(session_id)
            .and_then(|m| m.get(&guard_key(workspace, path)))
            .copied()
    };
    let seen = seen?;
    let Ok(bytes) = std::fs::read(path) else {
        return None;
    };
    if fnv64(&bytes) == seen {
        return None;
    }
    Some(format!(
        "Stale-edit guard: '{}' changed on disk since you last read or wrote \
         it — writing now would overwrite changes you have never seen. \
         Re-read the file with read_file to see its current content, then \
         retry the edit.",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv64_is_deterministic_and_sensitive() {
        assert_eq!(fnv64(b"hello"), fnv64(b"hello"));
        assert_ne!(fnv64(b"hello"), fnv64(b"hello!"));
        assert_ne!(fnv64(b""), fnv64(b" "));
        assert_ne!(fnv64(b"a\nb"), fnv64(b"ab"), "line structure must matter");
    }

    #[test]
    fn fnv64_matches_known_fnv1a_offset_basis() {
        // Reference vectors for the FNV-1a 64-bit variant (offset basis
        // 0xcbf29ce484222325, prime 0x100000001b3).
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
