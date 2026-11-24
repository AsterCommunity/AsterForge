//! Shared Actix rate-limit building blocks.
//!
//! This module keeps product-neutral rate-limit mechanics in Forge while leaving
//! product response envelopes and protocol-specific error bodies in application
//! crates. It wraps the [`governor`] crate's keyed rate limiter directly and
//! provides:
//!
//! - [`RateLimitKeyExtractor`], the key-extraction and rejection-response
//!   contract used by the middleware;
//! - [`RateLimiterConfig`], a cloneable configuration that shares one quota
//!   state across Actix workers;
//! - [`RateLimit`], the Actix middleware installed via [`actix_web::dev::Transform`];
//! - a trusted-proxy-aware IP key extractor ([`TrustedProxyIpKeyExtractor`]);
//! - small helpers for building IP configs from non-zero
//!   `(seconds_per_request, burst_size)` pairs;
//! - a keyed string limiter for protocol endpoints that rate-limit by usernames,
//!   emails, or other normalized business keys (see [`NormalizedStringRateLimiter`]).

use std::fmt;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr};
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;

use actix_web::body::EitherBody;
use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready};
use actix_web::http::header::ContentType;
use actix_web::{Error, HttpResponse, HttpResponseBuilder};
use futures::future::{LocalBoxFuture, Ready, err, ok};
use governor::clock::{Clock, DefaultClock};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use ipnet::IpNet;

pub use crate::shared::rate_limit::{
    NormalizedStringRateLimiter, RateLimitRejection, retry_after_seconds,
};

/// Keyed rate limiter shared by all workers of one [`RateLimiterConfig`].
type SharedKeyedRateLimiter<K, C> =
    RateLimiter<K, DefaultKeyedStateStore<K>, C, NoOpMiddleware<<C as Clock>::Instant>>;

/// Product-neutral contract for deriving a rate-limit key from a request and
/// rendering the rejection response when a key exceeds its quota.
///
/// Extraction errors abort the request with the returned error untouched, so
/// implementations can map failures to any response they need. Extractors that
/// can always derive a key (e.g. [`TrustedProxyIpKeyExtractor`], which falls
/// back to the peer address or loopback) simply never return `Err`.
pub trait RateLimitKeyExtractor: Send + Sync + 'static {
    /// The bucket key. Distinct keys never share quota state.
    type Key: Hash + Eq + Clone + Send + Sync + 'static;

    /// Derives the bucket key for a request.
    ///
    /// # Errors
    ///
    /// Returns the extraction error untouched; it aborts the request with the
    /// error's own response rendering.
    fn extract_key(&self, req: &ServiceRequest) -> Result<Self::Key, Error>;

    /// Builds the response returned when the key exceeds its quota.
    ///
    /// `retry_after_seconds` is the rounded-up wait time (always at least 1).
    /// The provided builder is already initialized with `429 Too Many Requests`;
    /// implementations may override the status.
    fn exceed_rate_limit_response(
        &self,
        retry_after_seconds: u64,
        response: HttpResponseBuilder,
    ) -> HttpResponse;
}

/// Cloneable rate-limit configuration holding one shared quota state.
///
/// Clones share the same limiter, so counters survive Actix worker forks; wrap
/// the config in [`RateLimit::new`] per worker app.
pub struct RateLimiterConfig<K: RateLimitKeyExtractor, C: Clock = DefaultClock> {
    limiter: Arc<SharedKeyedRateLimiter<K::Key, C>>,
    extractor: Arc<K>,
}

impl<K: RateLimitKeyExtractor, C: Clock> Clone for RateLimiterConfig<K, C> {
    fn clone(&self) -> Self {
        Self {
            limiter: Arc::clone(&self.limiter),
            extractor: Arc::clone(&self.extractor),
        }
    }
}

impl<K: RateLimitKeyExtractor> RateLimiterConfig<K, DefaultClock> {
    /// Builds a config from a [`Quota`] and a key extractor.
    ///
    /// Use [`crate::shared::rate_limit::rate_limit_quota`] to build a quota from
    /// the non-zero `(seconds_per_request, burst_size)` pairs used by product
    /// config models.
    #[must_use]
    pub fn new(quota: Quota, extractor: K) -> Self {
        Self::with_clock(quota, extractor, DefaultClock::default())
    }
}

impl<K: RateLimitKeyExtractor, C: Clock> RateLimiterConfig<K, C> {
    /// Builds a config using an explicitly supplied clock.
    ///
    /// This supports deterministic tests and products with a deliberate custom clock source.
    #[must_use]
    pub fn with_clock(quota: Quota, extractor: K, clock: C) -> Self {
        Self {
            limiter: Arc::new(RateLimiter::new(
                quota,
                DefaultKeyedStateStore::default(),
                clock,
            )),
            extractor: Arc::new(extractor),
        }
    }

    /// Removes expired key buckets; call from the owning product's maintenance runtime.
    ///
    /// Keyed state is otherwise retained for the lifetime of this config. Products that accept
    /// attacker-controlled or high-cardinality keys should schedule this method periodically.
    pub fn retain_recent(&self) {
        self.limiter.retain_recent();
    }
}

/// Actix middleware that rate-limits requests by [`RateLimitKeyExtractor`] keys.
///
/// Clone one [`RateLimiterConfig`], then install one [`RateLimit::new`] per
/// worker app; all installed wrappers consume the same quota state.
pub struct RateLimit<K: RateLimitKeyExtractor, C: Clock = DefaultClock> {
    config: RateLimiterConfig<K, C>,
}

impl<K: RateLimitKeyExtractor, C: Clock> RateLimit<K, C> {
    /// Wraps a config clone for installation on one worker app.
    #[must_use]
    pub fn new(config: &RateLimiterConfig<K, C>) -> Self {
        Self {
            config: config.clone(),
        }
    }
}

impl<S, B, K, C> Transform<S, ServiceRequest> for RateLimit<K, C>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: actix_web::body::MessageBody + 'static,
    K: RateLimitKeyExtractor + 'static,
    C: Clock + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type InitError = ();
    type Transform = RateLimitService<S, K, C>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(RateLimitService {
            service: std::rc::Rc::new(service),
            config: self.config.clone(),
        })
    }
}

/// Service wrapper installed by [`RateLimit`].
pub struct RateLimitService<S, K: RateLimitKeyExtractor, C: Clock = DefaultClock> {
    service: std::rc::Rc<S>,
    config: RateLimiterConfig<K, C>,
}

impl<S, B, K, C> Service<ServiceRequest> for RateLimitService<S, K, C>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: actix_web::body::MessageBody + 'static,
    K: RateLimitKeyExtractor + 'static,
    C: Clock + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = std::rc::Rc::clone(&self.service);
        let extractor = Arc::clone(&self.config.extractor);

        let key = match extractor.extract_key(&req) {
            Ok(key) => key,
            Err(error) => return Box::pin(err(error)),
        };

        if let Err(not_until) = self.config.limiter.check_key(&key) {
            let retry_after = retry_after_for_clock(&not_until, self.config.limiter.clock());
            let response =
                extractor.exceed_rate_limit_response(retry_after, HttpResponse::TooManyRequests());
            let (http_req, _) = req.into_parts();
            return Box::pin(ok(
                ServiceResponse::new(http_req, response).map_into_right_body()
            ));
        }

        Box::pin(async move {
            service
                .call(req)
                .await
                .map(ServiceResponse::map_into_left_body)
        })
    }
}

fn retry_after_for_clock<C: Clock>(not_until: &governor::NotUntil<C::Instant>, clock: &C) -> u64 {
    let delay = not_until.wait_time_from(clock.now());
    delay
        .as_secs()
        .saturating_add(u64::from(delay.subsec_nanos() > 0))
        .max(1)
}

type RejectionResponseFactory =
    dyn Fn(u64, HttpResponseBuilder) -> HttpResponse + Send + Sync + 'static;

/// Trusted-proxy-aware IP key extractor.
///
/// The extractor uses the direct peer address by default. When the peer address
/// matches a trusted proxy, `X-Forwarded-For` is walked from right to left, skipping
/// trusted hops and using the first untrusted address as the client key. Malformed
/// hops encountered before that address, missing headers and entirely trusted
/// chains fall back to the direct peer address.
///
/// Deployments without a peer address (e.g. Unix domain sockets) fall back to
/// `127.0.0.1`, so every client shares one rate-limit bucket and a single
/// burst rejects all of them. Products serving over UDS should disable IP
/// rate limiting or use [`NormalizedStringRateLimiter`] business keys instead.
#[derive(Clone)]
pub struct TrustedProxyIpKeyExtractor {
    trusted: Vec<IpNet>,
    rejection_response: Option<Arc<RejectionResponseFactory>>,
}

impl fmt::Debug for TrustedProxyIpKeyExtractor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedProxyIpKeyExtractor")
            .field("trusted", &self.trusted)
            .field(
                "has_custom_rejection_response",
                &self.rejection_response.is_some(),
            )
            .finish()
    }
}

impl TrustedProxyIpKeyExtractor {
    /// Builds an extractor from raw trusted proxy entries.
    ///
    /// Entries may be CIDR ranges or single IP addresses. Invalid entries are
    /// skipped by `aster_forge_utils::net::parse_trusted_proxies` after logging
    /// a warning.
    #[must_use]
    pub fn new(trusted_proxies: &[String]) -> Self {
        Self {
            trusted: aster_forge_utils::net::parse_trusted_proxies(trusted_proxies),
            rejection_response: None,
        }
    }

    /// Builds an extractor from an already parsed trusted proxy list.
    #[must_use]
    pub fn from_trusted(trusted: Vec<IpNet>) -> Self {
        Self {
            trusted,
            rejection_response: None,
        }
    }

    /// Uses a product-provided response factory when the request exceeds its quota.
    ///
    /// The factory receives the retry delay in whole seconds and the response builder
    /// initialized with `429 Too Many Requests`. Products can use this to preserve
    /// their response envelope and error code without reimplementing trusted-proxy
    /// extraction.
    #[must_use]
    pub fn with_rejection_response<F>(mut self, factory: F) -> Self
    where
        F: Fn(u64, HttpResponseBuilder) -> HttpResponse + Send + Sync + 'static,
    {
        self.rejection_response = Some(Arc::new(factory));
        self
    }

    /// Returns whether the provided IP is trusted as a proxy.
    #[must_use]
    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        aster_forge_utils::net::is_trusted_proxy(ip, &self.trusted)
    }

    /// Resolves the client IP for a request and direct peer IP.
    #[must_use]
    pub fn real_ip(&self, req: &ServiceRequest, peer: IpAddr) -> IpAddr {
        crate::actix::client_ip::real_ip_from_trusted_headers(req.headers(), peer, &self.trusted)
    }
}

impl RateLimitKeyExtractor for TrustedProxyIpKeyExtractor {
    type Key = IpAddr;

    fn extract_key(&self, req: &ServiceRequest) -> Result<Self::Key, Error> {
        // A localhost fallback is a bucket key, never proof that a proxy sent these headers.
        Ok(req
            .peer_addr()
            .map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |socket| {
                self.real_ip(req, socket.ip())
            }))
    }

    fn exceed_rate_limit_response(
        &self,
        retry_after: u64,
        mut response: HttpResponseBuilder,
    ) -> HttpResponse {
        if let Some(factory) = &self.rejection_response {
            return factory(retry_after, response);
        }
        response
            .insert_header(("Retry-After", retry_after.to_string()))
            .content_type(ContentType::plaintext())
            .body(format!("Too many requests, retry in {retry_after}s"))
    }
}

/// Builds a rate-limit config using the trusted-proxy-aware IP extractor.
///
/// The inputs are non-zero to match the application config model; the quota
/// construction is therefore infallible.
#[must_use]
pub fn build_ip_rate_limit_config(
    seconds_per_request: NonZeroU64,
    burst_size: NonZeroU32,
    trusted_proxies: &[String],
) -> RateLimiterConfig<TrustedProxyIpKeyExtractor> {
    RateLimiterConfig::new(
        crate::shared::rate_limit::rate_limit_quota(seconds_per_request, burst_size),
        TrustedProxyIpKeyExtractor::new(trusted_proxies),
    )
}

/// Builds a rate-limit config with a product-provided rejection response.
#[must_use]
pub fn build_ip_rate_limit_config_with_rejection_response<F>(
    seconds_per_request: NonZeroU64,
    burst_size: NonZeroU32,
    trusted_proxies: &[String],
    rejection_response: F,
) -> RateLimiterConfig<TrustedProxyIpKeyExtractor>
where
    F: Fn(u64, HttpResponseBuilder) -> HttpResponse + Send + Sync + 'static,
{
    RateLimiterConfig::new(
        crate::shared::rate_limit::rate_limit_quota(seconds_per_request, burst_size),
        TrustedProxyIpKeyExtractor::new(trusted_proxies)
            .with_rejection_response(rejection_response),
    )
}

#[cfg(test)]
mod tests {
    use super::{RateLimit, RateLimitKeyExtractor, RateLimiterConfig};
    use crate::shared::rate_limit::rate_limit_quota;
    use actix_web::dev::ServiceRequest;
    use actix_web::{
        App, HttpResponse, HttpResponseBuilder, http::StatusCode, test as actix_test, web,
    };
    use governor::Quota;
    use std::net::IpAddr;
    use std::num::{NonZeroU32, NonZeroU64};
    use std::time::Duration;

    struct StringKeyExtractor;

    impl RateLimitKeyExtractor for StringKeyExtractor {
        type Key = String;

        fn extract_key(&self, _req: &ServiceRequest) -> Result<Self::Key, actix_web::Error> {
            Ok("test-key".to_string())
        }

        fn exceed_rate_limit_response(
            &self,
            retry_after: u64,
            mut response: HttpResponseBuilder,
        ) -> HttpResponse {
            response
                .insert_header(("Retry-After", retry_after.to_string()))
                .body(format!("limited, retry in {retry_after}s"))
        }
    }

    /// A key extractor whose extraction always fails with a 400.
    struct FailingKeyExtractor;

    impl RateLimitKeyExtractor for FailingKeyExtractor {
        type Key = String;

        fn extract_key(&self, _req: &ServiceRequest) -> Result<Self::Key, actix_web::Error> {
            Err(actix_web::error::ErrorBadRequest("bad key"))
        }

        fn exceed_rate_limit_response(
            &self,
            _retry_after: u64,
            _response: HttpResponseBuilder,
        ) -> HttpResponse {
            HttpResponse::TooManyRequests().finish()
        }
    }

    /// A key extractor that fails only on its first extraction.
    ///
    /// Requests rejected by extraction must not consume quota, so the second
    /// request sees a full bucket even with a burst of one.
    struct FailOnceExtractor {
        failed: std::sync::atomic::AtomicBool,
    }

    impl RateLimitKeyExtractor for FailOnceExtractor {
        type Key = String;

        fn extract_key(&self, _req: &ServiceRequest) -> Result<Self::Key, actix_web::Error> {
            if self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                Ok("counted".to_string())
            } else {
                Err(actix_web::error::ErrorBadRequest("transient failure"))
            }
        }

        fn exceed_rate_limit_response(
            &self,
            retry_after: u64,
            mut response: HttpResponseBuilder,
        ) -> HttpResponse {
            response
                .insert_header(("Retry-After", retry_after.to_string()))
                .finish()
        }
    }

    fn string_config(quota: Quota) -> RateLimiterConfig<StringKeyExtractor> {
        RateLimiterConfig::new(quota, StringKeyExtractor)
    }

    #[actix_web::test]
    async fn allows_requests_within_burst_and_rejects_the_next_one() {
        let quota = rate_limit_quota(NonZeroU64::new(60).unwrap(), NonZeroU32::new(3).unwrap());
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&string_config(quota)))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        for _ in 0..3 {
            let request = actix_test::TestRequest::get().uri("/").to_request();
            assert_eq!(
                actix_test::call_service(&app, request).await.status(),
                StatusCode::OK,
                "requests within the burst must pass"
            );
        }

        let rejected =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = rejected
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        assert_eq!(
            retry_after,
            Some(60),
            "retry delay must round up to whole seconds"
        );
    }

    #[actix_web::test]
    async fn burst_of_one_rejects_every_request_after_the_first() {
        let quota = rate_limit_quota(NonZeroU64::new(60).unwrap(), NonZeroU32::new(1).unwrap());
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&string_config(quota)))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let first =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(first.status(), StatusCode::OK);

        for _ in 0..2 {
            let rejected = actix_test::call_service(
                &app,
                actix_test::TestRequest::get().uri("/").to_request(),
            )
            .await;
            assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
        }
    }

    #[actix_web::test]
    async fn distinct_keys_never_share_quota_state() {
        let quota = rate_limit_quota(NonZeroU64::new(60).unwrap(), NonZeroU32::new(1).unwrap());
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&string_config(quota)))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        // One limiter, one default key: exhausting the key must not affect a
        // different extractor instance with its own state store.
        let exhausted =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(exhausted.status(), StatusCode::OK);

        let rejected =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);

        let other_config = string_config(quota);
        let other_app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&other_config))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;
        let other = actix_test::call_service(
            &other_app,
            actix_test::TestRequest::get().uri("/").to_request(),
        )
        .await;
        assert_eq!(other.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn quota_refills_after_the_period_elapses() {
        use governor::clock::FakeRelativeClock;

        let clock = FakeRelativeClock::default();
        let quota = rate_limit_quota(NonZeroU64::new(1).unwrap(), NonZeroU32::new(1).unwrap());
        let config = RateLimiterConfig::with_clock(quota, StringKeyExtractor, clock.clone());
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&config))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let first =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(first.status(), StatusCode::OK);

        let rejected =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);

        clock.advance(Duration::from_secs(1));

        let refilled =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(refilled.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn default_rejection_is_plaintext_with_retry_after_header() {
        // The plaintext default is the TrustedProxyIpKeyExtractor behavior;
        // requests without a peer address fall back to the loopback bucket key.
        let config = crate::actix::rate_limit::build_ip_rate_limit_config(
            NonZeroU64::new(30).unwrap(),
            NonZeroU32::new(1).unwrap(),
            &[],
        );
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&config))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let exhausted =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(exhausted.status(), StatusCode::OK);

        let rejected =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(rejected.headers().contains_key("Retry-After"));
        assert_eq!(
            rejected
                .headers()
                .get("Content-Type")
                .and_then(|value| value.to_str().ok()),
            Some("text/plain; charset=utf-8"),
            "default rejection body is plaintext"
        );
        let body = actix_test::read_body(rejected).await;
        let body_text = String::from_utf8_lossy(&body);
        assert!(
            body_text.contains("retry in 30s"),
            "default body should advertise the retry delay"
        );
    }

    #[actix_web::test]
    async fn failed_extraction_does_not_consume_quota() {
        let quota = rate_limit_quota(NonZeroU64::new(3600).unwrap(), NonZeroU32::new(1).unwrap());
        let config: RateLimiterConfig<FailOnceExtractor> = RateLimiterConfig::new(
            quota,
            FailOnceExtractor {
                failed: std::sync::atomic::AtomicBool::new(false),
            },
        );
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&config))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        // First request fails extraction and must leave the full bucket intact.
        let failed = actix_test::try_call_service(
            &app,
            actix_test::TestRequest::get().uri("/").to_request(),
        )
        .await;
        assert!(failed.is_err());

        // Second request passes: the extraction failure consumed nothing.
        let passed =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(passed.status(), StatusCode::OK);

        // Third request is the one that exhausts the bucket.
        let exhausted =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;
        assert_eq!(exhausted.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[actix_web::test]
    async fn key_extraction_errors_are_propagated_untouched() {
        let quota = rate_limit_quota(NonZeroU64::new(60).unwrap(), NonZeroU32::new(1).unwrap());
        let config: RateLimiterConfig<FailingKeyExtractor> =
            RateLimiterConfig::new(quota, FailingKeyExtractor);
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&config))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let response = actix_test::try_call_service(
            &app,
            actix_test::TestRequest::get().uri("/").to_request(),
        )
        .await;
        // The middleware must surface the extractor error untouched; the test
        // harness renders it via the error's own ResponseError impl.
        assert!(response.is_err());
        let response = response.unwrap_err();
        assert_eq!(
            response.as_response_error().status_code(),
            StatusCode::BAD_REQUEST
        );
    }

    #[actix_web::test]
    async fn cloned_configs_consume_the_same_allowance_across_worker_apps() {
        let quota = rate_limit_quota(NonZeroU64::new(3600).unwrap(), NonZeroU32::new(1).unwrap());
        let shared = string_config(quota);
        let worker_one = shared.clone();
        let worker_two = shared.clone();
        let app_one = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&worker_one))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;
        let app_two = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&worker_two))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let first = actix_test::call_service(
            &app_one,
            actix_test::TestRequest::get().uri("/").to_request(),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);

        let second = actix_test::call_service(
            &app_two,
            actix_test::TestRequest::get().uri("/").to_request(),
        )
        .await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[actix_web::test]
    async fn custom_rejection_response_preserves_product_envelope() {
        let config = crate::actix::rate_limit::build_ip_rate_limit_config_with_rejection_response(
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
            &[],
            |retry_after, mut response| {
                response
                    .insert_header(("Retry-After", retry_after.to_string()))
                    .json(serde_json::json!({
                        "code": "rate_limited",
                        "retry_after": retry_after,
                    }))
            },
        );
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&config))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let first = actix_test::TestRequest::get()
            .uri("/")
            .peer_addr("127.0.0.1:12345".parse().unwrap())
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, first).await.status(),
            StatusCode::OK
        );

        let second = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/")
                .peer_addr("127.0.0.1:12345".parse().unwrap())
                .to_request(),
        )
        .await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(second.headers().contains_key("Retry-After"));
        let body: serde_json::Value = actix_test::read_body_json(second).await;
        assert_eq!(body["code"], "rate_limited");
        assert!(body["retry_after"].as_u64().is_some_and(|value| value > 0));
    }

    #[actix_web::test]
    async fn trusted_proxy_extractor_uses_client_ip_from_forwarded_chain_as_key() {
        // Two clients behind one trusted proxy must get independent buckets.
        let config = crate::actix::rate_limit::build_ip_rate_limit_config(
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
            &["10.0.0.0/8".to_string()],
        );
        let app = actix_test::init_service(
            App::new()
                .wrap(RateLimit::new(&config))
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let client_a = actix_test::TestRequest::get()
            .uri("/")
            .peer_addr("10.0.0.5:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", "203.0.113.10"))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, client_a).await.status(),
            StatusCode::OK
        );

        // Same client IP again: rejected.
        let client_a_again = actix_test::TestRequest::get()
            .uri("/")
            .peer_addr("10.0.0.5:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", "203.0.113.10"))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, client_a_again)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );

        // Different client behind the same proxy: independent bucket.
        let client_b = actix_test::TestRequest::get()
            .uri("/")
            .peer_addr("10.0.0.5:54321".parse().unwrap())
            .insert_header(("X-Forwarded-For", "198.51.100.2"))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, client_b).await.status(),
            StatusCode::OK
        );
    }

    #[test]
    fn trusted_proxy_extractor_accepts_cidr_and_single_ip() {
        let extractor = crate::actix::rate_limit::TrustedProxyIpKeyExtractor::new(&[
            "10.0.0.0/8".to_string(),
            "192.168.1.1".to_string(),
        ]);

        assert!(extractor.is_trusted("10.0.0.5".parse().unwrap()));
        assert!(extractor.is_trusted("192.168.1.1".parse().unwrap()));
        assert!(!extractor.is_trusted("203.0.113.1".parse().unwrap()));
    }

    #[actix_web::test]
    async fn trusted_proxy_extractor_stops_at_first_untrusted_hop_from_trusted_peer() {
        use crate::actix::rate_limit::TrustedProxyIpKeyExtractor;
        let extractor = TrustedProxyIpKeyExtractor::new(&["10.0.0.0/8".to_string()]);
        for (peer, forwarded, expected) in [
            (
                "10.0.0.5:12345",
                "203.0.113.10, 198.51.100.2",
                "198.51.100.2",
            ),
            ("10.0.0.5:12345", "203.0.113.10, 10.0.0.9", "203.0.113.10"),
            ("10.0.0.5:12345", "203.0.113.10", "203.0.113.10"),
            (
                "10.0.0.5:12345",
                "203.0.113.10, not-an-ip, 10.0.0.9",
                "10.0.0.5",
            ),
            ("10.0.0.5:12345", "10.0.0.8, 10.0.0.9", "10.0.0.5"),
            ("198.51.100.2:12345", "203.0.113.10", "198.51.100.2"),
        ] {
            let req = actix_test::TestRequest::default()
                .peer_addr(peer.parse().unwrap())
                .insert_header(("X-Forwarded-For", forwarded))
                .to_srv_request();
            assert_eq!(
                extractor.extract_key(&req).unwrap(),
                expected.parse::<IpAddr>().unwrap(),
                "peer={peer}, forwarded={forwarded}"
            );
        }
    }

    #[actix_web::test]
    async fn trusted_proxy_extractor_falls_back_for_invalid_forwarded_ip_or_missing_peer() {
        use crate::actix::rate_limit::TrustedProxyIpKeyExtractor;
        let extractor = TrustedProxyIpKeyExtractor::new(&["10.0.0.0/8".to_string()]);
        let invalid_forwarded = actix_test::TestRequest::default()
            .peer_addr("10.0.0.5:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", "not-an-ip"))
            .to_srv_request();

        assert_eq!(
            extractor.extract_key(&invalid_forwarded).unwrap(),
            "10.0.0.5".parse::<IpAddr>().unwrap()
        );

        let missing_peer = actix_test::TestRequest::default().to_srv_request();
        assert_eq!(
            extractor.extract_key(&missing_peer).unwrap(),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );

        let loopback_trusted = TrustedProxyIpKeyExtractor::new(&["127.0.0.1".into()]);
        let missing_peer = actix_test::TestRequest::default()
            .insert_header(("x-forwarded-for", "203.0.113.1"))
            .to_srv_request();
        assert_eq!(
            loopback_trusted.extract_key(&missing_peer).unwrap(),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }
}
