//! IP ranges, for the `[access] trusted_proxies` list.
//!
//! A trusted-proxy entry is either a bare address (`203.0.113.7`, `::1`) or a
//! CIDR block (`10.0.0.0/8`, `2400:cb00::/32`). Both forms are matched against
//! the **direct peer** of a request, which is the whole point: `X-Forwarded-For`
//! is written by the client unless the socket's other end is a proxy the operator
//! named, so "is this peer trusted" has to be answered before a single forwarded
//! header is read.
//!
//! The parser is strict on purpose. A typo must be a startup error, not a rule
//! that silently matches nothing - a security control that quietly does nothing
//! is worse than one that is absent, because the log says it is on.

use crate::{Error, Result};
use std::fmt;
use std::net::IpAddr;

/// One parsed range: a base address plus a prefix length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CidrRange {
    base: IpAddr,
    prefix: u8,
}

impl CidrRange {
    /// Parse `ADDR` or `ADDR/PREFIX`.
    ///
    /// A prefix longer than the address width is an error (`10.0.0.0/33` is a
    /// typo, not `/32`), and so is a prefix that leaves host bits set
    /// (`10.0.0.1/8`): the second is the classic "I meant this host, not this
    /// /8" mistake, and silently masking it would widen the trust set.
    pub fn parse(raw: &str) -> Result<Self> {
        let text = raw.trim();
        if text.is_empty() {
            return Err(Error::Config("empty IP range".into()));
        }
        let (addr_part, prefix_part) = match text.split_once('/') {
            Some((addr, prefix)) => (addr.trim(), Some(prefix.trim())),
            None => (text, None),
        };
        let base: IpAddr = addr_part.parse().map_err(|_| {
            Error::Config(format!(
                "`{raw}` is not an IP address or CIDR block (expected e.g. 10.0.0.0/8 or 203.0.113.7)"
            ))
        })?;
        let max = if base.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix_part {
            None => max,
            Some(prefix) => {
                let value: u8 = prefix.parse().map_err(|_| {
                    Error::Config(format!(
                        "`{raw}` has an invalid prefix length `{prefix}` (expected 0-{max})"
                    ))
                })?;
                if value > max {
                    return Err(Error::Config(format!(
                        "`{raw}` has a prefix length of {value}, but this address family allows at most {max}"
                    )));
                }
                value
            }
        };
        let range = Self { base, prefix };
        if range.masked_base() != base {
            return Err(Error::Config(format!(
                "`{raw}` has host bits set; {} would also include addresses outside the block (did you mean {}/{prefix}?)",
                addr_part,
                range.masked_base()
            )));
        }
        Ok(range)
    }

    /// The address with every host bit cleared.
    fn masked_base(&self) -> IpAddr {
        match self.base {
            IpAddr::V4(addr) => {
                let bits = u32::from(addr);
                let mask = mask_u32(self.prefix as u32);
                IpAddr::V4((bits & mask).into())
            }
            IpAddr::V6(addr) => {
                let bits = u128::from(addr);
                let mask = mask_u128(self.prefix as u32);
                IpAddr::V6((bits & mask).into())
            }
        }
    }

    /// Whether `addr` falls inside this range. An IPv4-mapped IPv6 peer
    /// (`::ffff:10.0.0.1`, which is what a dual-stack listener reports) is
    /// matched against an IPv4 range too: the alternative is a trust rule that
    /// works on one socket family and silently not on the other.
    pub fn contains(&self, addr: IpAddr) -> bool {
        match (self.base, addr) {
            (IpAddr::V4(base), IpAddr::V4(addr)) => {
                mask_u32(self.prefix as u32) & u32::from(addr) == u32::from(base)
            }
            (IpAddr::V6(base), IpAddr::V6(addr)) => {
                mask_u128(self.prefix as u32) & u128::from(addr) == u128::from(base)
            }
            (IpAddr::V4(base), IpAddr::V6(addr)) => match mapped_v4(addr) {
                Some(v4) => mask_u32(self.prefix as u32) & u32::from(v4) == u32::from(base),
                None => false,
            },
            (IpAddr::V6(_), IpAddr::V4(_)) => false,
        }
    }
}

impl fmt::Display for CidrRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.base, self.prefix)
    }
}

/// The IPv4 address behind an IPv4-mapped IPv6 address.
fn mapped_v4(addr: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    addr.to_ipv4_mapped()
}

fn mask_u32(prefix: u32) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn mask_u128(prefix: u32) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

/// Parse a list of ranges, failing on the first bad entry and naming it.
pub fn parse_ranges(entries: &[String]) -> Result<Vec<CidrRange>> {
    entries
        .iter()
        .map(|entry| CidrRange::parse(entry))
        .collect()
}

/// A set of ranges; the empty set trusts nobody.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CidrSet {
    ranges: Vec<CidrRange>,
}

impl CidrSet {
    pub fn new(ranges: Vec<CidrRange>) -> Self {
        Self { ranges }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn contains(&self, addr: IpAddr) -> bool {
        self.ranges.iter().any(|range| range.contains(addr))
    }
}

/// Cloudflare's published proxy addresses (`cloudflare.com/ips-v4` and `-v6` at
/// the time of writing).
///
/// A Cloudflare Tunnel (`cloudflared`) connects *out* to Cloudflare, so requests
/// arrive from a local `cloudflared` process - usually `127.0.0.1`, which the
/// default loopback rule already trusts. These ranges matter for the other
/// deployment: the orange-cloud proxy mode, where Cloudflare connects to the
/// origin directly from one of its own addresses.
pub const CLOUDFLARE_V4: [&str; 15] = [
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
];

/// Cloudflare's published IPv6 proxy addresses.
pub const CLOUDFLARE_V6: [&str; 7] = [
    "2400:cb00::/32",
    "2606:4700::/32",
    "2803:f800::/32",
    "2405:b500::/32",
    "2405:8100::/32",
    "2a06:98c0::/29",
    "2c0f:f248::/32",
];

/// The Cloudflare ranges as a set, for `tunnel = "cloudflare"`.
pub fn cloudflare_ranges() -> Vec<CidrRange> {
    let mut out = Vec::with_capacity(CLOUDFLARE_V4.len() + CLOUDFLARE_V6.len());
    for entry in CLOUDFLARE_V4.iter().chain(CLOUDFLARE_V6.iter()) {
        // Constants, so a parse failure is a programming error, not input.
        if let Ok(range) = CidrRange::parse(entry) {
            out.push(range);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(cidr: &str, addr: &str) -> bool {
        CidrRange::parse(cidr)
            .unwrap()
            .contains(addr.parse::<IpAddr>().unwrap())
    }

    #[test]
    fn a_block_matches_inside_and_not_outside() {
        assert!(contains("10.0.0.0/8", "10.1.2.3"));
        assert!(contains("10.0.0.0/8", "10.255.255.255"));
        assert!(!contains("10.0.0.0/8", "11.0.0.1"));
        assert!(contains("172.16.0.0/12", "172.31.255.1"));
        assert!(!contains("172.16.0.0/12", "172.32.0.1"));
    }

    #[test]
    fn a_bare_address_matches_only_itself() {
        assert!(contains("203.0.113.7", "203.0.113.7"));
        assert!(!contains("203.0.113.7", "203.0.113.8"));
        assert!(contains("::1", "::1"));
    }

    #[test]
    fn host_bits_are_refused_rather_than_masked() {
        let error = CidrRange::parse("10.0.0.1/8").unwrap_err().to_string();
        assert!(error.contains("host bits set"), "{error}");
        assert!(error.contains("10.0.0.0/8"), "{error}");
        assert!(CidrRange::parse("10.0.0.0/8").is_ok());
    }

    #[test]
    fn a_prefix_too_long_for_the_family_is_refused() {
        assert!(CidrRange::parse("10.0.0.0/33").is_err());
        assert!(CidrRange::parse("::/129").is_err());
        assert!(CidrRange::parse("10.0.0.0/32").is_ok());
        assert!(CidrRange::parse("::/128").is_ok());
    }

    #[test]
    fn garbage_is_refused_with_the_input_named() {
        let error = CidrRange::parse("proxy.local").unwrap_err().to_string();
        assert!(error.contains("proxy.local"), "{error}");
        assert!(CidrRange::parse("").is_err());
    }

    #[test]
    fn an_ipv4_mapped_peer_matches_the_v4_rule() {
        // A dual-stack listener reports ::ffff:127.0.0.1 for an IPv4 client.
        assert!(contains("127.0.0.1/32", "::ffff:127.0.0.1"));
        assert!(contains("10.0.0.0/8", "::ffff:10.9.8.7"));
    }

    #[test]
    fn cloudflare_ranges_parse_and_cover_a_known_address() {
        let ranges = cloudflare_ranges();
        assert_eq!(ranges.len(), CLOUDFLARE_V4.len() + CLOUDFLARE_V6.len());
        let set = CidrSet::new(ranges);
        assert!(set.contains("104.16.132.229".parse().unwrap()));
        assert!(set.contains("2606:4700::6810:84e5".parse().unwrap()));
        assert!(!set.contains("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn an_empty_set_trusts_nobody() {
        let set = CidrSet::default();
        assert!(set.is_empty());
        assert!(!set.contains("127.0.0.1".parse().unwrap()));
    }
}
