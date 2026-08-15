//! Update commands — wrap the Tauri v2 updater plugin for frontend consumption.
//!
//! Two flows:
//! - Manual: `check_for_update` → user clicks → `download_and_install_update`
//! - Silent (backend-only releases): the client downloads in the background
//!   (`download_silent_update`) and the installer runs when the app exits
//!   (lib.rs ExitRequested hook) — the user never sees an update prompt.

use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;
use tracing::{info, warn};

/// Update metadata returned to the frontend when an update is available.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub date: Option<String>,
    pub body: Option<String>,
    /// True → backend-only release: download silently, install on exit.
    /// False → UI-driven manual download/install.
    pub silent: bool,
    /// Oldest supported client version. When set and the running client is
    /// older, the update is mandatory (force page).
    pub min_version: Option<String>,
    /// True when the update is mandatory (paired with min_version).
    pub force: bool,
}

/// Download progress event payload emitted during `download_and_install_update`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum UpdateProgress {
    Started,
    Progress {
        downloaded: u64,
        total: Option<u64>,
        fraction: f64,
    },
    Finished,
    Error {
        message: String,
    },
}

/// Directory (under app data) holding the pending silent-update package.
pub fn pending_update_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_default()
        .join("pending-update")
}

/// The staged silent-update package, if any: `(version, installer_path)`.
/// NSIS installers are `.exe` (the old MSI format is no longer produced).
pub fn pending_staged_version(app: &AppHandle) -> Option<(String, PathBuf)> {
    let dir = pending_update_dir(app);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(version) = name
            .strip_prefix("update-")
            .and_then(|n| n.strip_suffix(".exe"))
        {
            return Some((version.to_string(), path));
        }
    }
    None
}

/// Simple semver-ish compare: `a >= b` (missing components are 0, so
/// "2.0" == "2.0.0" and "2.0" < "2.0.1" — the old length tiebreak wrongly
/// ranked "2.0" below "2.0.0").
pub fn version_ge(a: &str, b: &str) -> bool {
    let num = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .filter(|p| !p.is_empty())
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (pa, pb) = (num(a), num(b));
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (
            pa.get(i).copied().unwrap_or(0),
            pb.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    true
}

/// Clean up a staged update once the running app is at (or beyond) that
/// version — i.e. the exit-install succeeded, or a newer release replaced it.
/// Called at startup so stale MSIs never accumulate.
pub fn cleanup_stale_pending(app: &AppHandle) {
    let Some((staged_version, _)) = pending_staged_version(app) else {
        return;
    };
    if version_ge(env!("CARGO_PKG_VERSION"), &staged_version) {
        tracing::info!(
            staged = %staged_version,
            current = env!("CARGO_PKG_VERSION"),
            "Removing stale staged update"
        );
        let dir = pending_update_dir(app);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Check for an available update via the Tauri updater plugin.
///
/// Returns `Ok(None)` when the client is already on the latest version.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;

    let update = updater
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;

    match update {
        Some(u) => {
            let min_version = u
                .raw_json
                .get("min_version")
                .and_then(|v| v.as_str())
                .map(String::from);
            // Mandatory only when the running client is below the release's
            // min_version floor (the server's `force` mirrors this).
            let force = min_version
                .as_deref()
                .is_some_and(|mv| !version_ge(&u.current_version, mv));
            info!(
                latest = %u.version,
                current = %u.current_version,
                silent = u.raw_json.get("silent").is_some_and(|v| v.as_bool().unwrap_or(false)),
                force,
                min_version = ?min_version,
                "Update available"
            );
            Ok(Some(UpdateInfo {
                version: u.version.clone(),
                current_version: u.current_version.clone(),
                date: u.date.map(|d| d.to_string()),
                body: u.body.clone(),
                silent: u
                    .raw_json
                    .get("silent")
                    .is_some_and(|v| v.as_bool().unwrap_or(false)),
                min_version,
                force,
            }))
        }
        None => {
            info!("No update available");
            Ok(None)
        }
    }
}

/// Download, verify, and install the update if one is available.
///
/// Emits `update-progress` events during download. After installation,
/// the caller should prompt the user to restart the application.
#[tauri::command]
pub async fn download_and_install_update(app: AppHandle) -> Result<bool, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;

    let update = updater
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;

    let update = match update {
        Some(u) => u,
        None => return Ok(false),
    };

    info!(version = %update.version, "Starting update download");

    let _ = app.emit("update-progress", UpdateProgress::Started);

    let app_for_progress = app.clone();
    let mut total_downloaded: u64 = 0;

    let result = update
        .download_and_install(
            move |chunk_size, content_length| {
                total_downloaded += chunk_size as u64;
                let fraction = content_length
                    .map(|t| {
                        if t > 0 {
                            total_downloaded as f64 / t as f64
                        } else {
                            0.0
                        }
                    })
                    .unwrap_or(0.0);

                let _ = app_for_progress.emit(
                    "update-progress",
                    UpdateProgress::Progress {
                        downloaded: total_downloaded,
                        total: content_length,
                        fraction,
                    },
                );
            },
            {
                let app = app.clone();
                move || {
                    let _ = app.emit("update-progress", UpdateProgress::Finished);
                }
            },
        )
        .await;

    match result {
        Ok(()) => {
            info!("Update installed successfully");
            Ok(true)
        }
        Err(e) => {
            warn!(error = %e, "Update installation failed");
            let _ = app.emit(
                "update-progress",
                UpdateProgress::Error {
                    message: e.to_string(),
                },
            );
            Err(format!("Update installation failed: {e}"))
        }
    }
}

/// Silent update flow — download (with resume + signature verification,
/// mirroring the updater plugin's own minisign checks) and stage the
/// installer for the next app exit. No UI is shown; the install happens in
/// the ExitRequested hook.
///
/// Download robustness (user-reported "stuck downloading"): the plugin's
/// `download()` has NO timeout — a TCP half-open (common behind Chinese
/// residential networks / proxies) hangs forever. We download ourselves
/// with per-chunk timeouts and resume-from-offset retries against the
/// server's Range support, then verify the minisign signature with the
/// same public key the plugin uses.
///
/// Returns `Ok(None)` when nothing to do, or `Some(version)` once staged.
#[tauri::command]
pub async fn download_silent_update(app: AppHandle) -> Result<Option<String>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;

    let update = match updater
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?
    {
        Some(u)
            if u.raw_json
                .get("silent")
                .is_some_and(|v| v.as_bool().unwrap_or(false)) =>
        {
            u
        }
        _ => return Ok(None),
    };

    info!(version = %update.version, "Silent update download started");

    // Stage: <app_data>/pending-update/update-<version>.exe (+ .part while
    // downloading, so a restart of the download resumes from the offset).
    // NSIS installers are .exe; the old MSI format is no longer produced.
    let dir = pending_update_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part_path = dir.join(format!("update-{}.exe.part", update.version));
    let staged = dir.join(format!("update-{}.exe", update.version));

    // The plugin resolves the same pubkey from tauri.conf.json
    // (`plugins.updater.pubkey`, held as raw JSON in the config map).
    let pubkey = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let version = update.version.clone();

    let bytes =
        download_resumable(&update.download_url, &part_path, &pubkey, &update.signature).await?;

    // The download is verified — stage it and drop the part file.
    std::fs::write(&staged, &bytes).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&part_path);
    info!(version = %version, path = %staged.display(), "Silent update staged for next exit");

    Ok(Some(version))
}

/// Download `url` into `part`, resuming from the part file's current size
/// (server supports Range — verified on the release server). Per-chunk
/// stall timeout prevents the half-open-connection hang; interruptions
/// retry from the last offset; the final buffer is minisign-verified.
async fn download_resumable(
    url: &reqwest::Url,
    part: &std::path::Path,
    pubkey: &str,
    signature: &str,
) -> Result<Vec<u8>, String> {
    use futures_util::StreamExt;
    use std::io::Write;

    const MAX_ATTEMPTS: u32 = 6;
    const CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
    const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client build failed: {e}"))?;

    for attempt in 1..=MAX_ATTEMPTS {
        let offset = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        let mut req = client.get(url.clone());
        if offset > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Download request failed: {e}"))?;
        let status = resp.status();

        if offset > 0 && status == reqwest::StatusCode::OK {
            // Server ignored Range (no resume support) — restart from zero.
            let _ = std::fs::remove_file(part);
            info!(attempt, "Server ignored Range — restarting download");
            continue;
        }
        if !status.is_success() {
            return Err(format!("Download failed with status: {status}"));
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(part)
            .map_err(|e| e.to_string())?;
        let mut stream = resp.bytes_stream();

        let mut stalled = false;
        loop {
            let next = tokio::time::timeout(CHUNK_TIMEOUT, stream.next()).await;
            match next {
                Ok(Some(Ok(chunk))) => {
                    file.write_all(&chunk).map_err(|e| e.to_string())?;
                }
                Ok(Some(Err(e))) => {
                    warn!(attempt, error = %e, "Download stream interrupted — will resume");
                    // A mid-stream error is an interruption like a stall:
                    // the part file must be kept for resume. Without this
                    // flag the partial download would fall through to
                    // signature verification, fail, and be DELETED — losing
                    // the resume offset (and falsely flagging the package).
                    stalled = true;
                    break;
                }
                Ok(None) => break, // Stream finished cleanly.
                Err(_) => {
                    warn!(attempt, "Download stalled 90s — will resume from offset");
                    stalled = true;
                    break;
                }
            }
        }
        let _ = file.flush();
        drop(file);

        // Interrupted — retry from the persisted offset (attempts will run
        // out eventually, but the part file stays for the next invocation).
        if stalled {
            continue;
        }

        // Stream complete — verify the minisign signature before staging.
        let bytes = std::fs::read(part).map_err(|e| e.to_string())?;
        match verify_minisign(pubkey, signature, &bytes) {
            Ok(true) => return Ok(bytes),
            Ok(false) | Err(_) => {
                // Corrupted payload — delete and report; a fresh attempt
                // (or the next hourly check) restarts cleanly.
                let _ = std::fs::remove_file(part);
                return Err(
                    "Downloaded package failed signature verification — retrying later".into(),
                );
            }
        }
    }

    Err("Download failed after 6 attempts — will retry on the next check".into())
}

/// Verify a downloaded buffer against the updater's minisign public key —
/// the exact same steps the updater plugin runs in `verify_signature`.
fn verify_minisign(pubkey: &str, signature: &str, data: &[u8]) -> Result<bool, String> {
    use base64::Engine;

    let pk_raw = base64::engine::general_purpose::STANDARD
        .decode(pubkey)
        .map_err(|e| format!("Bad pubkey encoding: {e}"))?;
    let pk_text = std::str::from_utf8(&pk_raw).map_err(|_| "Pubkey is not UTF-8".to_string())?;
    let public_key =
        minisign_verify::PublicKey::decode(pk_text).map_err(|e| format!("Bad pubkey: {e}"))?;

    let sig_raw = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|e| format!("Bad signature encoding: {e}"))?;
    let sig_text =
        std::str::from_utf8(&sig_raw).map_err(|_| "Signature is not UTF-8".to_string())?;
    let signature =
        minisign_verify::Signature::decode(sig_text).map_err(|e| format!("Bad signature: {e}"))?;

    public_key
        .verify(data, &signature, true)
        .map_err(|e| format!("Signature verification failed: {e}"))?;
    Ok(true)
}

/// Whether a silent update is staged for the next app exit.
#[tauri::command]
pub fn has_pending_silent_update(app: AppHandle) -> Result<Option<String>, String> {
    Ok(pending_staged_version(&app).map(|(v, _)| v))
}

/// Relaunch the application (used after a mandatory update installs so the
/// user stops running the old, unsupported version).
#[tauri::command]
pub fn relaunch_app(app: AppHandle) -> Result<(), String> {
    app.restart()
}

/// Clean up staged silent-update files (called at startup once the app runs
/// at or beyond the staged version).
#[tauri::command]
pub fn clear_pending_silent_update(app: AppHandle) -> Result<(), String> {
    let dir = pending_update_dir(&app);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end signature verification against the REAL signing key and a
    /// REAL built MSI (target/release/bundle/msi). Skipped silently when no
    /// release build exists yet (unit-test runs pre-build are fine).
    #[test]
    fn verifies_real_release_signature() {
        let nsis_dir = std::path::Path::new("target/release/bundle/nsis");
        let Ok(entries) = std::fs::read_dir(nsis_dir) else {
            return; // no release build — skip
        };
        let setup = entries.flatten().map(|e| e.path()).find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("DeepDepCat_0.1.8") && n.ends_with(".exe"))
        });
        let Some(setup) = setup else {
            return; // 0.1.8 not built — skip
        };
        let sig_path = setup.with_extension("exe.sig");
        let (bytes, sig) = (
            std::fs::read(&setup).unwrap(),
            std::fs::read_to_string(&sig_path).unwrap(),
        );
        let pubkey = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDU2REMxRTBDMTJFMDBDMQpSV1RCQUM3QjRNRnRCZGpHQTRrMXBaWlNYdi9PVmNqbm9wRnllcVhIeTE5amRnbllqTDZla1Irago=";
        assert!(
            verify_minisign(pubkey, sig.trim(), &bytes).unwrap(),
            "real NSIS setup must verify"
        );
    }

    #[test]
    fn rejects_tampered_payload() {
        let pubkey = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDU2REMxRTBDMTJFMDBDMQpSV1RCQUM3QjRNRnRCZGpHQTRrMXBaWlNYdi9PVmNqbm9wRnllcVhIeTE5amRnbllqTDZla1Irago=";
        // A valid signature from the 0.1.8 release, against tampered bytes.
        let sig = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUQkFDN0I0TUZ0QlVURlBMb2NNcklLeGhDeDBWUGJvUER2dlBML1F0V3QwOHdUQ2srcWsvY2xOUXo0ZFlOZjE0eEM0ZHgwaS81RmtGYVcxOHg5d3ZlMEJHQzJJS0VlRlE4PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg1Njc5NzEyCWZpbGU6RGVlcERlcENhdF8wLjEuOF94NjRfZW4tVVMubXNpClU5b3ZEV0RNbmVuc0JKbzVyTU1hbEZyazNFOFFxdnM4cDhJV0dHQXB3RVNBV2pJWitMR01GV2l6dURkcEhBZFRpSlU1YkV6c1N4UHRyZ0Y5bGo1YkR3PT0K";
        let tampered = vec![0u8; 64];
        let result = verify_minisign(pubkey, sig, &tampered);
        assert!(result.is_err(), "tampered bytes must not verify");
    }
}
