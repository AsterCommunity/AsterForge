//! Trusted-proxy-aware HTTP header adapter. Direct peers come from the connection, never headers.
use axum::extract::{ConnectInfo, Request};
use http::HeaderMap;
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};

/// Resolves a client IP using raw trusted CIDRs/IPs and an explicit direct peer.
#[must_use]
pub fn real_ip_from_headers(
    headers: &HeaderMap,
    peer: IpAddr,
    trusted_proxies: &[String],
) -> IpAddr {
    real_ip_from_trusted_headers(
        headers,
        peer,
        &aster_forge_utils::net::parse_trusted_proxies(trusted_proxies),
    )
}

/// Resolves a client IP using parsed trusted proxies. Invalid forwarded values fall back to peer.
#[must_use]
pub fn real_ip_from_trusted_headers(
    headers: &HeaderMap,
    peer: IpAddr,
    trusted: &[IpNet],
) -> IpAddr {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok());
    aster_forge_utils::net::real_ip_from_forwarded_for(forwarded, peer, trusted)
}

/// Reads the TCP peer populated by `into_make_service_with_connect_info::<SocketAddr>()`.
/// UDS or services without connection info return `None`; headers cannot fill this gap.
#[must_use]
pub fn direct_peer(request: &Request) -> Option<IpAddr> {
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip())
}

/// Resolves the request's client IP, retaining `None` when connection info is missing.
#[must_use]
pub fn client_ip(request: &Request, trusted: &[IpNet]) -> Option<IpAddr> {
    direct_peer(request).map(|peer| real_ip_from_trusted_headers(request.headers(), peer, trusted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{Body, to_bytes},
        middleware,
        response::IntoResponse,
        routing::get,
    };
    use http::{HeaderValue, Request as HttpRequest};
    use tower::ServiceExt;

    #[tokio::test]
    async fn router_resolves_only_connection_proven_trusted_peers() {
        let trusted = aster_forge_utils::net::parse_trusted_proxies(&[
            "10.0.0.0/8".into(),
            "192.168.1.1".into(),
            "2001:db8:ffff::/48".into(),
            "127.0.0.1".into(),
            "bad-entry".into(),
        ]);
        let router =
            Router::new()
                .route("/", get(|| async { "unused" }))
                .layer(middleware::from_fn(
                    move |request: Request, _next: axum::middleware::Next| {
                        let trusted = trusted.clone();
                        async move {
                            client_ip(&request, &trusted)
                                .map_or_else(|| "none".into(), |ip| ip.to_string())
                                .into_response()
                        }
                    },
                ));
        for (peer, forwarded, expected) in [
            (
                Some("10.0.0.5:1234"),
                "203.0.113.10, 198.51.100.2",
                "203.0.113.10",
            ),
            (Some("192.168.1.1:1234"), "203.0.113.10:443", "203.0.113.10"),
            (
                Some("[2001:db8:ffff::1]:1234"),
                "[2001:db8::1]:443",
                "2001:db8::1",
            ),
            (Some("10.0.0.5:1234"), "2001:db8::1", "2001:db8::1"),
            (Some("198.51.100.2:1234"), "203.0.113.10", "198.51.100.2"),
            (Some("10.0.0.5:1234"), "not-an-ip", "10.0.0.5"),
            (Some("10.0.0.5:1234"), "", "10.0.0.5"),
            (None, "203.0.113.10", "none"),
        ] {
            let mut request = HttpRequest::builder()
                .header("x-forwarded-for", forwarded)
                .body(Body::empty())
                .unwrap();
            if let Some(peer) = peer {
                request
                    .extensions_mut()
                    .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
            }
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(
                to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
                expected.as_bytes()
            );
        }
    }

    #[test]
    fn opaque_forwarded_header_and_absent_headers_fall_back_to_peer() {
        let mut headers = HeaderMap::new();
        let peer: IpAddr = "10.0.0.5".parse().unwrap();
        let trusted = vec!["10.0.0.0/8".into()];
        assert_eq!(real_ip_from_headers(&headers, peer, &trusted), peer);
        headers.insert("x-forwarded-for", HeaderValue::from_bytes(&[0xff]).unwrap());
        assert_eq!(real_ip_from_headers(&headers, peer, &trusted), peer);
    }
}
