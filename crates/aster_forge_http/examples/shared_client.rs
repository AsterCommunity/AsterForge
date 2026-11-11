//! Caller-owned lazy transport for a batch CLI or an embedded service.
//! Run without arguments for an offline command, or pass URLs for a batch.
//! HTTPS additionally requires an explicit TLS feature (see the crate README).
use aster_forge_http::HttpClient;
use std::{sync::Arc, time::Duration};
use tokio::sync::OnceCell;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;

// This is application state, not a process-global pool or a new Forge wrapper type.
#[derive(Default)]
struct AppState {
    http: Arc<OnceCell<HttpClient>>,
}

struct Session {
    http: Arc<OnceCell<HttpClient>>,
    user_agent: String,
    timeout: Duration,
}

impl Session {
    fn new(app: &AppState, user_agent: String, timeout: Duration) -> Self {
        Self {
            http: app.http.clone(),
            user_agent,
            timeout,
        }
    }

    async fn fetch(&self, url: &str) -> Result<usize, Box<dyn std::error::Error>> {
        let client = self
            .http
            .get_or_try_init(|| async {
                // Configure TLS/roots/proxy here, once per compatible trust boundary.
                // Do not capture a session's identity, credentials or deadline here.
                HttpClient::builder()
                    .user_agent("ExampleProduct/1.0")
                    .build()
            })
            .await?;
        let response = client
            .get(url)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .timeout(self.timeout)
            .send()
            .await?;
        let body = aster_forge_http::read_reqwest_body_limited(
            response,
            "example response",
            1024 * 1024,
            std::io::Error::other,
        )
        .await?;
        Ok(body.len())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = AppState::default();
    // Batch CLI: one state for the whole command, not one state per item.
    // No URLs means a local-only command: the HTTP client is never initialized.
    for (index, url) in std::env::args().skip(1).enumerate() {
        let session = Session::new(
            &app,
            format!("ExampleProduct/session-{index}"),
            Duration::from_secs(15),
        );
        println!("{} bytes", session.fetch(&url).await?);
    }
    // Embedded service: retain AppState for the service lifetime, create Sessions
    // from it for incoming jobs, and call fetch concurrently as needed. A second
    // AppState isolates another proxy/TLS trust boundary. No global singleton.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn local_sessions_do_not_initialize_http() {
        let app = AppState::default();
        let _session = Session::new(&app, "offline".into(), Duration::from_secs(1));
        assert!(app.http.get().is_none());
    }

    #[tokio::test]
    async fn concurrent_initialization_builds_once_and_failure_can_retry() {
        support::initialize_tls();
        let app = AppState::default();
        let attempts = AtomicUsize::new(0);
        let initialize = || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            HttpClient::builder().no_proxy().build()
        };
        let failed = app.http.get_or_try_init(|| async {
            HttpClient::builder()
                .user_agent("invalid\nidentity")
                .build()
        });
        assert!(failed.await.is_err());
        assert!(app.http.get().is_none());
        let (first, second, third) = tokio::join!(
            app.http.get_or_try_init(initialize),
            app.http.get_or_try_init(initialize),
            app.http.get_or_try_init(initialize),
        );
        let (first, second, third) = (first.unwrap(), second.unwrap(), third.unwrap());
        assert!(std::ptr::eq(first, second));
        assert!(std::ptr::eq(second, third));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_initialization_can_retry() {
        support::initialize_tls();
        let app = AppState::default();
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            app.http.get_or_try_init(|| async {
                std::future::pending::<Result<HttpClient, reqwest::Error>>().await
            }),
        )
        .await;
        assert!(result.is_err());
        assert!(app.http.get().is_none());
        assert!(
            app.http
                .get_or_try_init(|| async { HttpClient::builder().no_proxy().build() })
                .await
                .is_ok()
        );
    }
}
