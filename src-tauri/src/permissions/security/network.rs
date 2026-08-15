//! Network command policy — command-level allowlist for the bash tool.
//!
//! Mode mirrors Codex's network policy shape but is enforced at the COMMAND
//! layer (we cannot intercept arbitrary process traffic on Windows without
//! an OS-level proxy): `block` denies every network primitive, `allowlist`
//! only permits matching domains, and `allow_all` (default) preserves the
//! historical behavior. Private/local targets are rejected when
//! `network_allow_private` is false. Obfuscated/encoded payloads are
//! already hard-denied before this layer runs.

use regex::Regex;
use std::net::IpAddr;
use std::str::FromStr;

/// Network primitives the policy can see at command level.
fn network_words() -> Regex {
    Regex::new(
        r"(?i)\b(curl(?:\.exe)?|wget|iwr|irm|invoke-webrequest|invoke-restmethod|nc|netcat|ssh|scp|sftp|ftp|telnet|powershell|pwsh)\b",
    )
    .unwrap()
}

/// Scheme-based host extraction: `https://host[:port]/...`.
fn scheme_hosts() -> Regex {
    Regex::new(r#"(?i)\bhttps?://(?:[^/@\s]+@)?([a-z0-9.-]+(?::[0-9]+)?)"#).unwrap()
}

/// `-Uri host` / `-Uri 'host'` (PowerShell).
fn uri_hosts() -> Regex {
    Regex::new(r#"(?i)-uri\s+['"]?([^'"\s]+)"#).unwrap()
}

/// Bare hosts after curl/wget when no scheme is present.
fn bare_hosts() -> Regex {
    Regex::new(r"(?i)\b(?:curl(?:\.exe)?|wget)\s+([a-z0-9.-]+(?:\.[a-z]{2,})?(?::[0-9]+)?)").unwrap()
}

/// Command-level network policy checker.
#[derive(Debug, Clone)]
pub struct NetworkPolicyChecker {
    mode: String,
    domains: Vec<String>,
    allow_private: bool,
}

impl NetworkPolicyChecker {
    pub fn new(mode: &str, domains: Vec<String>, allow_private: bool) -> Self {
        Self {
            mode: mode.to_string(),
            domains: domains
                .into_iter()
                .map(|d| d.trim().trim_start_matches("*.").to_lowercase())
                .filter(|d| !d.is_empty())
                .collect(),
            allow_private,
        }
    }

    /// Deny reason when the command violates the network policy.
    pub fn check(&self, command: &str) -> Option<String> {
        let hosts = extract_hosts(command);
        if hosts.is_empty() && !network_words().is_match(command) {
            return None;
        }

        match self.mode.as_str() {
            "block" => {
                return Some(
                    "网络策略：bash 网络访问已禁用（network_policy_mode=block）".to_string(),
                );
            }
            "allowlist" => {
                if hosts.is_empty() {
                    return Some("网络策略：未识别到可校验的目标域名，已拒绝".to_string());
                }
                for host in &hosts {
                    if is_private_host(host) && !self.allow_private {
                        return Some(format!(
                            "网络策略：目标 {host} 是私有/本机地址，已拒绝"
                        ));
                    }
                    if !self.domain_allowed(host) {
                        return Some(format!("网络策略：域名 {host} 不在允许列表"));
                    }
                }
            }
            _ => {
                // allow_all: only the private-target restriction applies.
                if !self.allow_private {
                    for host in &hosts {
                        if is_private_host(host) {
                            return Some(format!(
                                "网络策略：目标 {host} 是私有/本机地址，已拒绝"
                            ));
                        }
                    }
                }
            }
        }
        None
    }

    fn domain_allowed(&self, host: &str) -> bool {
        let host = host.to_lowercase();
        self.domains
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    }
}

/// Extract candidate hosts from a command (scheme URLs, `-Uri`, bare
/// curl/wget targets). Flags and obvious non-hosts are skipped.
fn extract_hosts(command: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut push = |h: &str| {
        // Strip scheme/userinfo if a full URL slipped in via -Uri.
        let mut h = h.trim();
        if let Some(rest) = h.strip_prefix("https://") {
            h = rest;
        } else if let Some(rest) = h.strip_prefix("http://") {
            h = rest;
        }
        if let Some(at) = h.rfind('@') {
            h = &h[at + 1..];
        }
        if let Some(idx) = h.find(['/', '?', '#']) {
            h = &h[..idx];
        }
        let h = h.trim_end_matches(['/', '\\', '.']);
        if h.is_empty()
            || h.starts_with('-')
            || h.starts_with("$(")
            || h == "localhost:"
        {
            return;
        }
        if !hosts.iter().any(|x: &String| x == h) {
            hosts.push(h.to_string());
        }
    };
    for cap in scheme_hosts().captures_iter(command) {
        if let Some(h) = cap.get(1) {
            push(h.as_str());
        }
    }
    for cap in uri_hosts().captures_iter(command) {
        if let Some(h) = cap.get(1) {
            push(h.as_str());
        }
    }
    for cap in bare_hosts().captures_iter(command) {
        if let Some(h) = cap.get(1) {
            let raw = h.as_str();
            // Skip flags and words that are clearly not hosts.
            if raw.contains('.') || raw.contains(':') || raw.eq_ignore_ascii_case("localhost") {
                push(raw);
            }
        }
    }
    hosts
}

/// Literal-IP private/local detection (hostnames are not resolved at the
/// command layer — documented limitation).
fn is_private_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    let name = host.split(':').next().unwrap_or(host);
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Ok(ip) = IpAddr::from_str(name) else {
        return false;
    };
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_mode_denies_all_network_primitives() {
        let p = NetworkPolicyChecker::new("block", vec![], true);
        assert!(p.check("curl https://example.com").is_some());
        assert!(p.check("Invoke-WebRequest https://example.com").is_some());
        assert!(p.check("git fetch").is_none(), "git is not a network primitive");
        assert!(p.check("npm test").is_none());
    }

    #[test]
    fn allowlist_matches_apex_and_subdomains() {
        let p = NetworkPolicyChecker::new(
            "allowlist",
            vec!["example.com".into(), "*.openai.com".into()],
            true,
        );
        assert!(p.check("curl https://example.com/x").is_none());
        assert!(p.check("curl https://api.example.com/x").is_none());
        assert!(p.check("curl https://api.openai.com/v1").is_none());
        assert!(p.check("curl https://evil.com/x").is_some());
    }

    #[test]
    fn private_targets_are_blocked_when_disabled() {
        let p = NetworkPolicyChecker::new("allow_all", vec![], false);
        assert!(p.check("curl http://127.0.0.1:8080/x").is_some());
        assert!(p.check("curl http://192.168.1.10/x").is_some());
        assert!(p.check("curl https://example.com/x").is_none());
        assert!(p.check("curl http://localhost:3000").is_some());
        // Default (allow_private=true) keeps localhost working.
        let open = NetworkPolicyChecker::new("allow_all", vec![], true);
        assert!(open.check("curl http://127.0.0.1:8080/x").is_none());
    }

    #[test]
    fn powershell_uri_and_bare_hosts_are_extracted() {
        let p = NetworkPolicyChecker::new("allowlist", vec!["example.com".into()], true);
        assert!(p.check("Invoke-WebRequest -Uri 'https://example.com/a'").is_none());
        assert!(p.check("curl example.com/path").is_none());
        assert!(p.check("curl evil.com/path").is_some());
    }
}
