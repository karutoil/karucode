//! The `Host` allowlist (DNS-rebinding defence) and the `Origin` comparison
//! used by the CSRF middleware.
//!
//! # Why the `Host` header must be checked
//!
//! Comparing `Origin` with `Host` alone does not stop DNS rebinding: a page on
//! `http://evil.example` whose DNS record is re-pointed at `127.0.0.1` sends
//! `Host: evil.example` *and* `Origin: http://evil.example` — both headers agree,
//! so an `Origin == Host` comparison accepts the request and the browser happily
//! `PATCH`es `/api/config`. The defence is therefore to validate `Host` against a
//! fixed allowlist, so a rebound name is rejected before the handler runs.
//!
//! # When the allowlist applies
//!
//! Only in the configuration that is exposed without a credential:
//!
//! * the process bound a loopback address (`--host 127.0.0.1`, the default), and
//! * no `KARUCODE_AUTH_TOKEN` is configured.
//!
//! With a bearer token the `Host` check is skipped and the `Origin == Host` rule
//! (case-insensitive) stays: the token is the credential and a rebinding page
//! cannot guess it. A non-loopback bind always requires a token, so the
//! loopback + no-token combination is exactly "no credential at all".
//!
//! # Allowed hosts
//!
//! * `127.0.0.1`, `localhost`, `::1` / `[::1]` — always, any port. The `Host`
//!   header's port is ignored for these (the browser uses the server's port).
//! * `KARUCODE_ALLOWED_HOSTS` extends the list, comma separated, for reverse
//!   proxies:
//!   - `karucode.example.com` — that host, any port,
//!   - `karucode.example.com:8443` — that host and port only,
//!   - `*.example.com` — any subdomain of `example.com` (**not** the bare
//!     `example.com`; list it separately if the apex is also used), optionally
//!     with a fixed port (`*.example.com:443`).
//!
//! Entries are matched case-insensitively. An entry that is not a valid
//! `host[:port]` / `*.domain[:port]` pattern is ignored with a warning: a typo
//! must not silently widen (or narrow) the allowlist.
//!
//! Values are rejected — not trimmed — when they contain whitespace or control
//! characters (`\r`, `\n`, `\t`, ...), when the authority is malformed, or when
//! the port is not a number in `1..=65535`. Rejecting beats normalising: any
//! transformation of an attacker-controlled header is a potential bypass.
//!
//! This is a request-routing guard, not an authentication mechanism. It is
//! bypassed by anything that can open a TCP connection to the port (a local
//! process, or a proxy that rewrites `Host`), which is why it only raises the
//! bar for browsers talking to a loopback-only, unauthenticated server.

use tracing::warn;

/// Environment variable that extends the `Host` allowlist.
pub const ALLOWED_HOSTS_ENV: &str = "KARUCODE_ALLOWED_HOSTS";

/// Hosts always accepted when the guard applies (the port is ignored).
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// A parsed `host[:port]`, normalised for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    /// Lowercase, brackets stripped, at most one trailing dot removed.
    pub host: String,
    pub port: Option<u16>,
}

/// One entry of the extra allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    /// `karucode.example.com` or `karucode.example.com:8443`.
    Exact { host: String, port: Option<u16> },
    /// `*.example.com`: any `*.`-prefixed suffix of `domain`.
    Suffix { domain: String, port: Option<u16> },
}

impl Rule {
    fn matches(&self, authority: &Authority) -> bool {
        match self {
            Rule::Exact { host, port } => {
                port_matches(*port, authority.port) && authority.host == *host
            }
            Rule::Suffix { domain, port } => {
                port_matches(*port, authority.port)
                    && authority
                        .host
                        .strip_suffix(domain.as_str())
                        .map(|prefix| prefix.ends_with('.'))
                        .unwrap_or(false)
            }
        }
    }
}

fn port_matches(expected: Option<u16>, actual: Option<u16>) -> bool {
    match expected {
        None => true,
        Some(expected) => actual == Some(expected),
    }
}

/// The parsed allowlist plus whether the process bound a loopback address.
#[derive(Debug, Clone)]
pub struct HostAllowlist {
    loopback_bind: bool,
    rules: Vec<Rule>,
    /// Human-readable list used in the 403 message and the startup log.
    description: String,
}

impl HostAllowlist {
    /// Build the allowlist from the configured bind host and the raw
    /// `KARUCODE_ALLOWED_HOSTS` value (production: [`Self::from_env`]; tests
    /// pass the value explicitly so they do not depend on process state).
    pub fn new(bind_host: &str, allowed_hosts: Option<&str>) -> Self {
        let loopback_bind = is_loopback_bind(bind_host);
        let mut rules = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for entry in allowed_hosts.unwrap_or_default().split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            match parse_rule(entry) {
                Some(rule) => {
                    rules.push(rule);
                    names.push(entry.to_ascii_lowercase());
                }
                None => warn!(
                    env = ALLOWED_HOSTS_ENV,
                    entry,
                    "ignoring an entry that is not a valid host, host:port or *.domain pattern"
                ),
            }
        }
        let mut description = String::from("127.0.0.1, localhost, [::1]");
        for name in &names {
            description.push_str(", ");
            description.push_str(name);
        }
        Self {
            loopback_bind,
            rules,
            description,
        }
    }

    /// True when the process is listening on a loopback address.
    pub fn bind_is_loopback(&self) -> bool {
        self.loopback_bind
    }

    /// The accepted hosts, for error messages and logs.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Whether the request's `host[:port]` is allowed. The caller has already
    /// decided that the guard applies (loopback bind, no auth token).
    pub fn allows(&self, authority: &Authority) -> bool {
        if is_loopback_host(&authority.host) {
            return true;
        }
        self.rules.iter().any(|rule| rule.matches(authority))
    }

    /// Full check for a raw header value: `Ok(())` when accepted, otherwise the
    /// 403 message naming the allowed hosts.
    pub fn check(&self, raw: &str) -> Result<(), String> {
        let Some(authority) = parse_authority(raw) else {
            return Err(format!(
                "rejected Host header: it is not a valid host[:port] value (whitespace, control \
                 characters and invalid ports are refused). This server only accepts Host {}",
                self.description
            ));
        };
        if self.allows(&authority) {
            return Ok(());
        }
        // `authority.host` is restricted to `[a-z0-9._:-]`, so it is safe to
        // echo back (no control characters can reach the JSON body).
        Err(format!(
            "unexpected Host header `{}`: this server is bound to a loopback address without \
             KARUCODE_AUTH_TOKEN, so it only accepts Host {} (add a reverse-proxy host with \
             {ALLOWED_HOSTS_ENV}, or configure KARUCODE_AUTH_TOKEN to disable the Host check)",
            match authority.port {
                Some(port) => format!("{}:{port}", authority.host),
                None => authority.host.clone(),
            },
            self.description
        ))
    }
}

/// Whether `host` (as configured for `--host`) is a loopback address.
pub fn is_loopback_bind(host: &str) -> bool {
    let host = host.trim();
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if matches!(bare, "localhost") {
        return true;
    }
    bare.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn is_loopback_host(host: &str) -> bool {
    LOOPBACK_HOSTS.contains(&host)
}

/// Parse a `Host`-header style authority. Returns `None` for anything that is
/// not exactly `host`, `host:port`, `[v6]`, `[v6]:port` or an unbracketed IPv6
/// literal; no trimming and no leniency.
pub fn parse_authority(raw: &str) -> Option<Authority> {
    if raw.is_empty() || raw.len() > 255 {
        return None;
    }
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (host, port) = split_port(raw)?;
    let host = normalize_host(host)?;
    Some(Authority { host, port })
}

/// `host[:port]` with IPv6 handling. `port` is `None` when absent.
fn split_port(raw: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = raw.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            suffix => Some(parse_port(suffix.strip_prefix(':')?)?),
        };
        return Some((inside, port));
    }
    // `::1` and friends: a bare IPv6 literal has no port.
    if raw.parse::<std::net::Ipv6Addr>().is_ok() {
        return Some((raw, None));
    }
    match raw.rsplit_once(':') {
        Some((host, port)) => Some((host, Some(parse_port(port)?))),
        None => Some((raw, None)),
    }
}

fn parse_port(raw: &str) -> Option<u16> {
    let port: u16 = raw.parse().ok()?;
    (port != 0).then_some(port)
}

/// Lowercase the host and drop one trailing dot (`localhost.` is `localhost`),
/// rejecting anything that is not `[a-z0-9._:-]`.
fn normalize_host(raw: &str) -> Option<String> {
    let trimmed = raw.strip_suffix('.').unwrap_or(raw);
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':'))
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// Parse one `KARUCODE_ALLOWED_HOSTS` entry.
fn parse_rule(entry: &str) -> Option<Rule> {
    // Wildcards are only meaningful as a leading `*.` (or the Django-style
    // `.example.com`), so split the port off the pattern first.
    let (pattern, port) = split_port(entry)?;
    if let Some(domain) = pattern
        .strip_prefix("*.")
        .or_else(|| pattern.strip_prefix('.'))
    {
        let domain = normalize_host(domain)?;
        if domain.contains('*') || domain.is_empty() {
            return None;
        }
        return Some(Rule::Suffix { domain, port });
    }
    if pattern.contains('*') {
        return None;
    }
    let host = normalize_host(pattern)?;
    Some(Rule::Exact { host, port })
}

/// Compare an `Origin` header with the request's `Host` header.
///
/// `Ok(true)` when they name the same authority. Malformed input (not an
/// absolute `http(s)://` URL, empty authority, control characters/whitespace,
/// invalid port) yields `Ok(false)`: the caller treats that as a rejected
/// request, never as "no Origin".
pub fn origin_matches_host(origin: &str, host: &str) -> bool {
    if origin.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    // `http://host:port/path` -> `host:port`; an `http://host` origin has no
    // path and therefore never matches a host that carries a port.
    let origin_authority = rest.split('/').next().unwrap_or_default();
    let Some(origin) = parse_authority(origin_authority) else {
        return false;
    };
    let Some(request_host) = parse_authority(host) else {
        return false;
    };
    origin == request_host
}

/// Neutralise a header value for logging: control characters are replaced and
/// the length is capped. Never used for matching.
pub fn display_host(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .take(64)
        .collect();
    format!("{cleaned:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(extra: Option<&str>) -> HostAllowlist {
        HostAllowlist::new("127.0.0.1", extra)
    }

    fn ok(allow: &HostAllowlist, raw: &str) -> bool {
        allow.check(raw).is_ok()
    }

    #[test]
    fn loopback_hosts_are_accepted_with_any_port() {
        let allow = allowlist(None);
        for host in [
            "127.0.0.1",
            "127.0.0.1:8787",
            "localhost",
            "localhost:8787",
            "LOCALHOST:8787",
            "[::1]",
            "[::1]:8787",
            "::1",
            "127.0.0.1.",
        ] {
            assert!(ok(&allow, host), "{host} should be allowed");
        }
    }

    #[test]
    fn rebound_hosts_are_rejected() {
        let allow = allowlist(None);
        for host in [
            "evil.example",
            "evil.example:8787",
            "127.0.0.1.evil.example",
            "localhost.evil.example",
            "0.0.0.0:8787",
            "192.168.1.10:8787",
            "[::ffff:127.0.0.1]:8787",
        ] {
            assert!(!ok(&allow, host), "{host} must be rejected");
        }
    }

    #[test]
    fn malformed_hosts_are_rejected() {
        let allow = allowlist(None);
        for host in [
            "",
            " ",
            "127.0.0.1 ",
            "127.0.0.1\r\nX-Evil: 1",
            "127.0.0.1\t",
            "127.0.0.1:",
            "127.0.0.1:0",
            "127.0.0.1:99999",
            "127.0.0.1:87a",
            "user@127.0.0.1",
            "127.0.0.1/path",
            "http://127.0.0.1",
            "[::1",
            "127.0.0.1%00",
        ] {
            let result = allow.check(host);
            assert!(result.is_err(), "{host:?} must be rejected");
            assert!(
                result.unwrap_err().contains("127.0.0.1, localhost, [::1]"),
                "the 403 message must name the allowed hosts"
            );
        }
    }

    #[test]
    fn extra_entries_extend_the_allowlist() {
        let allow = allowlist(Some("karucode.example.com, 10.0.0.5:9000"));
        assert!(ok(&allow, "karucode.example.com"));
        assert!(ok(&allow, "KARUCODE.EXAMPLE.COM:8443"));
        assert!(ok(&allow, "10.0.0.5:9000"));
        assert!(!ok(&allow, "10.0.0.5:9001"));
        assert!(!ok(&allow, "other.example.com"));
        assert!(allow.description().contains("karucode.example.com"));
    }

    #[test]
    fn wildcard_entries_match_subdomains_only() {
        let allow = allowlist(Some("*.example.com, *.secure.test:8443"));
        assert!(ok(&allow, "a.example.com"));
        assert!(ok(&allow, "a.b.example.com:8787"));
        assert!(!ok(&allow, "example.com"), "bare apex is not implied");
        assert!(!ok(&allow, "evilexample.com"));
        assert!(!ok(&allow, "a.example.com.evil.test"));
        assert!(ok(&allow, "app.secure.test:8443"));
        assert!(!ok(&allow, "app.secure.test:8444"));
    }

    #[test]
    fn invalid_extra_entries_are_ignored_not_widened() {
        let allow = allowlist(Some("*, *evil.com, bad host, , example.com:0"));
        assert!(!ok(&allow, "anything.example"));
        assert!(!ok(&allow, "example.com"));
        assert!(ok(&allow, "127.0.0.1"));
        assert!(!allow.description().contains('*'));
    }

    #[test]
    fn only_a_loopback_bind_enables_the_guard() {
        assert!(HostAllowlist::new("127.0.0.1", None).bind_is_loopback());
        assert!(HostAllowlist::new("localhost", None).bind_is_loopback());
        assert!(HostAllowlist::new("::1", None).bind_is_loopback());
        assert!(HostAllowlist::new("[::1]", None).bind_is_loopback());
        assert!(!HostAllowlist::new("0.0.0.0", None).bind_is_loopback());
        assert!(!HostAllowlist::new("192.168.0.2", None).bind_is_loopback());
        assert!(!HostAllowlist::new("karucode.example.com", None).bind_is_loopback());
    }

    #[test]
    fn origins_are_compared_case_insensitively_and_never_trimmed() {
        assert!(origin_matches_host(
            "http://127.0.0.1:8787",
            "127.0.0.1:8787"
        ));
        assert!(origin_matches_host("https://Example.COM", "example.com"));
        assert!(origin_matches_host("http://[::1]:8787", "[::1]:8787"));
        assert!(origin_matches_host(
            "http://localhost:8787/x",
            "localhost:8787"
        ));
        assert!(!origin_matches_host("http://evil.com", "127.0.0.1:8787"));
        assert!(!origin_matches_host(
            "http://evil.example",
            "127.0.0.1:8787"
        ));
        assert!(!origin_matches_host("null", "127.0.0.1:8787"));
        assert!(!origin_matches_host("http://", "127.0.0.1"));
        assert!(!origin_matches_host("http://127.0.0.1:8787", "127.0.0.1"));
        assert!(!origin_matches_host(
            "http://127.0.0.1:8787 ",
            "127.0.0.1:8787"
        ));
        assert!(!origin_matches_host(
            "http://127.0.0.1:8787\r\nX: 1",
            "127.0.0.1:8787"
        ));
        assert!(!origin_matches_host("file://127.0.0.1", "127.0.0.1"));
    }

    #[test]
    fn display_escapes_control_characters() {
        let rendered = display_host("evil\r\n.example");
        assert!(!rendered.contains('\r') && !rendered.contains('\n'));
        assert!(rendered.starts_with('"') && rendered.ends_with('"'));
        assert!(display_host(&"a".repeat(200)).len() < 80);
    }
}
