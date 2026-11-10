use aster_forge_http::HttpClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Provider installation belongs to the application entrypoint, never a library singleton.
    #[cfg(any(feature = "rustls", feature = "rustls-ring"))]
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| "conflicting rustls provider")?;
    let builder = HttpClient::builder().no_proxy();
    #[cfg(any(
        feature = "rustls",
        feature = "rustls-aws-lc-rs",
        feature = "rustls-ring"
    ))]
    let builder = builder.tls_backend(aster_forge_http::TlsBackend::Rustls);
    #[cfg(all(feature = "native-tls", not(feature = "rustls-ring")))]
    let builder = builder.tls_backend(aster_forge_http::TlsBackend::Native);
    let client = builder.build()?;
    let request = client
        .post("http://example.invalid/")
        .body("bytes")
        .build()?;
    assert_eq!(
        request.body().and_then(reqwest::Body::as_bytes),
        Some(b"bytes".as_slice())
    );
    #[cfg(feature = "json")]
    assert_eq!(
        client
            .post("http://example.invalid/")
            .json(&[1, 2])
            .build()?
            .headers()["content-type"],
        "application/json"
    );
    #[cfg(feature = "form")]
    assert_eq!(
        client
            .post("http://example.invalid/")
            .form(&[("key", "a b")])
            .build()?
            .body()
            .and_then(reqwest::Body::as_bytes),
        Some(b"key=a+b".as_slice())
    );
    #[cfg(feature = "query")]
    assert_eq!(
        client
            .get("http://example.invalid/")
            .query(&[("key", "a b")])
            .build()?
            .url()
            .query(),
        Some("key=a+b")
    );
    #[cfg(feature = "multipart")]
    assert!(
        client
            .post("http://example.invalid/")
            .multipart(reqwest::multipart::Form::new().text("key", "value"))
            .build()?
            .headers()["content-type"]
            .to_str()?
            .starts_with("multipart/form-data; boundary=")
    );
    #[cfg(feature = "stream")]
    {
        let body =
            reqwest::Body::wrap_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(
                vec![1, 2],
            )]));
        assert!(
            client
                .post("http://example.invalid/")
                .body(body)
                .build()?
                .body()
                .unwrap()
                .as_bytes()
                .is_none()
        );
    }
    #[cfg(feature = "blocking")]
    let _client = HttpClient::builder().no_proxy().build_blocking()?;
    Ok(())
}
