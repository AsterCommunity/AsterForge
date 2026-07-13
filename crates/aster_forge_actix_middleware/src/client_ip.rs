//! Actix helpers for trusted-proxy-aware client IP extraction.
//!
//! The product-neutral parsing rules live in `aster_forge_utils::net`. This module only adapts
//! Actix `HeaderMap` values into those helpers so application services do not need to repeat
//! `X-Forwarded-For` extraction code.

use std::net::IpAddr;

use actix_web::http::header::HeaderMap;
use ipnet::IpNet;

/// Resolves the client IP from Actix headers, a direct peer IP, and raw trusted proxy entries.
///
/// `X-Forwarded-For` is trusted only when `peer` is covered by the trusted proxy list. Invalid
/// trusted proxy entries are skipped by `aster_forge_utils::net::parse_trusted_proxies`.
pub fn real_ip_from_headers(
    headers: &HeaderMap,
    peer: IpAddr,
    trusted_proxies: &[String],
) -> IpAddr {
    let trusted = aster_forge_utils::net::parse_trusted_proxies(trusted_proxies);
    real_ip_from_trusted_headers(headers, peer, &trusted)
}

/// Resolves the client IP from Actix headers, a direct peer IP, and parsed trusted proxy entries.
pub fn real_ip_from_trusted_headers(
    headers: &HeaderMap,
    peer: IpAddr,
    trusted: &[IpNet],
) -> IpAddr {
    let x_forwarded_for = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok());
    aster_forge_utils::net::real_ip_from_forwarded_for(x_forwarded_for, peer, trusted)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use actix_web::test as actix_test;
    use aster_forge_utils::net::{is_trusted_proxy, parse_trusted_proxies};

    use super::{real_ip_from_headers, real_ip_from_trusted_headers};

    #[test]
    fn parses_trusted_proxies_as_cidr_and_single_ip() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string(), "192.168.1.1".to_string()]);

        assert!(is_trusted_proxy("10.0.0.5".parse().unwrap(), &trusted));
        assert!(is_trusted_proxy("192.168.1.1".parse().unwrap(), &trusted));
        assert!(!is_trusted_proxy("203.0.113.1".parse().unwrap(), &trusted));
    }

    #[test]
    fn real_ip_uses_leftmost_forwarded_value_only_for_trusted_peer() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string()]);
        let req = actix_test::TestRequest::default()
            .insert_header(("X-Forwarded-For", "203.0.113.10, 198.51.100.2"))
            .to_srv_request();

        assert_eq!(
            real_ip_from_trusted_headers(
                req.headers(),
                "10.0.0.5".parse::<IpAddr>().unwrap(),
                &trusted,
            ),
            "203.0.113.10".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            real_ip_from_trusted_headers(
                req.headers(),
                "198.51.100.2".parse::<IpAddr>().unwrap(),
                &trusted,
            ),
            "198.51.100.2".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn real_ip_falls_back_to_peer_for_invalid_header() {
        let req = actix_test::TestRequest::default()
            .insert_header(("X-Forwarded-For", "not-an-ip"))
            .to_srv_request();

        assert_eq!(
            real_ip_from_headers(
                req.headers(),
                "10.0.0.5".parse::<IpAddr>().unwrap(),
                &["10.0.0.0/8".to_string()],
            ),
            "10.0.0.5".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn real_ip_accepts_forwarded_values_with_ports() {
        let trusted = parse_trusted_proxies(&["10.0.0.0/8".to_string()]);

        for (forwarded, expected) in [
            ("203.0.113.10:54321, 10.0.0.5", "203.0.113.10"),
            ("[2001:db8::1]:443, 10.0.0.5", "2001:db8::1"),
        ] {
            let req = actix_test::TestRequest::default()
                .insert_header(("X-Forwarded-For", forwarded))
                .to_srv_request();

            assert_eq!(
                real_ip_from_trusted_headers(
                    req.headers(),
                    "10.0.0.5".parse::<IpAddr>().unwrap(),
                    &trusted,
                ),
                expected.parse::<IpAddr>().unwrap()
            );
        }
    }
}
