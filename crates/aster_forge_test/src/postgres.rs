//! Shared reusable PostgreSQL container for integration tests.
//!
//! The container provides isolated databases with automatic stale-resource cleanup. Products own
//! their migrations and seed data; this module owns database creation, connection retry, and
//! teardown mechanics.

use crate::state::{ContainerLease, ContainerStateLock};
use crate::suite::TestContainerSuite;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use std::time::Duration;
use testcontainers::core::{ContainerAsync, IntoContainerPort, WaitFor};
use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

/// Handle to the suite's shared PostgreSQL container.
pub struct PostgresTestContainer {
    admin_url: String,
    suite: TestContainerSuite,
    _container: ContainerAsync<GenericImage>,
    _lease: ContainerLease,
}

/// Isolated PostgreSQL database owned by one test process.
pub struct PostgresTestDatabase {
    name: String,
    url: String,
    admin_url: String,
    suite: TestContainerSuite,
}

impl PostgresTestContainer {
    /// Starts (or reuses) the shared PostgreSQL container with `postgres`/`postgres` credentials.
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
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
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
        drop(lock);

        let fixture = Self {
            admin_url: format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres"),
            suite: suite.clone(),
            _container: container,
            _lease: ContainerLease::new(suite.clone(), "postgres"),
        };
        fixture.cleanup_databases(&stale_resources).await;
        fixture
    }

    /// Returns the admin URL pointing at the default `postgres` database.
    pub fn admin_url(&self) -> &str {
        &self.admin_url
    }

    /// Creates and registers an isolated database for a product test.
    pub async fn create_database(&self, name: &str) -> PostgresTestDatabase {
        assert_valid_database_name(name);
        let lock = ContainerStateLock::acquire(&self.suite, "postgres");
        let mut state = lock.load();
        state.remember_resource(std::process::id(), name);
        lock.save(&state);
        drop(lock);

        let admin = connect_with_retry(&self.admin_url).await;
        admin
            .execute_unprepared(&format!("CREATE DATABASE {}", quote_identifier(name)))
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
        }
    }

    async fn cleanup_databases(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let admin = connect_with_retry(&self.admin_url).await;
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
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the connection URL for this database.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Connects to this database, retrying while the service becomes ready.
    pub async fn connect(&self) -> DatabaseConnection {
        connect_with_retry(&self.url).await
    }

    /// Drops this database and removes it from the shared resource registry.
    pub async fn cleanup(&self) {
        let admin = connect_with_retry(&self.admin_url).await;
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
        state.forget_resource(std::process::id(), &self.name);
        lock.save(&state);
    }
}

async fn connect_with_retry(database_url: &str) -> DatabaseConnection {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        match Database::connect(database_url).await {
            Ok(database) => return database,
            Err(error) if tokio::time::Instant::now() >= deadline => {
                panic!("PostgreSQL test database did not become ready: {error}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

fn database_url(admin_url: &str, name: &str) -> String {
    admin_url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/{name}"))
        .unwrap_or_else(|| admin_url.to_string())
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
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
    use super::{assert_valid_database_name, database_url, quote_identifier};

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
