//! Network address helpers.
//!
//! The module centralizes small network parsing rules shared by Aster services: loopback host
//! detection, trusted proxy CIDR parsing, and real-client-IP selection from `X-Forwarded-For`.
//! Header-framework adapters stay in application crates; this module accepts plain strings and
//! standard address types so it remains independent of Actix, Axum, or Hyper.

use std::net::IpAddr;

use ipnet::IpNet;

/// Returns whether `host` is localhost or a loopback IP address.
pub fn is_loopback_host(host: &str) -> bool {
    let trimmed = host.trim();
    let host = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);

    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Parses trusted proxy entries as CIDR networks or single IP addresses.
///
/// Invalid entries are skipped after emitting a warning. This mirrors the fail-open-at-startup
/// behavior used by the application repositories: one bad optional proxy entry should not prevent
/// the service from starting, but it also must not become trusted.
pub fn parse_trusted_proxies(trusted_proxies: &[String]) -> Vec<IpNet> {
    trusted_proxies
        .iter()
        .filter_map(|entry| {
            entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                .map_err(|error| tracing::warn!("invalid trusted_proxy entry '{entry}': {error}"))
                .ok()
        })
        .collect()
}

/// Returns whether `ip` is covered by the trusted proxy list.
pub fn is_trusted_proxy(ip: IpAddr, trusted: &[IpNet]) -> bool {
    trusted.iter().any(|net| net.contains(&ip))
}

/// Returns the first client IP from `X-Forwarded-For` only when `peer` is trusted.
///
/// The leftmost value is used because application reverse proxies append their own address to the
/// right. If the peer is not trusted, the header is ignored. Malformed or empty header values fall
/// back to the direct peer address.
pub fn real_ip_from_forwarded_for(
    x_forwarded_for: Option<&str>,
    peer: IpAddr,
    trusted: &[IpNet],
) -> IpAddr {
    if !trusted.is_empty() && is_trusted_proxy(peer, trusted) {
        let ip = x_forwarded_for
            .and_then(|value| value.split(',').next())
            .and_then(|part| part.trim().parse::<IpAddr>().ok());
        if let Some(ip) = ip {
            return ip;
        }
    }
    peer
}

#[cfg(test)]
mod tests {
    use super::{
        is_loopback_host, is_trusted_proxy, parse_trusted_proxies, real_ip_from_forwarded_for,
    };
    use std::net::IpAddr;

    #[test]
    fn detects_loopback_hosts() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.0.0.2"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("[::1]"));

        assert!(!is_loopback_host("example.com"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.10"));
    }

    #[test]
    fn parse_trusted_proxies_accepts_cidr_and_single_ip() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string(), "192.168.1.1".to_string()]);

        assert!(is_trusted_proxy("10.0.0.5".parse().unwrap(), &trusted));
        assert!(is_trusted_proxy("192.168.1.1".parse().unwrap(), &trusted));
        assert!(!is_trusted_proxy("203.0.113.1".parse().unwrap(), &trusted));
    }

    #[test]
    fn parse_trusted_proxies_skips_invalid_entries() {
        let trusted = parse_trusted_proxies(&["not-a-proxy".to_string(), "10.0.0.0/8".to_string()]);

        assert_eq!(trusted.len(), 1);
        assert!(is_trusted_proxy("10.1.2.3".parse().unwrap(), &trusted));
    }

    #[test]
    fn real_ip_uses_leftmost_xff_only_for_trusted_peer() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string()]);

        assert_eq!(
            real_ip_from_forwarded_for(
                Some("203.0.113.10, 198.51.100.2"),
                "10.0.0.5".parse::<IpAddr>().unwrap(),
                &trusted,
            ),
            "203.0.113.10".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            real_ip_from_forwarded_for(
                Some("203.0.113.10"),
                "198.51.100.2".parse::<IpAddr>().unwrap(),
                &trusted,
            ),
            "198.51.100.2".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn real_ip_falls_back_to_peer_for_invalid_or_missing_xff() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string()]);
        let peer = "10.0.0.5".parse::<IpAddr>().unwrap();

        assert_eq!(
            real_ip_from_forwarded_for(Some("not-an-ip"), peer, &trusted),
            peer
        );
        assert_eq!(real_ip_from_forwarded_for(None, peer, &trusted), peer);
    }
}
