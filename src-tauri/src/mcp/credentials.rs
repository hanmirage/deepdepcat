//! Persistent credential storage for MCP server OAuth tokens.
//!
//! Credentials are stored in `{app_data_dir}/mcp_credentials.json`, keyed by
//! `"{server_name}:{server_url}"`. This keeps MCP OAuth tokens isolated from
//! other auth state. Writes are atomic (temp file + rename) so concurrent
//! processes never observe a half-written file.
//!
//! At-rest encryption: on Windows the whole JSON blob is DPAPI-encrypted
//! before landing on disk (`{"enc":"<base64>"}`) — the tokens are tied to
//! the current Windows user and a copied file leaks nothing. Platforms
//! without OS-keychain wiring yet fall back to plaintext with a warning;
//! legacy plaintext files are read transparently and re-encrypted on the
//! next save.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// File name for the credential store inside the app data directory.
const CREDENTIALS_FILENAME: &str = "mcp_credentials.json";

/// An OAuth token credential for one MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCredential {
    /// OAuth access token.
    pub access_token: String,
    /// Token type (e.g. "Bearer").
    pub token_type: String,
    /// Access token expiry as RFC3339 timestamp.
    pub expires_at: Option<String>,
    /// Refresh token, when the server supports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// The server URL the credential was issued for.
    pub server_url: String,
    /// OAuth token endpoint used to refresh the access token. `None` = the
    /// credential is static (no auto-renewal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    /// OAuth client id sent with the refresh grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

/// On-disk credential store: `{app_data_dir}/mcp_credentials.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpCredentialStore {
    #[serde(flatten)]
    entries: BTreeMap<String, McpCredential>,
}

/// Encrypted envelope written to disk on Windows: the base64 DPAPI
/// ciphertext of the full JSON store.
#[derive(Debug, Serialize, Deserialize)]
struct EncryptedEnvelope {
    enc: String,
}

impl McpCredentialStore {
    /// Build the composite key for a credential entry.
    pub fn key(server_name: &str, server_url: &str) -> String {
        format!("{server_name}:{server_url}")
    }

    /// Load the credential store from the given directory.
    ///
    /// Returns an empty store if the file does not exist. Both the
    /// encrypted envelope (Windows) and the legacy plaintext JSON are
    /// accepted — the legacy form keeps working until the next save
    /// upgrades it in place.
    pub fn load_from(app_data_dir: &Path) -> std::io::Result<Self> {
        let path = app_data_dir.join(CREDENTIALS_FILENAME);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;

        // Encrypted envelope first.
        if let Ok(envelope) = serde_json::from_str::<EncryptedEnvelope>(&content) {
            if let Ok(cipher) = base64::engine::general_purpose::STANDARD.decode(&envelope.enc) {
                if let Some(plain) = crate::mcp::credential_crypto::decrypt(&cipher) {
                    return serde_json::from_slice(&plain).map_err(std::io::Error::other);
                }
            }
            return Err(std::io::Error::other(
                "credential store is encrypted but could not be decrypted (different user account?)",
            ));
        }

        // Legacy plaintext JSON — still readable (upgraded on next save).
        serde_json::from_str(&content).map_err(std::io::Error::other)
    }

    /// Save the credential store to the given directory (atomic write).
    ///
    /// On Windows the JSON blob is DPAPI-encrypted before writing; other
    /// platforms store plaintext (no OS keychain wired yet).
    pub fn save_to(&self, app_data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(app_data_dir)?;
        let path = app_data_dir.join(CREDENTIALS_FILENAME);
        let tmp = app_data_dir.join(format!("{CREDENTIALS_FILENAME}.tmp"));
        let plain = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let content = match crate::mcp::credential_crypto::encrypt(&plain) {
            Some(cipher) => {
                let envelope = EncryptedEnvelope {
                    enc: base64::engine::general_purpose::STANDARD.encode(&cipher),
                };
                serde_json::to_vec(&envelope).map_err(std::io::Error::other)?
            }
            None => {
                tracing::warn!(
                    "At-rest encryption unavailable on this platform — MCP credentials stored in plaintext"
                );
                plain
            }
        };
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Insert a credential and save atomically.
    pub fn insert_and_save(
        &mut self,
        server_name: &str,
        server_url: &str,
        cred: McpCredential,
        app_data_dir: &Path,
    ) -> std::io::Result<()> {
        self.entries
            .insert(Self::key(server_name, server_url), cred);
        self.save_to(app_data_dir)
    }

    /// Get a credential by server name and URL.
    pub fn get(&self, server_name: &str, server_url: &str) -> Option<&McpCredential> {
        self.entries.get(&Self::key(server_name, server_url))
    }

    /// Remove a credential entry.
    pub fn remove(&mut self, server_name: &str, server_url: &str) -> Option<McpCredential> {
        self.entries.remove(&Self::key(server_name, server_url))
    }

    /// Server names that have a stored credential (keys only, no tokens).
    pub fn server_names(&self) -> Vec<String> {
        self.entries
            .keys()
            .map(|k| {
                k.split_once(':')
                    .map(|(n, _)| n.to_string())
                    .unwrap_or_else(|| k.clone())
            })
            .collect()
    }

    /// Auto-refresh every EXPIRED credential that carries a refresh token
    /// and a token endpoint (OAuth2 refresh grant). Persists once when
    /// anything changed. Returns true when at least one credential was
    /// refreshed.
    pub async fn refresh_expired(&mut self, app_data_dir: &Path) -> std::io::Result<bool> {
        let now = chrono::Utc::now();
        let http = reqwest::Client::new();
        let mut refreshed = false;
        let mut updated: Vec<(String, McpCredential)> = Vec::new();

        for (key, cred) in self.entries.iter() {
            let expired = cred
                .expires_at
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t.with_timezone(&chrono::Utc) <= now);
            let Some(refresh_token) = cred.refresh_token.clone() else {
                continue;
            };
            let Some(token_endpoint) = cred.token_endpoint.clone() else {
                continue;
            };
            if !expired {
                continue;
            }

            let mut form = vec![
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
            ];
            if let Some(ref client_id) = cred.client_id {
                form.push(("client_id", client_id));
            }
            let response = match http.post(&token_endpoint).form(&form).send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "MCP credential refresh request failed");
                    continue;
                }
            };
            if !response.status().is_success() {
                tracing::warn!(
                    status = %response.status(),
                    "MCP credential refresh rejected — keeping the old token"
                );
                continue;
            }
            let Ok(json) = response.json::<serde_json::Value>().await else {
                continue;
            };
            let Some(access_token) = json.get("access_token").and_then(|t| t.as_str()) else {
                continue;
            };
            let mut fresh = cred.clone();
            fresh.access_token = access_token.to_string();
            if let Some(expires_in) = json.get("expires_in").and_then(|e| e.as_u64()) {
                fresh.expires_at = Some(
                    (chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64))
                        .to_rfc3339(),
                );
            }
            if let Some(new_refresh) = json.get("refresh_token").and_then(|r| r.as_str()) {
                fresh.refresh_token = Some(new_refresh.to_string());
            }
            updated.push((key.clone(), fresh));
            refreshed = true;
        }

        if refreshed {
            for (key, cred) in updated {
                self.entries.insert(key, cred);
            }
            self.save_to(app_data_dir)?;
        }
        Ok(refreshed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn key_is_composite() {
        assert_eq!(McpCredentialStore::key("a", "b"), "a:b");
        assert_ne!(
            McpCredentialStore::key("a", "b"),
            McpCredentialStore::key("a", "c")
        );
    }

    fn expired_credential(token_endpoint: Option<String>) -> McpCredential {
        McpCredential {
            access_token: "old".into(),
            token_type: "Bearer".into(),
            expires_at: Some((chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339()),
            refresh_token: Some("old_refresh".into()),
            server_url: "http://srv".into(),
            token_endpoint,
            client_id: Some("client-1".into()),
        }
    }

    #[tokio::test]
    async fn refresh_skips_without_token_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = McpCredentialStore::default();
        store.entries.insert(
            McpCredentialStore::key("srv", "http://srv"),
            expired_credential(None),
        );

        let refreshed = store.refresh_expired(dir.path()).await.unwrap();
        assert!(!refreshed, "no token endpoint → nothing to refresh");
        assert_eq!(store.get("srv", "http://srv").unwrap().access_token, "old");
    }

    #[tokio::test]
    async fn refresh_skips_future_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = McpCredentialStore::default();
        let mut cred = expired_credential(Some("http://127.0.0.1:1/token".into()));
        cred.expires_at = Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
        store
            .entries
            .insert(McpCredentialStore::key("srv", "http://srv"), cred);

        let refreshed = store.refresh_expired(dir.path()).await.unwrap();
        assert!(!refreshed, "not expired yet → no refresh");
    }

    #[tokio::test]
    async fn refresh_uses_refresh_grant_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = McpCredentialStore::default();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request.contains("grant_type=refresh_token"));
            assert!(request.contains("refresh_token=old_refresh"));
            assert!(request.contains("client_id=client-1"));
            let body =
                r#"{"access_token":"new_token","expires_in":3600,"refresh_token":"new_refresh"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });

        store.entries.insert(
            McpCredentialStore::key("srv", "http://srv"),
            expired_credential(Some(format!("http://{addr}/token"))),
        );

        let refreshed = store.refresh_expired(dir.path()).await.unwrap();
        assert!(refreshed);
        server.await.unwrap();

        let cred = store.get("srv", "http://srv").unwrap();
        assert_eq!(cred.access_token, "new_token");
        assert_eq!(cred.refresh_token.as_deref(), Some("new_refresh"));

        let reloaded = McpCredentialStore::load_from(dir.path()).unwrap();
        assert_eq!(
            reloaded.get("srv", "http://srv").unwrap().access_token,
            "new_token",
            "refreshed credential must be persisted"
        );
    }
}
