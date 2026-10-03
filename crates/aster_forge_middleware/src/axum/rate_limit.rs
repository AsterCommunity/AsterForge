//! Axum rate limits with explicit peer/key and rejection-response boundaries.
pub use crate::shared::rate_limit::{NormalizedStringRateLimiter, RateLimitRejection};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use governor::{RateLimiter, clock::DefaultClock, state::keyed::DefaultKeyedStateStore};
use ipnet::IpNet;
use std::{
    net::{IpAddr, Ipv4Addr},
    num::{NonZeroU32, NonZeroU64},
    sync::Arc,
};

type IpLimiter = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;
type PeerResolver = dyn Fn(&Request) -> Option<IpAddr> + Send + Sync;
type RejectionMapper = dyn Fn(RateLimitRejection) -> Response + Send + Sync;
type KeyResolver = dyn Fn(&Request) -> Result<String, Box<Response>> + Send + Sync;

/// State for `from_fn_with_state(config, ip_rate_limit)`. Clones share the same quota buckets.
#[derive(Clone)]
pub struct IpRateLimitConfig {
    enabled: bool,
    trusted: Vec<IpNet>,
    limiter: Arc<IpLimiter>,
    peer: Arc<PeerResolver>,
    rejection: Arc<RejectionMapper>,
}

impl IpRateLimitConfig {
    /// Builds a shared IP limiter; the product supplies its 429 envelope and Retry-After policy.
    pub fn new<F>(
        enabled: bool,
        seconds_per_request: NonZeroU64,
        burst_size: NonZeroU32,
        trusted_proxies: &[String],
        rejection: F,
    ) -> Self
    where
        F: Fn(RateLimitRejection) -> Response + Send + Sync + 'static,
    {
        Self {
            enabled,
            trusted: aster_forge_utils::net::parse_trusted_proxies(trusted_proxies),
            limiter: Arc::new(RateLimiter::keyed(
                crate::shared::rate_limit::rate_limit_quota(seconds_per_request, burst_size),
            )),
            peer: Arc::new(super::client_ip::direct_peer),
            rejection: Arc::new(rejection),
        }
    }
    /// Overrides direct-peer extraction for an alternate trusted transport extension.
    /// This resolver must never infer a peer from unverified request headers.
    #[must_use]
    pub fn peer_resolver<P>(mut self, peer: P) -> Self
    where
        P: Fn(&Request) -> Option<IpAddr> + Send + Sync + 'static,
    {
        self.peer = Arc::new(peer);
        self
    }
    /// Removes expired buckets. Products may invoke this from their maintenance runtime.
    pub fn retain_recent(&self) {
        self.limiter.retain_recent();
    }
}

/// Applies trusted-proxy IP limits. Missing peers/UDS share a localhost bucket and ignore headers.
/// Products should disable IP limits or use keyed limits on UDS deployments.
pub async fn ip_rate_limit(
    State(config): State<IpRateLimitConfig>,
    request: Request,
    next: Next,
) -> Response {
    if config.enabled {
        let key = (config.peer)(&request).map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |peer| {
            super::client_ip::real_ip_from_trusted_headers(request.headers(), peer, &config.trusted)
        });
        if let Err(negative) = config.limiter.check_key(&key) {
            return (config.rejection)(RateLimitRejection::from_not_until(&negative));
        }
    }
    next.run(request).await
}

/// State for keyed limits. Product code extracts the key; Forge trims and lowercases it.
#[derive(Clone)]
pub struct KeyedRateLimitConfig {
    limiter: NormalizedStringRateLimiter,
    key: Arc<KeyResolver>,
    rejection: Arc<RejectionMapper>,
}
impl KeyedRateLimitConfig {
    /// Supplies a shared limiter, product key extraction, and product rejection response.
    pub fn new<K, F>(limiter: NormalizedStringRateLimiter, key: K, rejection: F) -> Self
    where
        K: Fn(&Request) -> Result<String, Box<Response>> + Send + Sync + 'static,
        F: Fn(RateLimitRejection) -> Response + Send + Sync + 'static,
    {
        Self {
            limiter,
            key: Arc::new(key),
            rejection: Arc::new(rejection),
        }
    }
}
/// Applies a normalized string quota, preserving product key-extraction errors.
pub async fn keyed_rate_limit(
    State(config): State<KeyedRateLimitConfig>,
    request: Request,
    next: Next,
) -> Response {
    if config.limiter.enabled() {
        let key = match (config.key)(&request) {
            Ok(key) => key,
            Err(response) => return *response,
        };
        if let Some(rejection) = config.limiter.check(&key) {
            return (config.rejection)(rejection);
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        extract::ConnectInfo,
        middleware,
        response::IntoResponse,
        routing::get,
    };
    use http::{Request as HttpRequest, StatusCode};
    use std::net::SocketAddr;
    use tower::ServiceExt;

    fn reject(rejection: RateLimitRejection) -> Response {
        let wait = rejection.retry_after_seconds();
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"code": "product_limit", "retry_after": wait})),
        )
            .into_response();
        response
            .headers_mut()
            .insert("retry-after", wait.to_string().parse().unwrap());
        response
    }
    fn ip_app(enabled: bool, burst: u32, trusted: &[String]) -> Router {
        let config = IpRateLimitConfig::new(
            enabled,
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(burst).unwrap(),
            trusted,
            reject,
        );
        Router::new()
            .route("/", get(|| async { "called" }))
            .layer(middleware::from_fn_with_state(config, ip_rate_limit))
    }
    fn request(peer: Option<&str>, forwarded: &str) -> HttpRequest<Body> {
        let mut request = HttpRequest::builder()
            .header("x-forwarded-for", forwarded)
            .body(Body::empty())
            .unwrap();
        if let Some(peer) = peer {
            request
                .extensions_mut()
                .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        }
        request
    }

    #[tokio::test]
    async fn ip_limits_isolate_clients_and_preserve_custom_429_retry_after() {
        let router = ip_app(true, 1, &["10.0.0.0/8".into()]);
        assert_eq!(
            router
                .clone()
                .oneshot(request(Some("10.0.0.5:1234"), "203.0.113.1"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            router
                .clone()
                .oneshot(request(Some("10.0.0.5:1234"), "203.0.113.2"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let response = router
            .oneshot(request(Some("10.0.0.5:1234"), "203.0.113.1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let delay: u64 = response.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&delay));
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["code"], "product_limit");
        assert_eq!(value["retry_after"], delay);
    }

    #[tokio::test]
    async fn untrusted_and_missing_peers_cannot_rotate_forwarded_headers_to_bypass_limits() {
        for peer in [Some("198.51.100.1:1234"), None] {
            let router = ip_app(true, 1, &["127.0.0.1".into()]);
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(peer, "203.0.113.1"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
            assert_eq!(
                router
                    .oneshot(request(peer, "203.0.113.2"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::TOO_MANY_REQUESTS
            );
        }
    }

    #[tokio::test]
    async fn disabled_ip_limit_never_extracts_the_peer() {
        let config = IpRateLimitConfig::new(
            false,
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
            &[],
            reject,
        )
        .peer_resolver(|_| panic!("disabled limiter must not invoke peer resolver"));
        let router = Router::new()
            .route("/", get(|| async { "called" }))
            .layer(middleware::from_fn_with_state(config, ip_rate_limit));
        for _ in 0..3 {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(None, "anything"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_requests_share_exactly_one_burst_across_router_clones() {
        let router = ip_app(true, 4, &[]);
        let mut jobs = Vec::new();
        for _ in 0..32 {
            let router = router.clone();
            jobs.push(tokio::spawn(async move {
                router
                    .oneshot(request(Some("203.0.113.1:1234"), ""))
                    .await
                    .unwrap()
                    .status()
            }));
        }
        let mut allowed = 0;
        for job in jobs {
            match job.await.unwrap() {
                StatusCode::OK => allowed += 1,
                StatusCode::TOO_MANY_REQUESTS => (),
                status => panic!("unexpected status {status}"),
            }
        }
        assert_eq!(allowed, 4);
    }

    fn keyed_app(enabled: bool) -> Router {
        let limiter = NormalizedStringRateLimiter::new(
            enabled,
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );
        let config = KeyedRateLimitConfig::new(
            limiter,
            |request| {
                request
                    .headers()
                    .get("x-key")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        Box::new(
                            (StatusCode::UNPROCESSABLE_ENTITY, "missing product key")
                                .into_response(),
                        )
                    })
            },
            reject,
        );
        Router::new()
            .route("/", get(|| async { "called" }))
            .layer(middleware::from_fn_with_state(config, keyed_rate_limit))
    }
    #[tokio::test]
    async fn string_keys_are_normalized_isolated_and_product_extraction_errors_are_retained() {
        let router = keyed_app(true);
        for (key, expected) in [
            ("User@Example.com", StatusCode::OK),
            ("other@example.com", StatusCode::OK),
            (" user@example.com ", StatusCode::TOO_MANY_REQUESTS),
        ] {
            let request = HttpRequest::builder()
                .header("x-key", key)
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                expected
            );
        }
        let response = router
            .oneshot(HttpRequest::new(Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            b"missing product key"
        );
    }
    #[tokio::test]
    async fn disabled_keyed_limit_bypasses_product_key_extraction() {
        let router = keyed_app(false);
        for _ in 0..3 {
            assert_eq!(
                router
                    .clone()
                    .oneshot(HttpRequest::new(Body::empty()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
    }
}
