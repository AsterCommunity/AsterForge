//! Network address helpers.
//!
//! The module centralizes small network parsing rules shared by Aster services: loopback host
//! detection, trusted proxy CIDR parsing, and real-client-IP selection from `X-Forwarded-For`.
//! Header-framework adapters stay in application crates; this module accepts plain strings and
//! standard address types so it remains independent of Actix, Axum, or Hyper.

use std::net::{IpAddr, SocketAddr};

use ipnet::IpNet;

/// Returns whether `host` is localhost or a loopback IP address.
#[must_use]
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
#[must_use]
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
#[must_use]
pub fn is_trusted_proxy(ip: IpAddr, trusted: &[IpNet]) -> bool {
    trusted.iter().any(|net| net.contains(&ip))
}

/// Returns the client IP from `X-Forwarded-For`, or the direct peer address when
/// the peer is not a trusted proxy or the chain carries no trustworthy client.
///
/// The chain is walked from the right (the address closest to `peer`, which
/// trusted proxies append on the left of their own hop) towards the left,
/// skipping every address inside the trusted set. The first untrusted address
/// is the client as attested by the trusted proxies; everything further to its
/// left is client-supplied and must never win. Without this right-to-left walk
/// a client behind one untrusted proxy could plant an arbitrary address.
#[must_use]
pub fn real_ip_from_forwarded_for(
    x_forwarded_for: Option<&str>,
    peer: IpAddr,
    trusted: &[IpNet],
) -> IpAddr {
    if trusted.is_empty() || !is_trusted_proxy(peer, trusted) {
        return peer;
    }
    let Some(forwarded) = x_forwarded_for else {
        return peer;
    };
    for entry in forwarded.split(',').rev() {
        let Some(ip) = parse_forwarded_ip(entry) else {
            // Malformed entries are client-controlled noise: stop trusting the
            // remainder of the chain and fall back to the direct peer.
            return peer;
        };
        if !is_trusted_proxy(ip, trusted) {
            return ip;
        }
    }
    // Every entry was a trusted proxy (or the header was empty): the client is
    // the direct peer we already attested.
    peer
}

fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    value
        .parse::<IpAddr>()
        .or_else(|_| value.parse::<SocketAddr>().map(|address| address.ip()))
        .ok()
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
    fn real_ip_walks_the_chain_right_to_left_and_stops_at_the_first_untrusted_hop() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string()]);
        let peer = "10.0.0.5".parse::<IpAddr>().unwrap();

        // client -> proxy(198.51.100.2) -> trusted(10.0.0.5): the proxied
        // client address 198.51.100.2 is the first untrusted hop from the
        // right; the leftmost 203.0.113.10 is client-supplied and must lose.
        assert_eq!(
            real_ip_from_forwarded_for(
                Some("203.0.113.10, 198.51.100.2"),
                peer,
                &trusted,
            ),
            "198.51.100.2".parse::<IpAddr>().unwrap()
        );

        // client -> trusted proxy only: the sole entry is the real client.
        assert_eq!(
            real_ip_from_forwarded_for(
                Some("203.0.113.10"),
                peer,
                &trusted,
            ),
            "203.0.113.10".parse::<IpAddr>().unwrap()
        );

        // a spoofed client-supplied chain must not overwrite the attested hop
        assert_eq!(
            real_ip_from_forwarded_for(
                Some("6.6.6.6, 203.0.113.10"),
                peer,
                &trusted,
            ),
            "203.0.113.10".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn real_ip_accepts_forwarded_socket_address_forms() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string()]);
        let peer = "10.0.0.5".parse::<IpAddr>().unwrap();

        assert_eq!(
            real_ip_from_forwarded_for(Some("203.0.113.10:54321, 10.0.0.5"), peer, &trusted),
            "203.0.113.10".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            real_ip_from_forwarded_for(Some("[2001:db8::1]:443, 10.0.0.5"), peer, &trusted),
            "2001:db8::1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn real_ip_stops_at_malformed_entries_and_falls_back_to_peer() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string()]);
        let peer = "10.0.0.5".parse::<IpAddr>().unwrap();

        // a malformed hop to the right of the client invalidates the chain:
        // trust cannot skip past unparseable entries.
        assert_eq!(
            real_ip_from_forwarded_for(
                Some("203.0.113.10, not-an-ip, 10.0.0.9"),
                peer,
                &trusted,
            ),
            peer
        );
        assert_eq!(real_ip_from_forwarded_for(None, peer, &trusted), peer);
        assert_eq!(
            real_ip_from_forwarded_for(Some(""), peer, &trusted),
            peer
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
