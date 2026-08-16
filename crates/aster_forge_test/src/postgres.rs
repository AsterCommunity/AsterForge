//! Shared reusable `PostgreSQL` container for integration tests.
//!
//! The container provides isolated databases with automatic stale-resource cleanup. Products own
//! their migrations and seed data; this module owns database creation, connection retry, and
//! teardown mechanics.

use crate::database::connect_with_retry;
use crate::state::{ContainerLease, ContainerStateLock};
use crate::suite::TestContainerSuite;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use testcontainers::core::{ContainerAsync, IntoContainerPort};
use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

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
        let stale_resources = state.prune_stale();
        state.register_pid(std::process::id());
        for resource in &stale_resources {
            state.remember_resource(std::process::id(), resource);
        }
        lock.save(&state);

        let container = GenericImage::new("postgres", "16")
            .with_exposed_port(IntoContainerPort::tcp(5432))
            .with_container_name(suite.container_name("postgres"))
            .with_reuse(ReuseDirective::Always)
            .with_env_var("POSTGRES_USER", "postgres")
            .with_env_var("POSTGRES_PASSWORD", "postgres")
            .with_env_var("POSTGRES_DB", "postgres")
            .start()
            .await
            .expect("failed to start PostgreSQL test container");
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
        drop(lock);

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
            DatabaseOwnership::Process => state.remember_resource(std::process::id(), name),
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
        assert_valid_database_name, create_database_statement, database_url, quote_identifier,
    };

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
