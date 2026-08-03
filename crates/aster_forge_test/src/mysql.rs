//! Shared reusable `MySQL` container for integration tests.
//!
//! The container provides a root connection to the default `mysql` system database. Creating
//! per-test databases and granting product users stays with the product test harness, which can
//! register database names via [`crate::state::SharedContainerState::remember_resource`] so stale
//! databases are pruned on later runs.

use crate::database::connect_with_retry;
use crate::state::{ContainerLease, ContainerStateLock};
use crate::suite::TestContainerSuite;
use sea_orm::ConnectionTrait;
use testcontainers::core::{ContainerAsync, IntoContainerPort};
use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

/// Table-definition cache used by the shared `MySQL` integration-test container.
///
/// Large test binaries may create hundreds of isolated schemas concurrently. `MySQL`'s default
/// cache is too small for that workload and can exhaust prepared-statement reprepare attempts.
pub const MYSQL_TEST_TABLE_DEFINITION_CACHE: u64 = 32_768;

/// Handle to the suite's shared `MySQL` container.
pub struct MysqlTestContainer {
    root_url: String,
    suite: TestContainerSuite,
    stale_resources: Vec<String>,
    _container: ContainerAsync<GenericImage>,
    _lease: ContainerLease,
}

impl MysqlTestContainer {
    /// Starts (or reuses) the shared `MySQL` container with `root`/`rootpass` credentials.
    ///
    /// # Panics
    ///
    /// Panics when shared state, container startup, port discovery, readiness connection, or
    /// readiness connection shutdown fails.
    pub async fn start(suite: &TestContainerSuite) -> Self {
        let lock = ContainerStateLock::acquire(suite, "mysql");
        let mut state = lock.load();
        let stale_resources = state.prune_stale();
        state.register_pid(std::process::id());
        lock.save(&state);

        let container = GenericImage::new("mysql", "8.4")
            .with_exposed_port(IntoContainerPort::tcp(3306))
            .with_container_name(suite.container_name("mysql"))
            .with_reuse(ReuseDirective::Always)
            .with_env_var("MYSQL_ROOT_PASSWORD", "rootpass")
            .start()
            .await
            .expect("failed to start MySQL test container");
        let port = container
            .get_host_port_ipv4(IntoContainerPort::tcp(3306))
            .await
            .expect("MySQL test port should be exposed");
        let root_url = format!("mysql://root:rootpass@127.0.0.1:{port}/mysql");
        let root = connect_with_retry(&root_url, "MySQL").await;
        root.execute_unprepared(&format!(
            "SET GLOBAL table_definition_cache = {MYSQL_TEST_TABLE_DEFINITION_CACHE}"
        ))
        .await
        .expect("failed to configure MySQL test table definition cache");
        root.close()
            .await
            .expect("failed to close MySQL readiness probe connection");
        drop(lock);

        Self {
            root_url,
            suite: suite.clone(),
            stale_resources,
            _container: container,
            _lease: ContainerLease::new(suite.clone(), "mysql"),
        }
    }

    /// Returns the root URL pointing at the `mysql` system database.
    #[must_use]
    pub fn root_url(&self) -> &str {
        &self.root_url
    }

    /// Builds a URL for a database created inside this container.
    #[must_use]
    pub fn database_url(&self, database: &str) -> String {
        self.root_url.rsplit_once('/').map_or_else(
            || self.root_url.clone(),
            |(base, _)| format!("{base}/{database}"),
        )
    }

    /// Returns resources left by test processes that no longer exist.
    #[must_use]
    pub fn stale_resources(&self) -> &[String] {
        &self.stale_resources
    }

    /// Registers a product-owned resource, such as a per-test database name.
    pub fn remember_resource(&self, resource: &str) {
        let lock = ContainerStateLock::acquire(&self.suite, "mysql");
        let mut state = lock.load();
        state.remember_resource(std::process::id(), resource);
        lock.save(&state);
    }

    /// Removes a resource after the product test harness cleaned it up.
    pub fn forget_resource(&self, resource: &str) {
        let lock = ContainerStateLock::acquire(&self.suite, "mysql");
        let mut state = lock.load();
        state.forget_resource(std::process::id(), resource);
        lock.save(&state);
    }
}
