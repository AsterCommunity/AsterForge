//! Shared Actix rate-limit building blocks.
//!
//! This module keeps product-neutral rate-limit mechanics in Forge while leaving
//! product response envelopes and protocol-specific error bodies in application
//! crates. It provides:
//!
//! - a trusted-proxy-aware IP key extractor for `actix-governor`;
//! - small quota helpers for building Actix governor configs from non-zero
//!   `(seconds_per_request, burst_size)` pairs;
//! - a keyed string limiter for protocol endpoints that rate-limit by usernames,
//!   emails, or other normalized business keys.

use std::net::{IpAddr, Ipv4Addr};
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::time::Duration;

use actix_governor::{
    GovernorConfig, GovernorConfigBuilder, KeyExtractor, SimpleKeyExtractionError,
};
use actix_web::dev::ServiceRequest;
use governor::clock::{Clock, DefaultClock, QuantaInstant};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{NotUntil, Quota, RateLimiter};
use ipnet::IpNet;

type StringKeyedLimiter =
    RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock, NoOpMiddleware>;

/// Trusted-proxy-aware IP key extractor for `actix-governor`.
///
/// The extractor uses the direct peer address by default. When the peer address
/// matches one of the trusted proxy CIDR entries, the leftmost `X-Forwarded-For`
/// address is used as the client key. Invalid or missing forwarded addresses
/// fall back to the direct peer address.
#[derive(Debug, Clone)]
pub struct TrustedProxyIpKeyExtractor {
    trusted: Vec<IpNet>,
}

impl TrustedProxyIpKeyExtractor {
    /// Builds an extractor from raw trusted proxy entries.
    ///
    /// Entries may be CIDR ranges or single IP addresses. Invalid entries are
    /// skipped by `aster_forge_utils::net::parse_trusted_proxies` after logging
    /// a warning.
    pub fn new(trusted_proxies: &[String]) -> Self {
        Self {
            trusted: aster_forge_utils::net::parse_trusted_proxies(trusted_proxies),
        }
    }

    /// Builds an extractor from an already parsed trusted proxy list.
    pub fn from_trusted(trusted: Vec<IpNet>) -> Self {
        Self { trusted }
    }

    /// Returns whether the provided IP is trusted as a proxy.
    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        aster_forge_utils::net::is_trusted_proxy(ip, &self.trusted)
    }

    /// Resolves the client IP for a request and direct peer IP.
    pub fn real_ip(&self, req: &ServiceRequest, peer: IpAddr) -> IpAddr {
        crate::client_ip::real_ip_from_trusted_headers(req.headers(), peer, &self.trusted)
    }
}

impl KeyExtractor for TrustedProxyIpKeyExtractor {
    type Key = IpAddr;
    type KeyExtractionError = SimpleKeyExtractionError<&'static str>;

    fn extract(&self, req: &ServiceRequest) -> Result<Self::Key, Self::KeyExtractionError> {
        let peer = req
            .peer_addr()
            .map(|socket| socket.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        Ok(self.real_ip(req, peer))
    }
}

/// Returns the retry delay in whole seconds for a governor rejection.
pub fn retry_after_seconds(not_until: &NotUntil<QuantaInstant>) -> u64 {
    not_until
        .wait_time_from(DefaultClock::default().now())
        .as_secs()
}

/// Builds an Actix governor config using the trusted-proxy-aware IP extractor.
///
/// The inputs are non-zero to match the application config model and avoid
/// runtime builder failures.
#[expect(
    clippy::expect_used,
    reason = "non-zero quota fields make actix-governor finish() infallible"
)]
pub fn build_ip_governor_config(
    seconds_per_request: NonZeroU64,
    burst_size: NonZeroU32,
    trusted_proxies: &[String],
) -> GovernorConfig<TrustedProxyIpKeyExtractor, NoOpMiddleware> {
    GovernorConfigBuilder::default()
        .key_extractor(TrustedProxyIpKeyExtractor::new(trusted_proxies))
        .seconds_per_request(seconds_per_request.get())
        .burst_size(burst_size.get())
        .finish()
        .expect("non-zero rate limit tier should always build")
}

/// A keyed string rate limiter with product-neutral key normalization.
///
/// The limiter trims surrounding whitespace and lowercases keys before checking
/// the quota. This suits usernames, email addresses, provider IDs, and similar
/// business-unique identifiers where accidental case differences should not
/// bypass a rate limit.
#[derive(Clone)]
pub struct NormalizedStringRateLimiter {
    enabled: bool,
    limiter: Arc<StringKeyedLimiter>,
}

impl NormalizedStringRateLimiter {
    /// Builds a limiter from a non-zero quota and enabled flag.
    pub fn new(enabled: bool, seconds_per_request: NonZeroU64, burst_size: NonZeroU32) -> Self {
        Self {
            enabled,
            limiter: Arc::new(build_string_keyed_limiter(seconds_per_request, burst_size)),
        }
    }

    /// Checks a raw key after trimming whitespace and lowercasing it.
    pub fn check(&self, raw_key: &str) -> Option<RateLimitRejection> {
        if !self.enabled {
            return None;
        }

        let key = raw_key.trim().to_ascii_lowercase();
        self.limiter
            .check_key(&key)
            .err()
            .map(RateLimitRejection::from_not_until)
    }
}

/// Product-neutral rate-limit rejection metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitRejection {
    retry_after_seconds: u64,
}

impl RateLimitRejection {
    fn from_not_until(not_until: NotUntil<QuantaInstant>) -> Self {
        Self {
            retry_after_seconds: retry_after_seconds(&not_until).max(1),
        }
    }

    /// Returns how many seconds clients should wait before retrying.
    pub const fn retry_after_seconds(self) -> u64 {
        self.retry_after_seconds
    }
}

#[expect(
    clippy::expect_used,
    reason = "NonZeroU64 seconds_per_request creates a non-zero duration"
)]
fn build_string_keyed_limiter(
    seconds_per_request: NonZeroU64,
    burst_size: NonZeroU32,
) -> StringKeyedLimiter {
    let quota = Quota::with_period(Duration::from_secs(seconds_per_request.get()))
        .expect("non-zero rate limit tier should always build")
        .allow_burst(burst_size);
    RateLimiter::keyed(quota)
}

#[cfg(test)]
mod tests {
    use super::{NormalizedStringRateLimiter, TrustedProxyIpKeyExtractor};
    use actix_governor::KeyExtractor;
    use actix_web::test as actix_test;
    use std::net::IpAddr;
    use std::num::{NonZeroU32, NonZeroU64};

    #[test]
    fn trusted_proxy_extractor_accepts_cidr_and_single_ip() {
        let extractor =
            TrustedProxyIpKeyExtractor::new(&["10.0.0.0/8".to_string(), "192.168.1.1".to_string()]);

        assert!(extractor.is_trusted("10.0.0.5".parse().unwrap()));
        assert!(extractor.is_trusted("192.168.1.1".parse().unwrap()));
        assert!(!extractor.is_trusted("203.0.113.1".parse().unwrap()));
    }

    #[actix_web::test]
    async fn trusted_proxy_extractor_uses_leftmost_forwarded_ip_only_from_trusted_peer() {
        let extractor = TrustedProxyIpKeyExtractor::new(&["10.0.0.0/8".to_string()]);
        let req = actix_test::TestRequest::default()
            .peer_addr("10.0.0.5:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", "203.0.113.10, 198.51.100.2"))
            .to_srv_request();

        assert_eq!(
            extractor.extract(&req).unwrap(),
            "203.0.113.10".parse::<IpAddr>().unwrap()
        );

        let untrusted = actix_test::TestRequest::default()
            .peer_addr("198.51.100.2:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", "203.0.113.10"))
            .to_srv_request();
        assert_eq!(
            extractor.extract(&untrusted).unwrap(),
            "198.51.100.2".parse::<IpAddr>().unwrap()
        );
    }

    #[actix_web::test]
    async fn trusted_proxy_extractor_falls_back_for_invalid_forwarded_ip_or_missing_peer() {
        let extractor = TrustedProxyIpKeyExtractor::new(&["10.0.0.0/8".to_string()]);
        let invalid_forwarded = actix_test::TestRequest::default()
            .peer_addr("10.0.0.5:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", "not-an-ip"))
            .to_srv_request();

        assert_eq!(
            extractor.extract(&invalid_forwarded).unwrap(),
            "10.0.0.5".parse::<IpAddr>().unwrap()
        );

        let missing_peer = actix_test::TestRequest::default().to_srv_request();
        assert_eq!(
            extractor.extract(&missing_peer).unwrap(),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn normalized_string_limiter_can_be_disabled() {
        let limiter = NormalizedStringRateLimiter::new(
            false,
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );

        assert!(limiter.check("admin@example.com").is_none());
        assert!(limiter.check("admin@example.com").is_none());
    }

    #[test]
    fn normalized_string_limiter_trims_and_lowercases_keys() {
        let limiter = NormalizedStringRateLimiter::new(
            true,
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );

        assert!(limiter.check("Admin@Example.com").is_none());
        assert!(limiter.check("other@example.com").is_none());
        assert!(limiter.check(" admin@example.com ").is_some());
    }
}
