//! Shared reusable Redis container for integration tests.
//!
//! The container is shared by suite name and reused across runs, so data persists between test
//! processes. Tests should use unique key prefixes or clean up after themselves.

use crate::state::{ContainerLease, ContainerStateLock};
use crate::suite::TestContainerSuite;
use crate::wait::wait_until;
use aster_forge_utils::url::url_with_credentials;
use std::net::TcpListener;
use std::time::Duration;
use testcontainers::core::{ContainerAsync, IntoContainerPort};
use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

/// Handle to the suite's shared Redis container.
pub struct RedisTestContainer {
    url: String,
    container: ContainerAsync<GenericImage>,
    _lease: ContainerLease,
}

/// Isolated Redis container configured with a caller-provided raw password.
///
/// Unlike [`RedisTestContainer`], this fixture is not shared or reused because authentication is
/// process-wide Redis state. The exposed base URL never contains the password.
pub struct AuthenticatedRedisTestContainer {
    base_url: String,
    _container: ContainerAsync<GenericImage>,
}

impl AuthenticatedRedisTestContainer {
    /// Starts an isolated Redis server that requires `password`.
    ///
    /// # Panics
    ///
    /// Panics when the container or port cannot be created, the credential URL is invalid, or
    /// Redis does not become ready before the timeout.
    pub async fn start(password: &str) -> Self {
        let container = GenericImage::new("redis", "7-alpine")
            .with_exposed_port(IntoContainerPort::tcp(6379))
            .with_cmd(["redis-server", "--requirepass", password])
            .start()
            .await
            .expect("failed to start authenticated Redis test container");
        let port = container
            .get_host_port_ipv4(IntoContainerPort::tcp(6379))
            .await
            .expect("authenticated Redis test port should be exposed");
        let base_url = format!("redis://127.0.0.1:{port}/0");
        let credential_url = url_with_credentials(
            &base_url,
            None,
            Some(password),
            "authenticated Redis test container base URL",
        )
        .expect("authenticated Redis test URL should accept credentials");
        let client = redis::Client::open(credential_url)
            .unwrap_or_else(|_| panic!("failed to build authenticated Redis readiness client"));
        wait_for_redis(&client, "authenticated Redis test container").await;

        Self {
            base_url,
            _container: container,
        }
    }

    /// Returns the Redis base URL without userinfo.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl RedisTestContainer {
    /// Starts (or reuses) the shared Redis container and waits for it to accept connections.
    ///
    /// # Panics
    ///
    /// Panics when shared state, port reservation, container startup, endpoint construction, or
    /// Redis readiness fails.
    pub async fn start(suite: &TestContainerSuite) -> Self {
        // Keep the host port fixed across stop/start. Docker assigns a new ephemeral port to a
        // container whose mapping leaves HostPort empty, stranding already-running processes on
        // the old Redis endpoint after a restart.
        let lock = ContainerStateLock::acquire(suite, "redis-fixed");
        let mut state = lock.load();
        let _ = state.prune_stale_before_current_execution();
        state.register_current_process();
        lock.save(&state);
        let host_port = TcpListener::bind(("127.0.0.1", 0))
            .expect("reserve Redis test host port")
            .local_addr()
            .expect("resolve Redis test host port")
            .port();

        let container = GenericImage::new("redis", "7-alpine")
            .with_mapped_port(host_port, IntoContainerPort::tcp(6379))
            .with_container_name(suite.container_name("redis-fixed"))
            .with_reuse(ReuseDirective::Always)
            .start()
            .await
            .expect("failed to start Redis test container");
        let port = container
            .get_host_port_ipv4(IntoContainerPort::tcp(6379))
            .await
            .expect("Redis test port should be exposed");

        let url = format!("redis://127.0.0.1:{port}/0");
        let client = redis::Client::open(url.as_str())
            .unwrap_or_else(|_| panic!("failed to build Redis readiness client"));
        wait_for_redis(&client, "Redis test container").await;
        drop(lock);

        Self {
            url,
            container,
            _lease: ContainerLease::new(suite.clone(), "redis-fixed"),
        }
    }

    /// Returns the Redis URL, for example `redis://127.0.0.1:6379/0`.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Stops Redis immediately to simulate a broker outage.
    ///
    /// # Panics
    ///
    /// Panics when the container runtime fails to stop Redis.
    pub async fn stop(&self) {
        self.container
            .stop_with_timeout(Some(0))
            .await
            .expect("failed to stop Redis test container");
    }

    /// Restarts a previously stopped Redis container.
    ///
    /// # Panics
    ///
    /// Panics when restart, client construction, or the Redis readiness check fails.
    pub async fn restart(&self) {
        self.container
            .start()
            .await
            .expect("failed to restart Redis test container");
        let client = redis::Client::open(self.url.as_str())
            .unwrap_or_else(|_| panic!("failed to build restarted Redis readiness client"));
        wait_for_redis(&client, "restarted Redis test container").await;
    }
}

async fn wait_for_redis(client: &redis::Client, context: &str) {
    let ready = wait_until(
        Duration::from_secs(90),
        Duration::from_millis(250),
        || async {
            let Ok(mut connection) = client.get_multiplexed_async_connection().await else {
                return false;
            };
            redis::cmd("PING")
                .query_async::<String>(&mut connection)
                .await
                .is_ok_and(|response| response == "PONG")
        },
    )
    .await;
    assert!(ready, "{context} did not answer PING before timeout");
}
