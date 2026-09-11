//! Shared reusable `PostgreSQL` container for integration tests.
//!
//! The container provides isolated databases with automatic stale-resource cleanup. Products own
//! their migrations and seed data; this module owns database creation, connection retry, and
//! teardown mechanics.

use crate::database::connect_with_retry;
use crate::state::{ContainerLease, ContainerStateLock};
use crate::suite::TestContainerSuite;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use testcontainers::core::{ContainerAsync, ContainerRequest, IntoContainerPort};
use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

const POSTGRES_TEST_SHM_SIZE_BYTES: u64 = 1024 * 1024 * 1024;
const POSTGRES_CONTAINER_SERVICE: &str = "postgres-shm-1g";

/// Handle to the suite's shared `PostgreSQL` container.
pub struct PostgresTestContainer {
    admin_url: String,
    suite: TestContainerSuite,
    _container: ContainerAsync<GenericImage>,
    _lease: ContainerLease,
}

/// Isolated `PostgreSQL` database owned by one test process.
pub struct PostgresTestDatabase {
    name: String,
    url: String,
    admin_url: String,
    suite: TestContainerSuite,
    ownership: DatabaseOwnership,
}

#[derive(Clone, Copy)]
enum DatabaseOwnership {
    Process,
    Shared,
}

impl PostgresTestContainer {
    /// Starts (or reuses) the shared `PostgreSQL` container with `postgres`/`postgres` credentials.
    ///
    /// # Panics
    ///
    /// Panics when shared state, container startup, port discovery, readiness, stale-database
    /// cleanup, or connection shutdown fails.
    pub async fn start(suite: &TestContainerSuite) -> Self {
        let lock = ContainerStateLock::acquire(suite, "postgres");
        let mut state = lock.load();
        let stale_resources = state.prune_stale_during_current_execution();
        state.register_current_process();
        for resource in &stale_resources {
            state.remember_current_process_resource(resource);
        }
        lock.save(&state);

        drop(lock);

        // Several nextest processes can enter this path at once. Docker's reusable-container
        // lookup and create operation is not atomic, so one process may briefly observe a name
        // conflict while another is creating the shared container. Retry those startup errors
        // until the first process has published a reusable container that this process can attach
        // to.
        let container = start_postgres_container_with_retry(suite).await;
        let port = container
            .get_host_port_ipv4(IntoContainerPort::tcp(5432))
            .await
            .expect("PostgreSQL test port should be exposed");
        let admin_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        connect_with_retry(&admin_url, "PostgreSQL")
            .await
            .close()
            .await
            .expect("failed to close PostgreSQL readiness probe connection");

        let fixture = Self {
            admin_url,
            suite: suite.clone(),
            _container: container,
            _lease: ContainerLease::new(suite.clone(), "postgres"),
        };
        fixture.cleanup_databases(&stale_resources).await;
        fixture
    }

    /// Returns the admin URL pointing at the default `postgres` database.
    #[must_use]
    pub fn admin_url(&self) -> &str {
        &self.admin_url
    }

    /// Returns a stable identity for the running suite container.
    ///
    /// The resolved admin URL includes the checkout-scoped host port and therefore changes when
    /// the reusable container belongs to a different suite instance.
    #[must_use]
    pub fn container_identity(&self) -> &str {
        &self.admin_url
    }

    /// Creates and registers an isolated database for a product test.
    ///
    /// # Panics
    ///
    /// Panics when the database name is invalid, shared state fails, or the admin connection,
    /// `CREATE DATABASE`, or connection shutdown fails.
    pub async fn create_database(&self, name: &str) -> PostgresTestDatabase {
        self.create_database_inner(name, None, DatabaseOwnership::Process)
            .await
    }

    /// Creates and registers an isolated database cloned from `template`.
    ///
    /// Products still own the template contents, migrations, and seed data. This helper only
    /// provides the product-neutral `PostgreSQL` database lifecycle and safe identifier handling.
    ///
    /// # Panics
    ///
    /// Panics when either database name is invalid, shared state fails, or the admin connection,
    /// `CREATE DATABASE ... TEMPLATE ...`, or connection shutdown fails.
    pub async fn create_database_from_template(
        &self,
        name: &str,
        template: &str,
    ) -> PostgresTestDatabase {
        assert_valid_database_name(template);
        self.create_database_inner(name, Some(template), DatabaseOwnership::Process)
            .await
    }

    /// Creates a suite-scoped database for a product-owned reusable fixture.
    ///
    /// Unlike [`Self::create_database`], this resource survives the producer process. Products
    /// must use a separate locked fixture-state protocol to validate or invalidate its contents.
    pub async fn create_shared_database(&self, name: &str) -> PostgresTestDatabase {
        self.create_database_inner(name, None, DatabaseOwnership::Shared)
            .await
    }

    /// Drops a suite-scoped fixture database and unregisters it.
    ///
    /// # Panics
    ///
    /// Panics when the name is invalid or database cleanup, connection, or shutdown fails.
    pub async fn drop_shared_database(&self, name: &str) {
        assert_valid_database_name(name);
        let admin = connect_with_retry(&self.admin_url, "PostgreSQL").await;
        admin
            .execute_unprepared(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                quote_identifier(name)
            ))
            .await
            .unwrap_or_else(|error| {
                panic!("failed to drop shared PostgreSQL test database {name}: {error}")
            });
        admin.close().await.unwrap_or_else(|error| {
            panic!("failed to close PostgreSQL shared database admin connection: {error}")
        });

        self.forget_shared_resource(name);
    }

    /// Registers a suite-scoped product fixture that must outlive the producer process.
    ///
    /// This is for product-owned migrated template databases. Products must pair it with their
    /// own fingerprint-based invalidation and call [`Self::forget_shared_resource`] after
    /// dropping a superseded fixture.
    pub fn remember_shared_resource(&self, resource: &str) {
        let lock = ContainerStateLock::acquire(&self.suite, "postgres");
        let mut state = lock.load();
        state.remember_shared_resource(resource);
        lock.save(&state);
    }

    /// Removes a suite-scoped product fixture after it was explicitly cleaned up.
    pub fn forget_shared_resource(&self, resource: &str) {
        let lock = ContainerStateLock::acquire(&self.suite, "postgres");
        let mut state = lock.load();
        state.forget_shared_resource(resource);
        lock.save(&state);
    }

    async fn create_database_inner(
        &self,
        name: &str,
        template: Option<&str>,
        ownership: DatabaseOwnership,
    ) -> PostgresTestDatabase {
        assert_valid_database_name(name);
        let lock = ContainerStateLock::acquire(&self.suite, "postgres");
        let mut state = lock.load();
        match ownership {
            DatabaseOwnership::Process => state.remember_current_process_resource(name),
            DatabaseOwnership::Shared => state.remember_shared_resource(name),
        }
        lock.save(&state);
        drop(lock);

        let admin = connect_with_retry(&self.admin_url, "PostgreSQL").await;
        let create_database = create_database_statement(name, template);
        admin
            .execute_unprepared(&create_database)
            .await
            .unwrap_or_else(|error| {
                panic!("failed to create PostgreSQL test database {name}: {error}")
            });
        admin
            .close()
            .await
            .unwrap_or_else(|error| panic!("failed to close PostgreSQL admin connection: {error}"));

        PostgresTestDatabase {
            name: name.to_string(),
            url: database_url(&self.admin_url, name),
            admin_url: self.admin_url.clone(),
            suite: self.suite.clone(),
            ownership,
        }
    }

    async fn cleanup_databases(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let admin = connect_with_retry(&self.admin_url, "PostgreSQL").await;
        for name in names {
            admin
                .execute_unprepared(&format!(
                    "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                    quote_identifier(name)
                ))
                .await
                .unwrap_or_else(|error| {
                    panic!("failed to drop stale PostgreSQL test database {name}: {error}")
                });
            let lock = ContainerStateLock::acquire(&self.suite, "postgres");
            let mut state = lock.load();
            state.forget_resource(std::process::id(), name);
            lock.save(&state);
        }
        admin
            .close()
            .await
            .unwrap_or_else(|error| panic!("failed to close PostgreSQL admin connection: {error}"));
    }
}

async fn start_postgres_container_with_retry(
    suite: &TestContainerSuite,
) -> ContainerAsync<GenericImage> {
    let mut last_error = None;
    for _attempt in 0..240 {
        match postgres_container_request(suite).start().await {
            Ok(container) => return container,
            Err(error) => {
                last_error = Some(error.to_string());
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }

    panic!(
        "failed to start PostgreSQL test container after retries: {}",
        last_error.unwrap_or_else(|| "unknown container startup error".to_string())
    );
}

fn postgres_container_request(suite: &TestContainerSuite) -> ContainerRequest<GenericImage> {
    GenericImage::new("postgres", "16")
        .with_exposed_port(IntoContainerPort::tcp(5432))
        .with_container_name(suite.container_name(POSTGRES_CONTAINER_SERVICE))
        .with_reuse(ReuseDirective::Always)
        .with_shm_size(POSTGRES_TEST_SHM_SIZE_BYTES)
        .with_env_var("POSTGRES_USER", "postgres")
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .with_env_var("POSTGRES_DB", "postgres")
}

impl PostgresTestDatabase {
    /// Returns the isolated database name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the connection URL for this database.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Connects to this database, retrying while the service becomes ready.
    ///
    /// # Panics
    ///
    /// Panics when the database does not accept a connection before the readiness timeout.
    pub async fn connect(&self) -> DatabaseConnection {
        connect_with_retry(&self.url, "PostgreSQL").await
    }

    /// Drops this database and removes it from the shared resource registry.
    ///
    /// # Panics
    ///
    /// Panics when the admin connection, database drop, connection shutdown, or shared-state
    /// update fails.
    pub async fn cleanup(&self) {
        let admin = connect_with_retry(&self.admin_url, "PostgreSQL").await;
        admin
            .execute_unprepared(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                quote_identifier(&self.name)
            ))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "failed to drop PostgreSQL test database {}: {error}",
                    self.name
                )
            });
        admin
            .close()
            .await
            .unwrap_or_else(|error| panic!("failed to close PostgreSQL admin connection: {error}"));

        let lock = ContainerStateLock::acquire(&self.suite, "postgres");
        let mut state = lock.load();
        match self.ownership {
            DatabaseOwnership::Process => {
                state.forget_resource(std::process::id(), &self.name);
            }
            DatabaseOwnership::Shared => state.forget_shared_resource(&self.name),
        }
        lock.save(&state);
    }
}

fn database_url(admin_url: &str, name: &str) -> String {
    admin_url.rsplit_once('/').map_or_else(
        || admin_url.to_string(),
        |(base, _)| format!("{base}/{name}"),
    )
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn create_database_statement(name: &str, template: Option<&str>) -> String {
    template.map_or_else(
        || format!("CREATE DATABASE {}", quote_identifier(name)),
        |template| {
            format!(
                "CREATE DATABASE {} TEMPLATE {}",
                quote_identifier(name),
                quote_identifier(template)
            )
        },
    )
}

fn assert_valid_database_name(name: &str) {
    assert!(
        !name.is_empty()
            && name.len() <= 63
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "PostgreSQL test database name must be 1-63 ASCII alphanumeric or '_' characters: {name:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::{
        POSTGRES_CONTAINER_SERVICE, POSTGRES_TEST_SHM_SIZE_BYTES, assert_valid_database_name,
        create_database_statement, database_url, postgres_container_request, quote_identifier,
    };
    use crate::suite::TestContainerSuite;
    use testcontainers::{core::ExecCommand, runners::AsyncRunner};

    #[test]
    fn database_url_replaces_admin_database() {
        assert_eq!(
            database_url("postgres://user:pass@127.0.0.1:5432/postgres", "isolated"),
            "postgres://user:pass@127.0.0.1:5432/isolated"
        );
    }

    #[test]
    fn identifier_quoting_escapes_quotes() {
        assert_eq!(quote_identifier("test\"name"), "\"test\"\"name\"");
    }

    #[test]
    fn postgres_container_request_sets_versioned_name_and_shared_memory() {
        let suite = TestContainerSuite::new("forge-postgres-request");
        let request = postgres_container_request(&suite);
        let expected_name = suite.container_name(POSTGRES_CONTAINER_SERVICE);

        assert_eq!(request.shm_size(), Some(POSTGRES_TEST_SHM_SIZE_BYTES));
        assert_eq!(
            request.container_name().as_deref(),
            Some(expected_name.as_str())
        );
    }

    #[tokio::test]
    async fn postgres_container_exposes_configured_shared_memory() {
        let suite = TestContainerSuite::new("forge-postgres-shm");
        let container = postgres_container_request(&suite)
            .start()
            .await
            .expect("PostgreSQL test container should start");
        let mut command = container
            .exec(ExecCommand::new(["df", "-B1", "--output=size", "/dev/shm"]))
            .await
            .expect("shared-memory capacity command should start");
        let stdout = command
            .stdout_to_vec()
            .await
            .expect("shared-memory capacity command should finish");
        let output = String::from_utf8(stdout).expect("df output should be UTF-8");
        let capacity = output
            .lines()
            .find_map(|line| line.trim().parse::<u64>().ok())
            .expect("df output should contain the shared-memory capacity");

        assert!(
            capacity >= POSTGRES_TEST_SHM_SIZE_BYTES,
            "PostgreSQL test container shared memory must be at least {POSTGRES_TEST_SHM_SIZE_BYTES} bytes, got {capacity}"
        );
    }

    #[test]
    fn database_creation_supports_an_optional_template() {
        assert_eq!(
            create_database_statement("isolated", None),
            "CREATE DATABASE \"isolated\""
        );
        assert_eq!(
            create_database_statement("isolated", Some("template")),
            "CREATE DATABASE \"isolated\" TEMPLATE \"template\""
        );
    }

    #[test]
    fn database_name_accepts_boundaries() {
        assert_valid_database_name("a");
        assert_valid_database_name(&"a".repeat(63));
        assert_valid_database_name("aster_product_123");
    }

    #[test]
    fn database_name_rejects_unsafe_or_oversized_values() {
        for name in ["", "has-hyphen", "has quote\"", "has space"] {
            assert!(
                std::panic::catch_unwind(|| assert_valid_database_name(name)).is_err(),
                "database name {name:?} should be rejected"
            );
        }
        let oversized = "a".repeat(64);
        assert!(std::panic::catch_unwind(|| assert_valid_database_name(&oversized)).is_err());
    }
}
