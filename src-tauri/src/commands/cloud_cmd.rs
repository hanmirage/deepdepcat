//! Cloud content commands — public website endpoints fetched from the Rust
//! side (the website API has no CORS configuration, so native HTTP only).
//!
//! - `/api/updates/changelog` — release history (public)
//! - `/api/site-config` — site info: latest version, download links,
//!   contact email, official links (public)

/// Fetch the release changelog from the website.
///
/// Returns the raw `{ updates: [...] }` payload — the frontend renders it
/// in the update/About panel. Best-effort; failures surface as errors the
/// UI can degrade gracefully from.
#[tauri::command]
pub async fn fetch_changelog(server_url: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let url = format!("{}/api/updates/changelog", server_url);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Changelog request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }
    resp.json()
        .await
        .map_err(|e| format!("Failed to parse changelog: {e}"))
}

/// Fetch the website site-config (latest version, download links, contact
/// info, official URLs).
#[tauri::command]
pub async fn fetch_site_config(server_url: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let url = format!("{}/api/site-config", server_url);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Site-config request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }
    resp.json()
        .await
        .map_err(|e| format!("Failed to parse site-config: {e}"))
}
