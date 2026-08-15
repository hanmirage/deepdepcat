//! Authentication commands — direct email+password login against the website
//! account system, plus token verify/revoke for session persistence.
//!
//! The desktop posts the user's credentials to the website's own `/api/auth/login`
//! (via reqwest, no CORS), then exchanges the resulting JWT for the account
//! identity. Registration (send-code → verify-email) also happens here so
//! users can create an account without leaving the app.

use serde::{Deserialize, Serialize};

/// Args for the registration step 1 (send verification code).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterSendCodeArgs {
    pub server_url: String,
    pub email: String,
    pub name: String,
    pub password: String,
}

/// Args for the registration step 2 (verify email + create account).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterVerifyEmailArgs {
    pub server_url: String,
    pub email: String,
    pub code: String,
}

/// Token response from the website login flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub user_id: String,
    pub username: String,
    /// Account avatar URL from the website ("" when unset).
    #[serde(default)]
    pub avatar: String,
}

/// Verify token response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyTokenResponse {
    pub valid: bool,
    pub user_id: Option<String>,
    pub expires_at: Option<f64>,
    /// Current avatar URL ("" when unset) — lets the client refresh it on
    /// startup without an extra round-trip.
    #[serde(default)]
    pub avatar: Option<String>,
}

/// Arguments for verify_token.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyTokenArgs {
    pub server_url: String,
    pub token: String,
}

/// Arguments for revoke_token.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeTokenArgs {
    pub server_url: String,
    pub token: String,
}

/// Arguments for login_with_password.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginWithPasswordArgs {
    pub server_url: String,
    pub email: String,
    pub password: String,
}

/// Arguments for update_user_profile (rename display name).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserProfileArgs {
    pub server_url: String,
    pub token: String,
    pub name: String,
}

/// Arguments for upload_avatar (multipart image upload).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadAvatarArgs {
    pub server_url: String,
    pub token: String,
    /// Local path of the image file to upload (JPG/PNG/WebP/GIF, ≤2MB).
    pub file_path: String,
}

/// Direct email+password login against the website account system.
///
/// Two steps (server-side zero changes):
/// 1. POST the website's own `/api/auth/login` with { email, password } →
///    returns the website JWT (`token`).
/// 2. POST `/api/v1/auth/web-session` with that JWT → resolves the account
///    identity (user_id/name/avatar) the desktop binds to.
///
/// The desktop calls the website over reqwest (not a browser), so there is
/// no CORS restriction. Registration stays on the website.
#[tauri::command]
pub async fn login_with_password(
    args: LoginWithPasswordArgs,
) -> Result<DeviceTokenResponse, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // Step 1: website login → website JWT.
    let login_url = format!("{}/api/auth/login", args.server_url);
    let resp = client
        .post(&login_url)
        .json(&serde_json::json!({ "email": args.email, "password": args.password }))
        .send()
        .await
        .map_err(|e| format!("Login request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // Classify the website's distinct failure statuses so the frontend can
        // show WHY login failed:
        //   401 = wrong password; 404 = user not found (Next.js maps
        //   USER_NOT_FOUND to 404, not 401) — both read as "bad credentials".
        //   403 = account disabled / email not verified; 429 = rate limited
        //   (5 per email, 20 per IP per 10 min) — each gets its own code so the
        //   UI can surface the server's message instead of a generic error.
        match status.as_u16() {
            401 | 404 => return Err("invalid_credentials".to_string()),
            403 => return Err("account_unavailable".to_string()),
            429 => return Err(format!("rate_limited:{body}")),
            _ => return Err(format!("HTTP {}: {}", status, body)),
        }
    }
    let login_json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse login response: {}", e))?;
    let website_token = login_json
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "Website login response missing token".to_string())?
        .to_string();

    // Step 2: exchange the website JWT for the account identity.
    let web_session_url = format!("{}/api/v1/auth/web-session", args.server_url);
    let resp = client
        .post(&web_session_url)
        .json(&serde_json::json!({ "token": website_token }))
        .send()
        .await
        .map_err(|e| format!("Session request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // 401 = the website JWT failed verification (key mismatch / expired) —
        // surface a classified error so the UI can show friendly copy.
        if status.as_u16() == 401 {
            return Err("invalid_session".to_string());
        }
        return Err(format!("Session HTTP {}: {}", status, body));
    }
    let session_json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse session response: {}", e))?;

    let user_id = session_json
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = session_json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let avatar = session_json
        .get("avatar")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(DeviceTokenResponse {
        // The website JWT IS the bearer token the desktop stores and later
        // verifies via /api/v1/auth/verify?token=... (which accepts website JWTs).
        access_token: website_token,
        token_type: "Bearer".to_string(),
        expires_in: 7 * 24 * 60 * 60, // 7 days (matches ACCESS_TOKEN_EXPIRE_MINUTES)
        user_id,
        username: name,
        avatar,
    })
}
#[tauri::command]
pub async fn verify_token(args: VerifyTokenArgs) -> Result<VerifyTokenResponse, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let url = format!(
        "{}/api/v1/auth/verify?token={}",
        args.server_url, args.token
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, body));
    }

    resp.json::<VerifyTokenResponse>()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))
}

/// Revoke a token.
#[tauri::command]
pub async fn revoke_token(args: RevokeTokenArgs) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let url = format!(
        "{}/api/v1/auth/revoke?token={}",
        args.server_url, args.token
    );

    let resp = client
        .post(&url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, body));
    }

    Ok(())
}

/// Get the default server URL (official website).
#[tauri::command]
pub async fn get_default_server_url() -> Result<String, String> {
    Ok("https://deepdepcat.hsmiai.xyz".to_string())
}

/// Update the display name on the website account (authority source).
///
/// Sends `Authorization: Bearer <website JWT>` to the website's
/// `/api/user/profile` (which writes data/users.json). The backend reads that
/// file directly, so the new name shows up across every consumer.
#[tauri::command]
pub async fn update_user_profile(args: UpdateUserProfileArgs) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let url = format!("{}/api/user/profile", args.server_url);
    let resp = client
        .patch(&url)
        .bearer_auth(&args.token)
        .json(&serde_json::json!({ "name": args.name }))
        .send()
        .await
        .map_err(|e| format!("Profile request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            return Err("invalid_session".to_string());
        }
        return Err(format!("HTTP {}: {}", status, body));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;
    Ok(json
        .get("user")
        .and_then(|u| u.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or(&args.name)
        .to_string())
}

/// Upload an avatar image to the website account.
///
/// Sends a multipart `file` field with `Authorization: Bearer <website JWT>` to
/// the website's `/api/user/avatar`. Returns the new avatar path
/// (e.g. `/uploads/avatars/...`).
#[tauri::command]
pub async fn upload_avatar(args: UploadAvatarArgs) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // The frontend sends the file path via the Tauri fs plugin. Read it from
    // disk and attach as a multipart part named "file".
    let bytes =
        std::fs::read(&args.file_path).map_err(|e| format!("Failed to read image file: {}", e))?;
    let file_name = std::path::Path::new(&args.file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("avatar.jpg")
        .to_string();

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(bytes).file_name(file_name),
    );

    let url = format!("{}/api/user/avatar", args.server_url);
    let resp = client
        .post(&url)
        .bearer_auth(&args.token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Avatar upload failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            return Err("invalid_session".to_string());
        }
        return Err(format!("HTTP {}: {}", status, body));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;
    json.get("avatar")
        .and_then(|a| a.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Avatar response missing 'avatar' field".to_string())
}

/// Registration step 1 — request a verification code by email.
///
/// Posts to the website's public `/api/auth/send-code`. Returns the server
/// message; when the website runs without SMTP it echoes the code in a
/// `debug` field (dev mode) — surfaced here for that case.
#[tauri::command]
pub async fn register_send_code(args: RegisterSendCodeArgs) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let url = format!("{}/api/auth/send-code", args.server_url);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "email": args.email,
            "name": args.name,
            "password": args.password,
        }))
        .send()
        .await
        .map_err(|e| format!("Send-code request failed: {}", e))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, body));
    }
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))?;
    let message = json
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("验证码已发送")
        .to_string();
    // Dev-mode fallback: the website echoes the code when SMTP is off.
    if let Some(debug) = json.get("debug").and_then(|d| d.as_str()) {
        return Ok(format!("{message}（开发模式验证码：{debug}）"));
    }
    Ok(message)
}

/// Registration step 2 — verify the email code and create the account.
///
/// Posts to the website's `/api/auth/verify-email`. Returns the created
/// user object so the caller can drop straight into login.
#[tauri::command]
pub async fn register_verify_email(
    args: RegisterVerifyEmailArgs,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let url = format!("{}/api/auth/verify-email", args.server_url);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "email": args.email,
            "code": args.code,
        }))
        .send()
        .await
        .map_err(|e| format!("Verify-email request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, body));
    }
    resp.json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))
}

// ── Token persistence (OS keyring) ───────────────────────────
//
// The website access token is a session credential — it must NOT live in
// localStorage where any renderer XSS could read it. These commands move it
// to the OS credential store (Windows Credential Manager / macOS Keychain /
// Linux Secret Service).

const KEYRING_SERVICE: &str = "deepdepcat-auth";
const KEYRING_USER: &str = "website-token";

/// Persist the website access token in the OS keyring.
#[tauri::command]
pub fn auth_store_token(token: String) -> Result<(), String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .and_then(|entry| entry.set_password(&token))
        .map_err(|e| format!("Failed to store auth token: {}", e))
}

/// Load the persisted access token from the OS keyring.
#[tauri::command]
pub fn auth_load_token() -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("Failed to open keyring: {}", e))?;
    match entry.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to read auth token: {}", e)),
    }
}

/// Remove the persisted access token from the OS keyring (logout).
#[tauri::command]
pub fn auth_delete_token() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("Failed to open keyring: {}", e))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete auth token: {}", e)),
    }
}
