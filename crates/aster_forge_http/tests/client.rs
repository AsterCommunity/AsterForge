use aster_forge_http::{HttpClient, HttpObservation, HttpObserver, HttpOutcome};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct Events(Mutex<Vec<HttpObservation>>);
impl HttpObserver for Events {
    fn observe(&self, event: HttpObservation) {
        self.0.lock().unwrap().push(event);
    }
}
async fn server(
    response: &'static [u8],
    delay: Duration,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4096];
        let count = socket.read(&mut bytes).await.unwrap();
        tokio::time::sleep(delay).await;
        let _ = socket.write_all(response).await;
        String::from_utf8_lossy(&bytes[..count]).to_string()
    });
    (url, task)
}
#[tokio::test]
async fn shared_factory_preserves_signed_request_and_never_follows_redirects() {
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let response: &'static [u8] = Box::leak(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",target.local_addr().unwrap()).into_bytes().into_boxed_slice());
    let (url, received) = server(response, Duration::ZERO).await;
    let events = Arc::new(Events::default());
    let client = HttpClient::builder()
        .user_agent("Product/42")
        .no_proxy()
        .observer(events.clone())
        .build()
        .unwrap();
    let request = client
        .post(format!("{url}/signed?token=never-log"))
        .header("x-signature", "signed-by-provider")
        .header("range", "bytes=1-3")
        .body("opaque-body")
        .build()
        .unwrap();
    let response = client.execute(request).await.unwrap();
    assert_eq!(response.status(), 307);
    let request = received.await.unwrap().to_ascii_lowercase();
    assert!(request.contains("user-agent: product/42"));
    assert!(request.contains("x-signature: signed-by-provider"));
    assert!(request.contains("range: bytes=1-3"));
    assert!(request.ends_with("opaque-body"));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), target.accept())
            .await
            .is_err()
    );
    let events = events.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status, Some(307));
    assert_eq!(events[0].outcome, HttpOutcome::Response);
    assert!(!format!("{:?}", events[0]).contains("never-log"));
}
#[tokio::test]
async fn observer_distinguishes_timeout_and_dropped_request_cancellation() {
    let events = Arc::new(Events::default());
    let (url, task) = server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        Duration::from_millis(100),
    )
    .await;
    let client = HttpClient::builder()
        .no_proxy()
        .timeout(Duration::from_millis(20))
        .observer(events.clone())
        .build()
        .unwrap();
    assert!(client.get(url).send().await.unwrap_err().is_timeout());
    task.await.unwrap();
    assert_eq!(events.0.lock().unwrap()[0].outcome, HttpOutcome::Timeout);
    let (url, task) = server(b"", Duration::from_millis(100)).await;
    let client = HttpClient::builder()
        .no_proxy()
        .observer(events.clone())
        .build()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), client.get(url).send())
            .await
            .is_err()
    );
    assert_eq!(events.0.lock().unwrap()[1].outcome, HttpOutcome::Cancelled);
    task.await.unwrap();
}
#[tokio::test]
async fn cloned_clients_reuse_one_pool_and_observe_each_exchange_once() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        for index in 0..2 {
            let mut bytes = [0; 1024];
            let len = socket.read(&mut bytes).await.unwrap();
            assert!(len > 0);
            let request = String::from_utf8_lossy(&bytes[..len]).to_ascii_lowercase();
            assert_eq!(
                request.contains("authorization: bearer per-request"),
                index == 0
            );
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        }
    });
    let events = Arc::new(Events::default());
    let client = HttpClient::builder()
        .no_proxy()
        .observer(events.clone())
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("per-request")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        client
            .clone()
            .get(&url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(events.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn request_builders_preserve_method_headers_encoding_and_request_deadline() {
    let client = HttpClient::builder().no_proxy().build().unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-provider-version", "one".parse().unwrap());
    let json = client
        .put("https://provider.invalid/object")
        .headers(headers)
        .json(&serde_json::json!({"name":"a b"}))
        .query(&[("key", "a/b +")])
        .basic_auth("user", Some("password"))
        .timeout(Duration::from_secs(2))
        .version(http::Version::HTTP_11);
    let cloned = json.try_clone().unwrap().build().unwrap();
    let request = json.build().unwrap();
    assert_eq!(request.method(), reqwest::Method::PUT);
    assert_eq!(request.url().query(), Some("key=a%2Fb+%2B"));
    assert_eq!(request.headers()["x-provider-version"], "one");
    assert_eq!(request.headers()["content-type"], "application/json");
    assert_eq!(
        request.headers()["authorization"],
        "Basic dXNlcjpwYXNzd29yZA=="
    );
    assert_eq!(
        request.body().unwrap().as_bytes().unwrap(),
        br#"{"name":"a b"}"#
    );
    assert_eq!(
        request.body().unwrap().as_bytes(),
        cloned.body().unwrap().as_bytes()
    );
    assert_eq!(request.timeout(), Some(&Duration::from_secs(2)));
    assert_eq!(request.version(), http::Version::HTTP_11);
    let form = client
        .patch("https://provider.invalid/token")
        .form(&[("code", "a+b &")])
        .build()
        .unwrap();
    assert_eq!(form.method(), reqwest::Method::PATCH);
    assert_eq!(
        form.headers()["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(form.body().unwrap().as_bytes().unwrap(), b"code=a%2Bb+%26");
    assert_eq!(
        client
            .head("https://provider.invalid/object")
            .build()
            .unwrap()
            .method(),
        reqwest::Method::HEAD
    );
    assert_eq!(
        client
            .delete("https://provider.invalid/object")
            .build()
            .unwrap()
            .method(),
        reqwest::Method::DELETE
    );
    // Debug must never expose a default credential or a raw pool.
    assert_eq!(format!("{client:?}"), "HttpClient { .. }");
}

#[tokio::test]
async fn streamed_multipart_is_not_clonable_and_keeps_boundary_body_and_auth() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let len = socket.read(&mut chunk).await.unwrap();
            assert!(len > 0);
            bytes.extend_from_slice(&chunk[..len]);
            if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                let source = String::from_utf8_lossy(&bytes);
                let header_end = source.find("\r\n\r\n").unwrap() + 4;
                let length = source[..header_end]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap();
                if bytes.len() >= header_end + length {
                    break;
                }
            }
        }
        socket
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8(bytes).unwrap()
    });
    let client = HttpClient::builder().no_proxy().build().unwrap();
    let form = reqwest::multipart::Form::new()
        .text("key", "logical/object")
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"file-payload".to_vec()).file_name("file.txt"),
        );
    let request = client
        .post(url)
        .multipart(form)
        .bearer_auth("request-token");
    assert!(request.try_clone().is_none());
    assert_eq!(request.send().await.unwrap().status(), 201);
    let wire = task.await.unwrap();
    assert!(wire.contains("multipart/form-data; boundary="));
    assert!(wire.contains("name=\"key\""));
    assert!(wire.contains("logical/object"));
    assert!(wire.contains("file-payload"));
    assert!(wire.contains("Bearer request-token"));
}

#[tokio::test]
async fn preparation_connect_and_protocol_failures_have_distinct_observation_boundaries() {
    let events = Arc::new(Events::default());
    let client = HttpClient::builder()
        .no_proxy()
        .observer(events.clone())
        .build()
        .unwrap();
    // Request construction never admitted an exchange: no transport event.
    assert!(client.get("invalid URL").send().await.is_err());
    assert!(events.0.lock().unwrap().is_empty());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(client.get(url).send().await.unwrap_err().is_connect());
    assert_eq!(events.0.lock().unwrap()[0].outcome, HttpOutcome::Connect);
    let (url, task) = server(b"not-an-http-response\r\n\r\n", Duration::ZERO).await;
    assert!(client.get(url).send().await.is_err());
    task.await.unwrap();
    assert_eq!(events.0.lock().unwrap()[1].outcome, HttpOutcome::Transport);
    assert_eq!(events.0.lock().unwrap()[1].status, None);
}

#[tokio::test]
async fn request_timeout_overrides_the_pool_deadline() {
    let (url, task) = server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        Duration::from_millis(100),
    )
    .await;
    let client = HttpClient::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    assert!(
        client
            .get(url)
            .timeout(Duration::from_millis(20))
            .send()
            .await
            .unwrap_err()
            .is_timeout()
    );
    task.await.unwrap();
}
