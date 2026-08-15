//! SSRF protection for HTTP-type hooks.
//!
//! HTTP hooks POST to user-configured URLs. A malicious or misconfigured
//! URL must never reach internal infrastructure: private ranges, link-local
//! beyond loopback, and cloud metadata endpoints are rejected up front.
//! Loopback (`127.x` / `::1`) is allowed — local development servers are a
//! legitimate hook target. Hostnames are resolved and every address is
//! checked before connecting, which also catches common DNS-rebinding setups.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

/// Whether an IP address is considered internal (never a valid hook target).
///
/// Loopback is explicitly allowed for local development servers.
pub fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_internal_ipv4(v4),
        IpAddr::V6(v6) => is_internal_ipv6(v6),
    }
}

fn is_internal_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 127.0.0.0/8 — loopback is allowed for local dev servers.
    if octets[0] == 127 {
        return false;
    }
    // 0.0.0.0/8 — "this network"
    if octets[0] == 0 {
        return true;
    }
    // 10.0.0.0/8, 169.254.0.0/16, 192.168.0.0/16
    if octets[0] == 10
        || (octets[0] == 169 && octets[1] == 254)
        || (octets[0] == 192 && octets[1] == 168)
    {
        return true;
    }
    // 172.16.0.0/12
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }
    // 100.64.0.0/10 — CGNAT
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return true;
    }
    false
}

fn is_internal_ipv6(ip: &Ipv6Addr) -> bool {
    // ::1 loopback — allowed for local dev servers.
    if ip.is_loopback() {
        return false;
    }
    // ::/128 unspecified
    if ip.is_unspecified() {
        return true;
    }
    // fe80::/10 link-local
    if ip.octets()[0] == 0xfe && (ip.octets()[1] & 0xc0) == 0x80 {
        return true;
    }
    // fc00::/7 unique-local
    if (ip.octets()[0] & 0xfe) == 0xfc {
        return true;
    }
    // IPv4-mapped (::ffff:a.b.c.d)
    ip.to_ipv4_mapped().is_some_and(|v4| is_internal_ipv4(&v4))
}

/// Validate that a hook URL is safe to POST to.
///
/// Rejects non-http(s) schemes and any URL resolving to an internal IP.
/// Loopback is allowed — local development servers are a legitimate hook
/// target.
pub fn validate_hook_url(url: &str) -> Result<(), String> {
    validate_url(url, true)
}

/// Validate that a URL is safe for content-fetch tools (web_fetch).
///
/// Stricter than the hook variant: loopback is rejected too, because the
/// URL is supplied by the model rather than configured by the user — a
/// fetch must never reach the local machine's own services.
pub fn validate_fetch_url(url: &str) -> Result<(), String> {
    validate_url(url, false)
}

/// Shared validation core.
///
/// `allow_loopback` controls whether 127.0.0.0/8 / ::1 / localhost are
/// accepted as legitimate targets.
fn validate_url(url: &str, allow_loopback: bool) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("Unsupported URL scheme: {other}")),
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // Literal IP in the URL — check directly without DNS.
    if let Ok(ip) = host.parse::<IpAddr>() {
        check_addr(&ip, allow_loopback, host)?;
        return Ok(());
    }

    // Hostname — resolve and check every address. Refusing on any match
    // closes the DNS-rebinding window for multi-A/AAAA records.
    let addrs: Vec<IpAddr> = (host, parsed.port_or_known_default().unwrap_or(443))
        .to_socket_addrs()
        .map_err(|e| format!("Failed to resolve host {host}: {e}"))?
        .map(|sa| sa.ip())
        .collect();

    if addrs.is_empty() {
        return Err(format!("No addresses for host: {host}"));
    }
    for addr in &addrs {
        check_addr(addr, allow_loopback, host)?;
    }
    Ok(())
}

/// Reject internal addresses, and loopback when `allow_loopback` is false.
fn check_addr(ip: &IpAddr, allow_loopback: bool, host: &str) -> Result<(), String> {
    if is_internal_ip(ip) {
        return Err(format!("URL resolves to internal address: {ip}"));
    }
    if !allow_loopback && ip.is_loopback() {
        return Err(format!("URL resolves to loopback address: {ip} ({host})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ipv4_private_ranges() {
        for ip in [
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(is_internal_ip(&addr), "{ip} should be internal");
        }
    }

    #[test]
    fn accepts_public_and_loopback_ipv4() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "127.0.0.1",
            "127.0.0.2",
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(!is_internal_ip(&addr), "{ip} should be allowed");
        }
    }

    #[test]
    fn rejects_ipv6_internal_ranges() {
        for ip in [
            "::",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "::ffff:10.0.0.1",
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(is_internal_ip(&addr), "{ip} should be internal");
        }
    }

    #[test]
    fn accepts_ipv6_loopback() {
        let addr: IpAddr = "::1".parse().unwrap();
        assert!(!is_internal_ip(&addr), "::1 should be allowed");
    }

    #[test]
    fn url_with_internal_ip_is_rejected() {
        for url in [
            "https://169.254.169.254/latest/meta-data",
            "http://192.168.0.10/hook",
            "https://10.0.0.5/hook",
        ] {
            assert!(validate_hook_url(url).is_err(), "{url} should be rejected");
        }
    }

    #[test]
    fn url_with_public_or_loopback_ip_is_accepted() {
        for url in [
            "https://8.8.8.8/hook",
            "http://1.1.1.1:8080/hook",
            "http://127.0.0.1:9000/hook",
            "http://[::1]:9000/hook",
        ] {
            assert!(validate_hook_url(url).is_ok(), "{url} should be accepted");
        }
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(validate_hook_url("file:///etc/passwd").is_err());
        assert!(validate_hook_url("ftp://8.8.8.8/x").is_err());
        assert!(validate_hook_url("").is_err());
    }

    #[test]
    fn hostname_localhost_is_accepted() {
        assert!(validate_hook_url("http://localhost:9000/hook").is_ok());
    }

    #[test]
    fn fetch_variant_rejects_loopback() {
        for url in [
            "http://127.0.0.1:9000/page",
            "http://localhost:9000/page",
            "http://[::1]:9000/page",
        ] {
            assert!(
                validate_fetch_url(url).is_err(),
                "{url} should be rejected for fetch"
            );
        }
    }

    #[test]
    fn fetch_variant_still_accepts_public() {
        for url in [
            "https://8.8.8.8/x",
            "https://example.com/x",
            "https://deepdepcat.hsmiai.xyz/api/v1/health",
        ] {
            assert!(
                validate_fetch_url(url).is_ok(),
                "{url} should be allowed for fetch"
            );
        }
    }

    #[test]
    fn fetch_variant_rejects_internal() {
        assert!(validate_fetch_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(validate_fetch_url("http://192.168.0.10/x").is_err());
    }
}
