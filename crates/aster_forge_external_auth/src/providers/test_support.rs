//! Shared transport fixtures; each provider test must construct its own adapter first.
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Accept a request without responding and verify the injected deadline, always joining cleanup.
pub(super) async fn assert_injected_timeout(client: aster_forge_http::HttpClient) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (received_tx, mut received_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener
            .accept()
            .await
            .expect("timeout fixture must accept a connection");
        let mut bytes = [0; 2048];
        let len = socket
            .read(&mut bytes)
            .await
            .expect("timeout fixture must read a request");
        assert!(len > 0, "timeout fixture received EOF before request bytes");
        received_tx
            .send(())
            .expect("timeout fixture receiver must remain alive");
        std::future::pending::<()>().await;
        drop(socket);
    });
    // This generous guard detects a lost client deadline; it is never accepted as a client timeout.
    let result = tokio::time::timeout(Duration::from_secs(2), client.get(url).send()).await;
    // Snapshot receipt before cleanup without waiting on a server that may never have received I/O.
    let received = received_rx.try_recv();
    server.abort();
    let server_result = server.await;
    assert!(
        matches!(&server_result, Err(error) if error.is_cancelled()),
        "timeout fixture failed before cancellation: {server_result:?}; received={received:?}"
    );
    received.expect("fixture must receive request bytes before the injected client deadline");
    let error = result
        .expect("injected deadline must fire before the outer test guard")
        .expect_err("the fixture never sends headers, so a response cannot succeed");
    assert!(
        error.is_timeout(),
        "expected the injected transport timeout: {error}"
    );
}
