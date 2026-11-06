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
