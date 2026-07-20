//! Shared reusable Redis container for integration tests.
//!
//! The container is shared by suite name and reused across runs, so data persists between test
//! processes. Tests should use unique key prefixes or clean up after themselves.

use crate::state::{ContainerLease, ContainerStateLock};
use crate::suite::TestContainerSuite;
use crate::wait::wait_until;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;
use testcontainers::core::{ContainerAsync, IntoContainerPort, WaitFor};
use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

/// Handle to the suite's shared Redis container.
pub struct RedisTestContainer {
    url: String,
    address: SocketAddr,
    _container: ContainerAsync<GenericImage>,
    _lease: ContainerLease,
}

impl RedisTestContainer {
    /// Starts (or reuses) the shared Redis container and waits for it to accept connections.
    pub async fn start(suite: &TestContainerSuite) -> Self {
        // Keep the host port fixed across stop/start. Docker assigns a new ephemeral port to a
        // container whose mapping leaves HostPort empty, stranding already-running processes on
        // the old Redis endpoint after a restart.
        let lock = ContainerStateLock::acquire(suite, "redis-fixed");
        let mut state = lock.load();
        let _ = state.prune_stale();
        state.register_pid(std::process::id());
        lock.save(&state);
        let host_port = TcpListener::bind(("127.0.0.1", 0))
            .expect("reserve Redis test host port")
            .local_addr()
            .expect("resolve Redis test host port")
            .port();

        let container = GenericImage::new("redis", "7-alpine")
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
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
        drop(lock);

        Self {
            url: format!("redis://127.0.0.1:{port}/0"),
            address: SocketAddr::from(([127, 0, 0, 1], port)),
            _container: container,
            _lease: ContainerLease::new(suite.clone(), "redis-fixed"),
        }
    }

    /// Returns the Redis URL, for example `redis://127.0.0.1:6379/0`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Stops Redis immediately to simulate a broker outage.
    pub async fn stop(&self) {
        self._container
            .stop_with_timeout(Some(0))
            .await
            .expect("failed to stop Redis test container");
    }

    /// Restarts a previously stopped Redis container.
    pub async fn restart(&self) {
        self._container
            .start()
            .await
            .expect("failed to restart Redis test container");
        let ready = wait_until(
            Duration::from_secs(90),
            Duration::from_millis(250),
            || async {
                TcpStream::connect_timeout(&self.address, Duration::from_millis(500)).is_ok()
            },
        )
        .await;
        assert!(ready, "restarted Redis test container did not become ready");
    }
}
