//! Shared Mailpit container for integration tests.

use crate::state::{ContainerLease, ContainerStateLock};
use crate::suite::TestContainerSuite;
use crate::wait::wait_until;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use testcontainers::core::{ContainerAsync, IntoContainerPort};
use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

/// Reusable Mailpit SMTP/API container.
pub struct SmtpTestContainer {
    smtp_address: SocketAddr,
    api_base_url: String,
    client: reqwest::Client,
    _container: ContainerAsync<GenericImage>,
    _lease: ContainerLease,
}

impl SmtpTestContainer {
    /// Starts or reuses a Mailpit container for the test suite.
    pub async fn start(suite: &TestContainerSuite) -> Self {
        let lock = ContainerStateLock::acquire(suite, "mailpit");
        let mut state = lock.load();
        let _ = state.prune_stale();
        state.register_pid(std::process::id());
        lock.save(&state);
        let container = GenericImage::new("axllent/mailpit", "v1.21.8")
            .with_exposed_port(IntoContainerPort::tcp(1025))
            .with_exposed_port(IntoContainerPort::tcp(8025))
            .with_container_name(suite.container_name("mailpit"))
            .with_reuse(ReuseDirective::Always)
            .start()
            .await
            .expect("failed to start Mailpit test container");
        let smtp_port = container
            .get_host_port_ipv4(IntoContainerPort::tcp(1025))
            .await
            .expect("Mailpit SMTP port should be exposed");
        let api_port = container
            .get_host_port_ipv4(IntoContainerPort::tcp(8025))
            .await
            .expect("Mailpit API port should be exposed");
        drop(lock);
        let smtp_address = SocketAddr::from(([127, 0, 0, 1], smtp_port));
        let ready = wait_until(
            Duration::from_secs(90),
            Duration::from_millis(250),
            || async {
                TcpStream::connect_timeout(&smtp_address, Duration::from_millis(500)).is_ok()
            },
        )
        .await;
        assert!(ready, "Mailpit SMTP endpoint did not become ready");

        Self {
            smtp_address,
            api_base_url: format!("http://127.0.0.1:{api_port}"),
            client: reqwest::Client::new(),
            _container: container,
            _lease: ContainerLease::new(suite.clone(), "mailpit"),
        }
    }

    /// Returns the SMTP endpoint host and port.
    pub fn smtp_address(&self) -> SocketAddr {
        self.smtp_address
    }

    /// Deletes all messages currently stored by Mailpit.
    pub async fn clear_messages(&self) {
        let response = self
            .client
            .delete(format!("{}/api/v1/messages", self.api_base_url))
            .send()
            .await
            .expect("failed to clear Mailpit messages");
        assert!(
            response.status().is_success(),
            "Mailpit message cleanup failed with {}",
            response.status()
        );
    }

    /// Returns the number of messages currently stored by Mailpit.
    pub async fn message_count(&self) -> u64 {
        let response = self
            .client
            .get(format!("{}/api/v1/messages", self.api_base_url))
            .send()
            .await
            .expect("failed to query Mailpit messages");
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .expect("failed to decode Mailpit messages response");
        assert!(status.is_success(), "Mailpit API failed: {body}");
        body["total"]
            .as_u64()
            .expect("Mailpit response should include total")
    }
}
