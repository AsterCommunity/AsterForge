use forge_release::registry::{CratesIoRegistry, Registry};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
fn serve(status: &str, body: &str) -> (CratesIoRegistry, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = [0; 4096];
        let n = socket.read(&mut bytes).unwrap();
        socket.write_all(response.as_bytes()).unwrap();
        String::from_utf8_lossy(&bytes[..n]).into_owned()
    });
    (
        CratesIoRegistry::new(format!("http://{addr}/api/v1/crates")).unwrap(),
        handle,
    )
}
#[test]
fn valid_versions_yanked_state_and_cache() {
    let (registry, server) = serve(
        "200 OK",
        r#"{"versions":[{"num":"1.0.0","yanked":false},{"num":"1.0.1","yanked":true}]}"#,
    );
    let versions = registry.versions("test_crate").unwrap();
    assert_eq!(versions.0.len(), 2);
    assert_eq!(versions.1.len(), 1);
    assert_eq!(registry.versions("test_crate").unwrap(), versions);
    assert!(
        server
            .join()
            .unwrap()
            .starts_with("GET /api/v1/crates/test_crate ")
    );
}
#[test]
fn only_404_means_a_new_crate() {
    let (registry, server) = serve("404 Not Found", "{}");
    assert!(registry.versions("new").unwrap().0.is_empty());
    server.join().unwrap();
    for status in [
        "403 Forbidden",
        "429 Too Many Requests",
        "500 Internal Server Error",
        "302 Found",
    ] {
        let (registry, server) = serve(status, "{}");
        assert!(registry.versions("test").is_err(), "{status}");
        server.join().unwrap();
    }
}
#[test]
fn malformed_empty_and_invalid_version_payloads_fail_closed() {
    for body in [
        "not JSON",
        r#"{"versions":[]}"#,
        r#"{"versions":[{"num":"nonsense","yanked":false}]}"#,
        r#"{"unexpected":[]}"#,
    ] {
        let (registry, server) = serve("200 OK", body);
        assert!(registry.versions("test").is_err(), "{body}");
        server.join().unwrap();
    }
}
#[test]
fn reject_insecure_urls_and_package_path_injection() {
    for url in [
        "http://crates.io/api/v1/crates",
        "file:///tmp/test",
        "nonsense",
        "https://crates.io/api/v1/crates?foo=bar",
    ] {
        assert!(CratesIoRegistry::new(url).is_err(), "{url}");
    }
    let registry = CratesIoRegistry::new("https://crates.io/api/v1/crates").unwrap();
    for name in ["", "../users", "bad?token=secret", "has space"] {
        assert!(registry.versions(name).is_err());
    }
}
