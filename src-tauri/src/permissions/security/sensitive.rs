//! Sensitive-file detection — the pre-edit guard for secret-bearing files.
//!
//! Mirrors the VS Code "sensitive files" preflight: editing a file that
//! likely holds credentials (.env, private keys, token stores) ALWAYS asks
//! for confirmation — even in auto-accept / accept-edits modes — so the
//! change is seen before it lands. Unlike the filesystem validator's hard
//! deny zones (~/.ssh, /etc/shadow), these files are legitimate edit
//! targets inside a project; they just never edit silently.

use serde_json::Value;

/// The write-target path argument of a write tool, when the call carries one.
///
/// Covers the core edit tools plus every Depwork file-writing tool. The
/// returned path is what the sensitive guard and the dispatcher's grant
/// short-circuit evaluate — a sensitive write must ALWAYS prompt and can
/// never be covered by a durable/session grant or a tool's self-approval.
pub fn sensitive_write_path(tool_name: &str, args: &Value) -> Option<String> {
    let key = match tool_name.to_ascii_lowercase().as_str() {
        "write_file" | "edit_file" | "search_replace" | "apply_patch" | "docx_edit"
        | "docx_generate" | "ppt_generate" | "xlsx_generate" | "pdf_generate"
        | "research_report" | "card_generate" | "citation_link" | "live_doc_write" => "path",
        "chart_generate" | "media_probe" | "media_convert" | "pdf_tools" => "output",
        "table_process" => "output_path",
        "content_pack" => "output_dir",
        _ => return None,
    };
    args.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// Whether this write call targets a sensitive (secret-bearing) file.
///
/// Used by the permission pipeline as a hard "always ask" gate: even
/// auto-accept modes, durable grants, session grants and Depwork's
/// self-approval must not silently modify these files.
pub fn is_sensitive_edit_call(tool_name: &str, args: &Value) -> bool {
    sensitive_write_path(tool_name, args)
        .map(|path| is_sensitive_path(&path))
        .unwrap_or(false)
}

/// Whether a path targets a sensitive (secret-bearing) file.
///
/// Matches on the FILE NAME (case-insensitive), so any project location
/// counts: `.env`, `.env.production`, `id_rsa`, `credentials.json`,
/// `secrets.yaml`, `*.pem`, `*.key`, `.git-credentials`, `.netrc`.
pub fn is_sensitive_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    let lower = file_name.to_ascii_lowercase();

    // Exact names.
    if matches!(
        lower.as_str(),
        ".env"
            | ".netrc"
            | ".git-credentials"
            | "id_rsa"
            | "id_ed25519"
            | "id_dsa"
            | "id_ecdsa"
            | "credentials"
            | "credentials.json"
            | "credential"
            | "secret"
            | "secrets"
    ) {
        return true;
    }
    // .env.* variants.
    if lower.starts_with(".env.") {
        return true;
    }
    // Extension-based (private keys, keystores).
    if lower.ends_with(".pem") || lower.ends_with(".key") || lower.ends_with(".pfx") {
        return true;
    }
    // credentials.* / secrets.* / token.* prefixes.
    if lower.starts_with("credentials.")
        || lower.starts_with("secrets.")
        || lower.starts_with("secret.")
        || lower.starts_with("token.")
        || lower.starts_with("tokens.")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_variants_match() {
        assert!(is_sensitive_path(".env"));
        assert!(is_sensitive_path(".env.production"));
        assert!(is_sensitive_path("src/.env.local"));
        assert!(is_sensitive_path(".ENV"));
        assert!(is_sensitive_path("config\\.env.development"));
    }

    #[test]
    fn key_files_match() {
        assert!(is_sensitive_path("keys/id_rsa"));
        assert!(is_sensitive_path("certs/server.pem"));
        assert!(is_sensitive_path("deploy/private.key"));
        assert!(is_sensitive_path("~/.ssh/id_ed25519"));
        assert!(is_sensitive_path("cert.pfx"));
    }

    #[test]
    fn credential_and_token_stores_match() {
        assert!(is_sensitive_path("credentials.json"));
        assert!(is_sensitive_path("secrets.yaml"));
        assert!(is_sensitive_path("deploy/tokens.json"));
        assert!(is_sensitive_path(".git-credentials"));
        assert!(is_sensitive_path(".netrc"));
        assert!(is_sensitive_path("auth/secret"));
    }

    #[test]
    fn ordinary_files_do_not_match() {
        assert!(!is_sensitive_path("src/main.rs"));
        assert!(!is_sensitive_path("README.md"));
        assert!(!is_sensitive_path("package.json"));
        assert!(!is_sensitive_path(".gitignore"));
        assert!(!is_sensitive_path("config.json"));
        assert!(!is_sensitive_path("src/keyboard.rs"));
        // A .env name in a longer identifier must not match.
        assert!(!is_sensitive_path("my.envfile"));
        assert!(!is_sensitive_path("env.example"));
    }

    #[test]
    fn sensitive_edit_call_covers_core_and_depwork_writers() {
        use serde_json::json;
        for tool in [
            "write_file",
            "edit_file",
            "search_replace",
            "apply_patch",
            "docx_generate",
            "ppt_generate",
            "xlsx_generate",
            "pdf_generate",
            "docx_edit",
            "research_report",
            "card_generate",
            "live_doc_write",
        ] {
            assert!(
                is_sensitive_edit_call(tool, &json!({ "path": "C:/x/.env" })),
                "{tool} path write to .env must be sensitive"
            );
            assert!(
                !is_sensitive_edit_call(tool, &json!({ "path": "C:/x/report.docx" })),
                "{tool} normal path must not be sensitive"
            );
        }
        assert!(is_sensitive_edit_call(
            "chart_generate",
            &json!({ "output": "a/id_rsa" })
        ));
        assert!(is_sensitive_edit_call(
            "media_convert",
            &json!({ "output": "a/secret.pem" })
        ));
        assert!(is_sensitive_edit_call(
            "pdf_tools",
            &json!({ "output": "a/token.txt" })
        ));
        assert!(is_sensitive_edit_call(
            "table_process",
            &json!({ "output_path": "a/.env" })
        ));
        assert!(is_sensitive_edit_call(
            "content_pack",
            &json!({ "output_dir": "a/.env" })
        ));
        // Non-write tools never trigger the guard.
        assert!(!is_sensitive_edit_call(
            "read_file",
            &json!({ "path": "a/.env" })
        ));
        assert!(!is_sensitive_edit_call(
            "doc_read",
            &json!({ "path": "a/.env" })
        ));
        assert!(!is_sensitive_edit_call(
            "bash",
            &json!({ "command": "cat .env" })
        ));
    }
}
