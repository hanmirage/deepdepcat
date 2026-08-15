//! Device heartbeat — anonymous online-presence reporting.
//!
//! Sends `{device_id, app_version, os, arch}` to
//! `POST /api/v1/devices/heartbeat` every 2 minutes so the admin's
//! "online devices" view can show live installs. The install id is a
//! random UUID persisted in the app data dir (NOT an account id).
//!
//! Privacy: honors the Settings → Privacy diagnostics toggle. When off,
//! no network request is ever made.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

/// Default interval between heartbeats.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(120);

fn device_id_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("device_id")
}

/// Load the persisted install id, or create and persist a fresh UUID.
pub fn load_or_create_device_id(app_data_dir: &Path) -> String {
    let path = device_id_path(app_data_dir);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let id = existing.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    let id = Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &id);
    id
}

/// Send one heartbeat. Best-effort: failures are debug-logged and dropped.
async fn send_heartbeat(device_id: &str, server_url: &str) {
    if !crate::observability::diagnostics::is_enabled() {
        return;
    }
    if server_url.is_empty() {
        return;
    }
    let base = server_url.trim_end_matches('/').to_string();
    let payload = serde_json::json!({
        "device_id": device_id,
        "app_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    });

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "Heartbeat: failed to build HTTP client");
            return;
        }
    };
    match client
        .post(format!("{base}/api/v1/devices/heartbeat"))
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => debug!("Device heartbeat sent"),
        Ok(resp) => debug!(status = %resp.status(), "Heartbeat rejected"),
        Err(e) => debug!(error = %e, "Heartbeat failed"),
    }
}

/// Spawn the background heartbeat loop. The server URL is resolved on every
/// tick from `server_url`, so it always follows the user's configured backend.
pub fn spawn_heartbeat_loop<F>(app_data_dir: PathBuf, interval: Duration, server_url: F)
where
    F: Fn() -> String + Send + Sync + 'static,
{
    let device_id = load_or_create_device_id(&app_data_dir);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            send_heartbeat(&device_id, &server_url()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_persisted_and_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let first = load_or_create_device_id(tmp.path());
        let second = load_or_create_device_id(tmp.path());
        assert_eq!(first, second, "install id must survive reloads");
        assert_eq!(first.len(), 36, "uuid v4 format");
        assert!(tmp.path().join("device_id").exists());
    }

    #[test]
    fn device_id_ignores_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("device_id"), "  \n ").unwrap();
        let id = load_or_create_device_id(tmp.path());
        assert_eq!(id.len(), 36);
    }
}
