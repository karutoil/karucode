//! Which peer wrote a request, and which forwarded headers may be believed.
//!
//! `X-Forwarded-For` is attacker-controlled unless the socket's other end is a
//! proxy the operator named: any client can send `X-Forwarded-For: 127.0.0.1`.
//! So every decision here is gated on the **direct peer** first:
//!
//! 1. Is the peer inside the trusted set? Loopback always is (that covers
//!    `cloudflared`, a local nginx, a Docker bridge gateway); `tunnel =
//!    "cloudflare"` adds Cloudflare's published ranges; `trusted_proxies` adds
//!    anything else the operator names.
//! 2. Only then is a forwarded header read. Otherwise the peer *is* the client.
//!
//! The header is then walked **from the right**: each proxy appends the address
//! it received the request from, so the right-most entries are the most recently
//! added ones and the left-most value is whatever the client chose to write. The
//! client is the first address from the right that is not itself trusted.
//!
//! `X-Forwarded-Proto` matters for one thing here: whether a cookie deserves
//! `Secure`. This binary does not terminate TLS, so "https" only ever comes from
//! a trusted proxy in front of it.

use crate::host::{parse_authority, Authority};
use axum::http::HeaderMap;
use kc_core::cidr::{CidrRange, CidrSet};
use kc_core::config::AccessConfig;
use std::net::IpAddr;

/// What the request looks like from the server's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientInfo {
    /// The client address, when it can be determined at all.
    pub ip: Option<IpAddr>,
    /// Whether the request reached the browser over TLS (always via a proxy).
    pub https: bool,
}

/// The trust policy for one running server, built once from `[access]`.
#[derive(Debug, Clone)]
pub struct ProxyTrust {
    /// Trusted proxies. Empty means nobody (the peer is the client).
    ranges: CidrSet,
    /// Whether forwarded headers are consulted at all.
    enabled: bool,
    /// The configured public URL, when there is one.
    public_authority: Option<String>,
}

impl ProxyTrust {
    /// Build from the `[access]` section.
    ///
    /// A bad entry cannot reach here in a validated config - `AccessConfig::validate`
    /// rejects it at startup and on save - but the error is propagated rather than
    /// swallowed so a caller that skipped validation does not silently run with a
    /// trust set that matches nothing.
    pub fn from_access(access: &AccessConfig) -> kc_core::Result<Self> {
        let ranges = access.trusted_proxy_ranges()?;
        Ok(Self {
            ranges: CidrSet::new(ranges),
            enabled: access.trusts_forwarded_headers(),
            public_authority: access.public_authority(),
        })
    }

    /// A policy that trusts nobody: the peer is the client.
    pub fn disabled() -> Self {
        Self {
            ranges: CidrSet::default(),
            enabled: false,
            public_authority: None,
        }
    }

    /// The set, for tests and for the startup log.
    pub fn ranges(&self) -> &CidrSet {
        &self.ranges
    }

    /// Whether forwarded headers are read at all.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The `host[:port]` from `[access] public_url`, if configured.
    pub fn public_authority(&self) -> Option<&str> {
        self.public_authority.as_deref()
    }

    /// Whether `peer` is a proxy whose headers may be believed.
    pub fn peer_is_trusted(&self, peer: Option<IpAddr>) -> bool {
        match peer {
            Some(ip) => self.ranges.contains(ip),
            // No connect info (an in-process test, a hand-rolled service): fail
            // closed. Believing a header because we could not identify the peer
            // is exactly the spoof this module exists to stop.
            None => false,
        }
    }

    /// Resolve the client address and scheme for one request.
    pub fn client(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> ClientInfo {
        if !self.enabled || !self.peer_is_trusted(peer) {
            return ClientInfo {
                ip: peer,
                https: false,
            };
        }
        ClientInfo {
            ip: self.forwarded_ip(headers).or(peer),
            https: forwarded_scheme(headers).as_deref() == Some("https"),
        }
    }

    /// The client address from `X-Forwarded-For`, walking from the right.
    fn forwarded_ip(&self, headers: &HeaderMap) -> Option<IpAddr> {
        let raw = header_string(headers, "x-forwarded-for")?;
        let hops: Vec<&str> = raw.split(',').map(str::trim).collect();
        for hop in hops.iter().rev() {
            // `unknown`, an empty element and a malformed address are skipped
            // rather than treated as "stop here": a proxy that wrote garbage is
            // not the client, and stopping would return the proxy's own address.
            let Ok(ip) = hop.parse::<IpAddr>() else {
                continue;
            };
            if !self.ranges.contains(ip) {
                return Some(ip);
            }
        }
        None
    }

    /// Whether a request's `Host` may be judged by a forwarded host name.
    ///
    /// A proxy that rewrites `Host` to an internal name (`karucode:8787`, the
    /// Docker service name) and puts the public name in `X-Forwarded-Host` is
    /// common enough to support - but only for a trusted peer, and only as a
    /// **fallback**: the `Host` header is still checked first, and an entry here
    /// is still validated against the same allowlist by the caller. A client
    /// cannot set this header cross-origin without a preflight this server never
    /// approves.
    pub fn forwarded_authority(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Option<Authority> {
        if !self.enabled || !self.peer_is_trusted(peer) {
            return None;
        }
        let raw = header_string(headers, "x-forwarded-host")?;
        let first = raw.split(',').next()?.trim();
        parse_authority(first)
    }
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// The scheme from `X-Forwarded-Proto` (the left-most value, which is the one the
/// outermost proxy wrote).
fn forwarded_scheme(headers: &HeaderMap) -> Option<String> {
    let raw = header_string(headers, "x-forwarded-proto")?;
    let first = raw.split(',').next()?.trim().to_ascii_lowercase();
    (!first.is_empty()).then_some(first)
}

/// A range list as the startup log prints it.
pub fn describe_ranges(ranges: &[CidrRange]) -> String {
    ranges
        .iter()
        .map(|range| range.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use kc_core::config::TunnelKind;

    fn access(tunnel: TunnelKind, proxies: &[&str]) -> AccessConfig {
        AccessConfig {
            tunnel,
            trusted_proxies: proxies.iter().map(|p| p.to_string()).collect(),
            ..AccessConfig::default()
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn with_no_tunnel_the_peer_is_the_client_and_headers_are_ignored() {
        let trust = ProxyTrust::from_access(&AccessConfig::default()).unwrap();
        assert!(!trust.enabled());
        let info = trust.client(
            Some(ip("203.0.113.9")),
            &headers(&[
                ("x-forwarded-for", "10.0.0.1"),
                ("x-forwarded-proto", "https"),
            ]),
        );
        assert_eq!(info.ip, Some(ip("203.0.113.9")));
        assert!(!info.https, "a forwarded scheme is never believed here");
    }

    #[test]
    fn a_trusted_loopback_proxy_supplies_the_client_address() {
        let trust = ProxyTrust::from_access(&access(TunnelKind::Custom, &[])).unwrap();
        let info = trust.client(
            Some(ip("127.0.0.1")),
            &headers(&[
                ("x-forwarded-for", "198.51.100.7"),
                ("x-forwarded-proto", "https"),
            ]),
        );
        assert_eq!(info.ip, Some(ip("198.51.100.7")));
        assert!(info.https);
    }

    #[test]
    fn a_client_cannot_spoof_the_client_address_from_an_untrusted_peer() {
        let trust = ProxyTrust::from_access(&access(TunnelKind::Custom, &[])).unwrap();
        let info = trust.client(
            Some(ip("203.0.113.9")),
            &headers(&[("x-forwarded-for", "127.0.0.1")]),
        );
        assert_eq!(
            info.ip,
            Some(ip("203.0.113.9")),
            "a spoofed X-Forwarded-For from an untrusted peer must be ignored"
        );
    }

    #[test]
    fn the_chain_is_walked_from_the_right_past_trusted_hops() {
        let trust = ProxyTrust::from_access(&access(TunnelKind::Custom, &["10.0.0.0/8"])).unwrap();
        // client -> 10.0.0.2 (our proxy) -> 10.0.0.1 (another trusted hop): the
        // chain the proxy appended is "10.0.0.1, 10.0.0.2", and the client wrote
        // the leading bogus value.
        let info = trust.client(
            Some(ip("10.0.0.2")),
            &headers(&[("x-forwarded-for", "1.2.3.4, 198.51.100.7, 10.0.0.1, 10.0.0.2")]),
        );
        assert_eq!(
            info.ip,
            Some(ip("198.51.100.7")),
            "the first untrusted hop from the right is the client, not the value the client wrote"
        );
    }

    #[test]
    fn a_chain_of_only_trusted_hops_falls_back_to_the_peer() {
        let trust = ProxyTrust::from_access(&access(TunnelKind::Custom, &[])).unwrap();
        let info = trust.client(
            Some(ip("127.0.0.1")),
            &headers(&[("x-forwarded-for", "127.0.0.2, 127.0.0.1")]),
        );
        assert_eq!(info.ip, Some(ip("127.0.0.1")));
    }

    #[test]
    fn garbage_hops_are_skipped_not_returned() {
        let trust = ProxyTrust::from_access(&access(TunnelKind::Custom, &[])).unwrap();
        let info = trust.client(
            Some(ip("127.0.0.1")),
            &headers(&[("x-forwarded-for", "198.51.100.7, unknown, ") ]),
        );
        assert_eq!(info.ip, Some(ip("198.51.100.7")));
    }

    #[test]
    fn an_unknown_peer_is_never_trusted() {
        let trust = ProxyTrust::from_access(&access(TunnelKind::Custom, &[])).unwrap();
        assert!(!trust.peer_is_trusted(None));
        let info = trust.client(None, &headers(&[("x-forwarded-for", "198.51.100.7")]));
        assert_eq!(info.ip, None);
        assert!(!info.https);
    }

    #[test]
    fn cloudflare_ranges_are_trusted_only_when_that_tunnel_is_selected() {
        let with_cf = ProxyTrust::from_access(&access(TunnelKind::Cloudflare, &[])).unwrap();
        assert!(with_cf.peer_is_trusted(Some(ip("104.16.132.229"))));
        let custom = ProxyTrust::from_access(&access(TunnelKind::Custom, &[])).unwrap();
        assert!(!custom.peer_is_trusted(Some(ip("104.16.132.229"))));
    }

    #[test]
    fn a_forwarded_host_is_only_read_from_a_trusted_peer() {
        let trust = ProxyTrust::from_access(&access(TunnelKind::Custom, &[])).unwrap();
        let forwarded = headers(&[("x-forwarded-host", "karucode.example.com")]);
        assert!(trust.forwarded_authority(Some(ip("203.0.113.9")), &forwarded).is_none());
        let authority = trust
            .forwarded_authority(Some(ip("127.0.0.1")), &forwarded)
            .expect("a trusted peer's X-Forwarded-Host is readable");
        assert_eq!(authority.host, "karucode.example.com");
    }

    #[test]
    fn a_disabled_policy_trusts_nothing_including_loopback() {
        let trust = ProxyTrust::disabled();
        assert!(!trust.peer_is_trusted(Some(ip("127.0.0.1"))));
        assert_eq!(trust.public_authority(), None);
    }
}
