//! Shared reusable `MySQL` container for integration tests.
//!
//! The container provides a root connection to the default `mysql` system database. Creating
//! per-test databases and granting product users stays with the product test harness, which can
//! register database names via [`crate::state::SharedContainerState::remember_resource`] so stale
//! databases are pruned on later runs.

use crate::database::connect_with_retry;
use crate::state::{ContainerLease, ContainerStateLock, SharedContainerEndpoint};
use crate::suite::TestContainerSuite;
use sea_orm::{ConnectionTrait, Database};
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
    _container: Option<ContainerAsync<GenericImage>>,
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
        let endpoint_identity = format!("mysql:8.4/{}", suite.container_name("mysql"));

        if let Some(port) = state
            .endpoint()
            .filter(|endpoint| endpoint.matches(&endpoint_identity))
            .map(SharedContainerEndpoint::port)
        {
            let root_url = root_url(port);
            if let Ok(root) = Database::connect(&root_url).await
                && root.close().await.is_ok()
            {
                lock.save(&state);
                drop(lock);
                return Self {
                    root_url,
                    suite: suite.clone(),
                    stale_resources,
                    _container: None,
                    _lease: ContainerLease::new(suite.clone(), "mysql"),
                };
            }
            state.clear_endpoint();
        }

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
        let root_url = root_url(port);
        let root = connect_with_retry(&root_url, "MySQL").await;
        root.execute_unprepared(&format!(
            "SET GLOBAL table_definition_cache = {MYSQL_TEST_TABLE_DEFINITION_CACHE}"
        ))
        .await
        .expect("failed to configure MySQL test table definition cache");
        root.close()
            .await
            .expect("failed to close MySQL readiness probe connection");
        state.set_endpoint(SharedContainerEndpoint::new(endpoint_identity, port));
        lock.save(&state);
        drop(lock);

        Self {
            root_url,
            suite: suite.clone(),
            stale_resources,
            _container: Some(container),
            _lease: ContainerLease::new(suite.clone(), "mysql"),
        }
    }

    /// Returns the root URL pointing at the `mysql` system database.
    #[must_use]
    pub fn root_url(&self) -> &str {
        &self.root_url
    }

    /// Returns a stable identity for the running suite container.
    #[must_use]
    pub fn container_identity(&self) -> &str {
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

    /// Registers a suite-scoped product fixture that must outlive the producer process.
    ///
    /// This is for product-owned migrated template schemas. Products must pair it with their own
    /// fingerprint-based invalidation and call [`Self::forget_shared_resource`] after dropping a
    /// superseded fixture.
    pub fn remember_shared_resource(&self, resource: &str) {
        let lock = ContainerStateLock::acquire(&self.suite, "mysql");
        let mut state = lock.load();
        state.remember_shared_resource(resource);
        lock.save(&state);
    }

    /// Removes a suite-scoped product fixture after it was explicitly cleaned up.
    pub fn forget_shared_resource(&self, resource: &str) {
        let lock = ContainerStateLock::acquire(&self.suite, "mysql");
        let mut state = lock.load();
        state.forget_shared_resource(resource);
        lock.save(&state);
    }

    /// Creates a suite-scoped database for a product-owned reusable fixture.
    ///
    /// Products own user grants, migrations, and fingerprint validation. This helper owns only
    /// the root-level database lifecycle and shared-resource registration.
    ///
    /// # Panics
    ///
    /// Panics when the name is invalid or database creation, connection, or shutdown fails.
    pub async fn create_shared_database(&self, name: &str) {
        assert_valid_database_name(name);
        self.remember_shared_resource(name);

        let root = connect_with_retry(&self.root_url, "MySQL").await;
        root.execute_unprepared(&format!("CREATE DATABASE {}", quote_identifier(name)))
            .await
            .unwrap_or_else(|error| {
                panic!("failed to create shared MySQL test database {name}: {error}")
            });
        root.close().await.unwrap_or_else(|error| {
            panic!("failed to close MySQL shared database admin connection: {error}")
        });
    }

    /// Drops a suite-scoped fixture database and unregisters it.
    ///
    /// # Panics
    ///
    /// Panics when the name is invalid or database cleanup, connection, or shutdown fails.
    pub async fn drop_shared_database(&self, name: &str) {
        assert_valid_database_name(name);
        let root = connect_with_retry(&self.root_url, "MySQL").await;
        root.execute_unprepared(&format!(
            "DROP DATABASE IF EXISTS {}",
            quote_identifier(name)
        ))
        .await
        .unwrap_or_else(|error| {
            panic!("failed to drop shared MySQL test database {name}: {error}")
        });
        root.close().await.unwrap_or_else(|error| {
            panic!("failed to close MySQL shared database admin connection: {error}")
        });
        self.forget_shared_resource(name);
    }
}

fn root_url(port: u16) -> String {
    format!("mysql://root:rootpass@127.0.0.1:{port}/mysql")
}

fn quote_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

fn assert_valid_database_name(name: &str) {
    assert!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "MySQL test database name must be 1-64 ASCII alphanumeric or '_' characters: {name:?}"
    );
}
