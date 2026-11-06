use aster_forge_http::HttpClient;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn origin() -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 2048];
        let len = socket.read(&mut bytes).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes[..len]).to_string()
    });
    (addr, task)
}
#[tokio::test]
async fn explicit_proxy_and_pinned_dns_have_an_order_independent_boundary() {
    // An HTTP forward proxy receives the absolute target, without needing target DNS.
    let (proxy, task) = origin().await;
    let client = HttpClient::builder()
        .proxy(reqwest::Proxy::all(format!("http://{proxy}")).unwrap())
        .build()
        .unwrap();
    assert_eq!(
        client
            .get("http://provider.invalid/proxy-target")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    assert!(
        task.await
            .unwrap()
            .starts_with("GET http://provider.invalid/proxy-target HTTP/1.1")
    );
    for pin_first in [false, true] {
        let (addr, task) = origin().await;
        let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = reqwest::Proxy::all(format!("http://{}", proxy_listener.local_addr().unwrap()))
            .unwrap();
        let builder = HttpClient::builder()
            .connect_timeout(Duration::from_secs(1))
            .pool_idle_timeout(Duration::from_secs(2))
            .pool_max_idle_per_host(1);
        let builder = if pin_first {
            builder
                .resolve_to_addrs("provider.invalid", &[addr])
                .proxy(proxy)
        } else {
            builder
                .proxy(proxy)
                .resolve_to_addrs("provider.invalid", &[addr])
        };
        let client = builder.build().unwrap();
        assert_eq!(
            client
                .get(format!("http://provider.invalid:{}/pinned", addr.port()))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        assert!(task.await.unwrap().starts_with("GET /pinned HTTP/1.1"));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), proxy_listener.accept())
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn idle_read_timer_does_not_become_a_whole_download_deadline() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 1024];
        assert!(socket.read(&mut buf).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        for byte in b"body" {
            tokio::time::sleep(Duration::from_millis(40)).await;
            socket.write_all(&[*byte]).await.unwrap();
        }
    });
    let client = HttpClient::builder()
        .no_proxy()
        .without_total_timeout()
        .read_timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    assert_eq!(
        client.get(url).send().await.unwrap().text().await.unwrap(),
        "body"
    );
    task.await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 1024];
        assert!(socket.read(&mut buf).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
    });
    assert!(
        client
            .get(url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap_err()
            .is_timeout()
    );
    task.await.unwrap();
    // Upload protocols can explicitly disable the header-read timer and own progress cancellation.
    let _client = HttpClient::builder()
        .without_read_timeout()
        .without_total_timeout()
        .build()
        .unwrap();
}

fn certificate() -> (
    reqwest::Certificate,
    Arc<tokio_rustls::rustls::ServerConfig>,
) {
    use tokio_rustls::rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    };
    let cert = rcgen::generate_simple_self_signed(vec!["provider.test".into()]).unwrap();
    let root = reqwest::Certificate::from_der(cert.cert.der()).unwrap();
    let key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let config = ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(cert.cert.der().to_vec())],
        key.into(),
    )
    .unwrap();
    (root, Arc::new(config))
}
#[tokio::test]
async fn added_trust_roots_preserve_certificate_and_hostname_validation() {
    let (root, config) = certificate();
    for (trusted, host, expected) in [
        (false, "provider.test", false),
        (true, "provider.test", true),
        (true, "wrong.test", false),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(config.clone());
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            if let Ok(mut tls) = acceptor.accept(socket).await {
                let mut bytes = [0; 1024];
                assert!(tls.read(&mut bytes).await.unwrap() > 0);
                tls.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .await
                .unwrap();
                tls.shutdown().await.unwrap();
            }
        });
        let builder = HttpClient::builder().resolve_to_addrs(host, &[addr]);
        let builder = if trusted {
            builder.add_root_certificate(root.clone())
        } else {
            builder
        };
        let response = builder
            .build()
            .unwrap()
            .get(format!("https://{host}:{}/", addr.port()))
            .send()
            .await;
        assert_eq!(response.is_ok(), expected, "trusted={trusted} host={host}");
        if let Ok(response) = response {
            assert_eq!(response.text().await.unwrap(), "ok");
        }
        task.await.unwrap();
    }
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_factory_preserves_proxy_dns_tls_root_and_no_redirect_contracts() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let (root, _config) = certificate();
    for pin_first in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let task = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0; 2048];
            let len = socket.read(&mut bytes).unwrap();
            socket.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://provider.invalid/never-follow\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            String::from_utf8_lossy(&bytes[..len]).to_string()
        });
        let proxy = reqwest::Proxy::all("http://127.0.0.1:1").unwrap();
        let builder = HttpClient::builder()
            .user_agent("BlockingProduct/42")
            .connect_timeout(Duration::from_secs(1))
            .without_total_timeout()
            .pool_idle_timeout(Duration::from_secs(1))
            .pool_max_idle_per_host(1)
            .add_root_certificate(root.clone());
        let builder = if pin_first {
            builder
                .resolve_to_addrs("provider.invalid", &[addr])
                .proxy(proxy)
        } else {
            builder
                .proxy(proxy)
                .resolve_to_addrs("provider.invalid", &[addr])
        };
        let client = builder.build_blocking().unwrap();
        assert_eq!(
            client
                .get(format!("http://provider.invalid:{}/", addr.port()))
                .send()
                .unwrap()
                .status(),
            307
        );
        assert!(task.join().unwrap().contains("BlockingProduct/42"));
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let proxy = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut bytes = [0; 2048];
        let len = socket.read(&mut bytes).unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        String::from_utf8_lossy(&bytes[..len]).to_string()
    });
    let client = HttpClient::builder()
        .proxy(reqwest::Proxy::all(format!("http://{addr}")).unwrap())
        .timeout(Duration::from_secs(1))
        .build_blocking()
        .unwrap();
    assert_eq!(
        client
            .get("http://provider.invalid/through-proxy")
            .send()
            .unwrap()
            .status(),
        200
    );
    assert!(
        proxy
            .join()
            .unwrap()
            .starts_with("GET http://provider.invalid/through-proxy HTTP/1.1")
    );
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_default_identity_and_invalid_identity_are_explicit() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let task = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut bytes = [0; 2048];
        let len = socket.read(&mut bytes).unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        String::from_utf8_lossy(&bytes[..len]).to_string()
    });
    let client = HttpClient::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build_blocking()
        .unwrap();
    assert_eq!(
        client
            .get(format!("http://{addr}/"))
            .send()
            .unwrap()
            .status(),
        200
    );
    assert!(task.join().unwrap().contains("aster_forge_http/"));
    assert!(
        HttpClient::builder()
            .user_agent("bad\r\nidentity")
            .build_blocking()
            .is_err()
    );
    assert!(
        HttpClient::builder()
            .user_agent("bad\r\nidentity")
            .build()
            .is_err()
    );
}
