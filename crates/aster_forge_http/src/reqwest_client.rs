use super::read_reqwest_body_limited;

/// Errors produced while adapting a buffered `http` request to reqwest.
#[derive(Debug, thiserror::Error)]
pub enum BufferedHttpError {
    /// The request could not be converted or sent.
    #[error("outbound HTTP transport failed")]
    Transport(#[source] reqwest::Error),
    /// The response body could not be read within the configured bound.
    #[error("outbound HTTP response body failed: {0}")]
    ResponseBody(String),
    /// The buffered response could not be assembled.
    #[error("outbound HTTP response could not be assembled")]
    ResponseBuild(#[source] http::Error),
}

/// Executes an in-memory HTTP request with reqwest and buffers a strictly bounded response body.
///
/// Redirect and timeout behavior come from the supplied reqwest client. Callers retain ownership of
/// endpoint policy, user agent, status handling, and product error mapping.
///
/// # Errors
///
/// Returns a transport error when the request cannot be converted or sent, a response-body error
/// when the configured limit is exceeded or streaming fails, or a response-build error when the
/// buffered response cannot be reconstructed.
pub async fn execute_reqwest_buffered_limited(
    client: &reqwest::Client,
    request: http::Request<Vec<u8>>,
    max_response_bytes: usize,
) -> Result<http::Response<Vec<u8>>, BufferedHttpError> {
    let request = reqwest::Request::try_from(request).map_err(BufferedHttpError::Transport)?;
    let response = client
        .execute(request)
        .await
        .map_err(BufferedHttpError::Transport)?;
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let body = read_reqwest_body_limited(
        response,
        "outbound HTTP response body",
        max_response_bytes,
        BufferedHttpError::ResponseBody,
    )
    .await?;

    let mut builder = http::Response::builder().status(status).version(version);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    builder.body(body).map_err(BufferedHttpError::ResponseBuild)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{BufferedHttpError, execute_reqwest_buffered_limited};

    async fn spawn_response(response: &'static [u8]) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should expose address");
        tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .expect("test server should accept request");
            let mut buffer = [0_u8; 1024];
            let _ = socket
                .read(&mut buffer)
                .await
                .expect("test server should read request");
            socket
                .write_all(response)
                .await
                .expect("test server should write response");
        });
        address
    }

    #[tokio::test]
    async fn buffered_client_preserves_status_headers_and_body() {
        let address = spawn_response(
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 4\r\nX-Test: yes\r\nConnection: close\r\n\r\nbody",
        )
        .await;
        let request = http::Request::get(format!("http://{address}/token"))
            .body(Vec::new())
            .expect("test request should build");

        let response = execute_reqwest_buffered_limited(&reqwest::Client::new(), request, 4)
            .await
            .expect("bounded request should succeed");

        assert_eq!(response.status(), http::StatusCode::ACCEPTED);
        assert_eq!(response.headers()["x-test"], "yes");
        assert_eq!(response.body(), b"body");
    }

    #[tokio::test]
    async fn buffered_client_rejects_chunked_body_over_limit() {
        let address = spawn_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n14\r\nsecret-response-body\r\n0\r\n\r\n",
        )
        .await;
        let request = http::Request::get(format!("http://{address}/discovery"))
            .body(Vec::new())
            .expect("test request should build");

        let error = execute_reqwest_buffered_limited(&reqwest::Client::new(), request, 4)
            .await
            .expect_err("oversized body should fail");

        assert!(matches!(error, BufferedHttpError::ResponseBody(_)));
        assert!(error.to_string().contains("exceeds 4 bytes limit"));
        assert!(!error.to_string().contains("secret-response-body"));
        assert!(!format!("{error:?}").contains("secret-response-body"));
    }
}
