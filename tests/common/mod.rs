//! Shared integration test helpers.

use aster_forge::runtime::AppState;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

const TEST_DATABASE_BACKEND_ENV: &str = "ASTER_TEST_DATABASE_BACKEND";
const TEST_DATABASE_BACKEND_ALIAS_ENV: &str = "ASTER_DATABASE_BACKEND";
const SHARED_TEST_CONTAINER_STATE_DIR: &str = "/tmp/asterforge-testcontainers";

fn init_test_process_state() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {});
}

#[allow(dead_code)]
pub async fn set_foreign_key_checks(
    db: &sea_orm::DatabaseConnection,
    enabled: bool,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::ConnectionTrait;

    let sql = match (db.get_database_backend(), enabled) {
        (sea_orm::DbBackend::Sqlite, true) => "PRAGMA foreign_keys=ON;",
        (sea_orm::DbBackend::Sqlite, false) => "PRAGMA foreign_keys=OFF;",
        (sea_orm::DbBackend::Postgres, true) => "SET session_replication_role = origin;",
        (sea_orm::DbBackend::Postgres, false) => "SET session_replication_role = replica;",
        (sea_orm::DbBackend::MySql, true) => "SET FOREIGN_KEY_CHECKS = 1;",
        (sea_orm::DbBackend::MySql, false) => "SET FOREIGN_KEY_CHECKS = 0;",
        _ => return Ok(()),
    };

    db.execute_unprepared(sql).await.map(|_| ())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestDatabaseBackend {
    Sqlite,
    Postgres,
    MySql,
}

struct SharedTestDatabaseContainer {
    _container: testcontainers::ContainerAsync<testcontainers::GenericImage>,
    _lease: SharedTestContainerLease,
    admin_database_url: String,
    database_url: String,
}

struct MySqlSchemaTemplate {
    database_name: String,
    create_table_sql: Vec<String>,
}

struct PostgresDatabaseTemplate {
    database_name: String,
}

static POSTGRES_TEST_CONTAINER: tokio::sync::OnceCell<SharedTestDatabaseContainer> =
    tokio::sync::OnceCell::const_new();
static MYSQL_TEST_CONTAINER: tokio::sync::OnceCell<SharedTestDatabaseContainer> =
    tokio::sync::OnceCell::const_new();
static POSTGRES_DATABASE_TEMPLATE: tokio::sync::OnceCell<PostgresDatabaseTemplate> =
    tokio::sync::OnceCell::const_new();
static MYSQL_SCHEMA_TEMPLATE: tokio::sync::OnceCell<MySqlSchemaTemplate> =
    tokio::sync::OnceCell::const_new();

#[derive(Default, Deserialize, Serialize)]
struct SharedTestContainerState {
    #[serde(default)]
    databases_by_pid: HashMap<u32, Vec<String>>,
    pids: Vec<u32>,
}

struct SharedTestContainerLease {
    backend: TestDatabaseBackend,
}

impl Drop for SharedTestContainerLease {
    fn drop(&mut self) {
        release_shared_test_container(self.backend);
    }
}

impl SharedTestContainerLease {
    const fn new(backend: TestDatabaseBackend) -> Self {
        Self { backend }
    }
}

impl SharedTestContainerState {
    fn normalize(&mut self) {
        self.pids.sort_unstable();
        self.pids.dedup();

        self.databases_by_pid
            .retain(|pid, _| self.pids.binary_search(pid).is_ok());
        for pid in &self.pids {
            let databases = self.databases_by_pid.entry(*pid).or_default();
            databases.sort_unstable();
            databases.dedup();
        }
    }

    fn register_pid(&mut self, pid: u32) {
        if !self.pids.contains(&pid) {
            self.pids.push(pid);
        }
        self.normalize();
    }

    fn remember_database(&mut self, pid: u32, database_name: &str) {
        self.register_pid(pid);
        let databases = self.databases_by_pid.entry(pid).or_default();
        if !databases.iter().any(|name| name == database_name) {
            databases.push(database_name.to_string());
        }
        databases.sort_unstable();
    }
}

impl TestDatabaseBackend {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::MySql => "mysql",
        }
    }

    const fn container_port(self) -> u16 {
        match self {
            Self::Sqlite => 0,
            Self::Postgres => 5432,
            Self::MySql => 3306,
        }
    }

    fn shared_container_name(self) -> String {
        format!("asterforge-test-{}-{}", test_workspace_id(), self.as_str())
    }

    fn shared_state_path(self) -> PathBuf {
        shared_test_container_state_dir().join(format!(
            "{}-{}.json",
            test_workspace_id(),
            self.as_str()
        ))
    }

    fn shared_lock_path(self) -> PathBuf {
        shared_test_container_state_dir().join(format!(
            "{}-{}.lock",
            test_workspace_id(),
            self.as_str()
        ))
    }

    fn database_url(self, port: u16) -> String {
        match self {
            Self::Sqlite => "sqlite::memory:".to_string(),
            Self::Postgres => format!("postgres://postgres:postgres@127.0.0.1:{port}/asterforge"),
            Self::MySql => format!("mysql://aster:asterpass@127.0.0.1:{port}/asterforge"),
        }
    }

    fn admin_database_url(self, port: u16) -> String {
        match self {
            Self::Sqlite => "sqlite::memory:".to_string(),
            Self::Postgres => self.database_url(port),
            Self::MySql => format!("mysql://root:rootpass@127.0.0.1:{port}/asterforge"),
        }
    }
}

fn test_workspace_id() -> &'static str {
    static WORKSPACE_ID: OnceLock<String> = OnceLock::new();
    WORKSPACE_ID.get_or_init(|| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        env!("CARGO_MANIFEST_DIR").hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    })
}

fn shared_test_container_state_dir() -> &'static Path {
    static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();
    STATE_DIR
        .get_or_init(|| {
            let path = PathBuf::from(SHARED_TEST_CONTAINER_STATE_DIR);
            std::fs::create_dir_all(&path).expect("shared test container state dir should exist");
            path
        })
        .as_path()
}

fn lock_shared_test_container_state(backend: TestDatabaseBackend) -> File {
    let lock_path = backend.shared_lock_path();
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(lock_path)
        .expect("shared test container lock file should open");
    file.lock_exclusive()
        .expect("shared test container lock should be acquired");
    file
}

fn load_shared_test_container_state(
    file: &mut File,
    backend: TestDatabaseBackend,
) -> SharedTestContainerState {
    let state_path = backend.shared_state_path();
    if !state_path.exists() {
        return SharedTestContainerState::default();
    }

    file.seek(SeekFrom::Start(0))
        .expect("state lock file should seek");
    let mut raw = String::new();
    File::open(state_path)
        .and_then(|mut state_file| state_file.read_to_string(&mut raw))
        .expect("shared test container state should be readable");

    let mut state = if raw.trim().is_empty() {
        SharedTestContainerState::default()
    } else {
        serde_json::from_str(&raw).expect("shared test container state should be valid json")
    };
    state.normalize();
    state
}

fn save_shared_test_container_state(
    file: &mut File,
    backend: TestDatabaseBackend,
    state: &SharedTestContainerState,
) {
    let state_path = backend.shared_state_path();
    file.seek(SeekFrom::Start(0))
        .expect("state lock file should seek");

    let json = serde_json::to_vec(state).expect("shared test container state should serialize");
    let mut state_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(state_path)
        .expect("shared test container state file should open");
    state_file
        .write_all(&json)
        .expect("shared test container state should write");
    state_file
        .write_all(b"\n")
        .expect("shared test container state should end with newline");
    state_file
        .flush()
        .expect("shared test container state should flush");
    let _ = file.flush();
}

fn process_is_running(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }

    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn prune_shared_test_container_state(state: &mut SharedTestContainerState) -> Vec<String> {
    let stale_pids = state
        .pids
        .iter()
        .copied()
        .filter(|pid| !process_is_running(*pid))
        .collect::<Vec<_>>();
    let stale_databases = stale_pids
        .iter()
        .flat_map(|pid| {
            state
                .databases_by_pid
                .remove(pid)
                .unwrap_or_default()
                .into_iter()
        })
        .collect::<Vec<_>>();

    state.pids.retain(|pid| !stale_pids.contains(pid));
    state.normalize();
    stale_databases
}

fn remember_shared_test_database(backend: TestDatabaseBackend, database_name: &str) {
    let mut lock_file = lock_shared_test_container_state(backend);
    let mut state = load_shared_test_container_state(&mut lock_file, backend);
    state.remember_database(std::process::id(), database_name);
    save_shared_test_container_state(&mut lock_file, backend, &state);
}

fn test_backend_from_database_backend(backend: sea_orm::DbBackend) -> Option<TestDatabaseBackend> {
    match backend {
        sea_orm::DbBackend::Postgres => Some(TestDatabaseBackend::Postgres),
        sea_orm::DbBackend::MySql => Some(TestDatabaseBackend::MySql),
        _ => None,
    }
}

fn release_shared_test_container(backend: TestDatabaseBackend) {
    let mut lock_file = lock_shared_test_container_state(backend);
    let mut state = load_shared_test_container_state(&mut lock_file, backend);
    let _ = prune_shared_test_container_state(&mut state);
    save_shared_test_container_state(&mut lock_file, backend, &state);
}

async fn drop_stale_test_databases(
    backend: sea_orm::DbBackend,
    admin_database_url: &str,
    database_names: &[String],
) {
    if database_names.is_empty() {
        return;
    }

    use sea_orm::ConnectionTrait;

    let admin_cfg = aster_forge::config::DatabaseConfig {
        url: admin_database_url.to_string(),
        pool_size: 1,
        retry_count: 0,
    };
    let admin_db = aster_forge::db::connect_with_metrics(
        &admin_cfg,
        aster_forge::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("stale test database cleanup should connect");

    for database_name in database_names {
        let drop_sql = format!(
            "DROP DATABASE IF EXISTS {}",
            quote_database_identifier(backend, database_name)
        );
        admin_db
            .execute_unprepared(&drop_sql)
            .await
            .expect("stale test database should drop");
    }
}

async fn ensure_mysql_test_user_access(admin_database_url: &str, username: &str) {
    use sea_orm::ConnectionTrait;

    let admin_cfg = aster_forge::config::DatabaseConfig {
        url: admin_database_url.to_string(),
        pool_size: 1,
        retry_count: 0,
    };
    let admin_db = aster_forge::db::connect_with_metrics(
        &admin_cfg,
        aster_forge::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("mysql test admin connection should succeed");
    let grant_sql = format!(
        "GRANT ALL PRIVILEGES ON *.* TO {}@'%'",
        quote_mysql_string(username)
    );
    admin_db
        .execute_unprepared(&grant_sql)
        .await
        .expect("mysql test user grant should succeed");
}

fn configured_database_backend_env_value() -> Option<String> {
    std::env::var(TEST_DATABASE_BACKEND_ENV)
        .ok()
        .or_else(|| std::env::var(TEST_DATABASE_BACKEND_ALIAS_ENV).ok())
}

fn configured_test_database_backend() -> TestDatabaseBackend {
    match configured_database_backend_env_value()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("") | Some("sqlite") => TestDatabaseBackend::Sqlite,
        Some("postgres") | Some("postgresql") => TestDatabaseBackend::Postgres,
        Some("mysql") => TestDatabaseBackend::MySql,
        Some(other) => panic!(
            "unsupported {TEST_DATABASE_BACKEND_ENV}/{TEST_DATABASE_BACKEND_ALIAS_ENV} value '{other}', expected sqlite/postgres/mysql"
        ),
    }
}

async fn wait_for_database(database_url: &str) {
    let mut last_err: Option<String> = None;
    let ready = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let cfg = aster_forge::config::DatabaseConfig {
                url: database_url.to_string(),
                pool_size: 1,
                retry_count: 0,
            };
            match aster_forge::db::connect_with_metrics(
                &cfg,
                aster_forge::metrics_core::NoopMetrics::arc(),
            )
            .await
            {
                Ok(_) => break,
                Err(err) => {
                    last_err = Some(err.to_string());
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
    })
    .await;

    if ready.is_err() {
        panic!(
            "timed out waiting for database {database_url}: {}",
            last_err.unwrap_or_else(|| "unknown error".to_string())
        );
    }
}

async fn start_postgres_test_container() -> SharedTestDatabaseContainer {
    use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

    let backend = TestDatabaseBackend::Postgres;
    let mut lock_file = lock_shared_test_container_state(backend);
    let mut state = load_shared_test_container_state(&mut lock_file, backend);
    let stale_databases = prune_shared_test_container_state(&mut state);
    let current_pid = std::process::id();
    state.register_pid(current_pid);

    let container = GenericImage::new("postgres", "16")
        .with_exposed_port(testcontainers::core::IntoContainerPort::tcp(
            backend.container_port(),
        ))
        .with_container_name(backend.shared_container_name())
        .with_reuse(ReuseDirective::Always)
        .with_env_var("POSTGRES_USER", "postgres")
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .with_env_var("POSTGRES_DB", "asterforge")
        .start()
        .await
        .expect("failed to start postgres test container");
    let port = container
        .get_host_port_ipv4(testcontainers::core::IntoContainerPort::tcp(
            backend.container_port(),
        ))
        .await
        .expect("postgres test port should be exposed");
    let database_url = backend.database_url(port);
    let admin_database_url = backend.admin_database_url(port);

    wait_for_database(&database_url).await;
    drop_stale_test_databases(
        sea_orm::DbBackend::Postgres,
        &admin_database_url,
        &stale_databases,
    )
    .await;
    save_shared_test_container_state(&mut lock_file, backend, &state);

    SharedTestDatabaseContainer {
        _container: container,
        _lease: SharedTestContainerLease::new(backend),
        admin_database_url,
        database_url,
    }
}

async fn start_mysql_test_container() -> SharedTestDatabaseContainer {
    use testcontainers::{GenericImage, ImageExt, ReuseDirective, runners::AsyncRunner};

    let backend = TestDatabaseBackend::MySql;
    let mut lock_file = lock_shared_test_container_state(backend);
    let mut state = load_shared_test_container_state(&mut lock_file, backend);
    let stale_databases = prune_shared_test_container_state(&mut state);
    let current_pid = std::process::id();
    state.register_pid(current_pid);

    let container = GenericImage::new("mysql", "8.4")
        .with_exposed_port(testcontainers::core::IntoContainerPort::tcp(
            backend.container_port(),
        ))
        .with_container_name(backend.shared_container_name())
        .with_reuse(ReuseDirective::Always)
        .with_env_var("MYSQL_DATABASE", "asterforge")
        .with_env_var("MYSQL_USER", "aster")
        .with_env_var("MYSQL_PASSWORD", "asterpass")
        .with_env_var("MYSQL_ROOT_PASSWORD", "rootpass")
        .start()
        .await
        .expect("failed to start mysql test container");
    let port = container
        .get_host_port_ipv4(testcontainers::core::IntoContainerPort::tcp(
            backend.container_port(),
        ))
        .await
        .expect("mysql test port should be exposed");
    let database_url = backend.database_url(port);
    let admin_database_url = backend.admin_database_url(port);

    wait_for_database(&database_url).await;
    ensure_mysql_test_user_access(&admin_database_url, "aster").await;
    drop_stale_test_databases(
        sea_orm::DbBackend::MySql,
        &admin_database_url,
        &stale_databases,
    )
    .await;
    save_shared_test_container_state(&mut lock_file, backend, &state);

    SharedTestDatabaseContainer {
        _container: container,
        _lease: SharedTestContainerLease::new(backend),
        admin_database_url,
        database_url,
    }
}

async fn shared_test_database_urls(backend: TestDatabaseBackend) -> (String, String) {
    match backend {
        TestDatabaseBackend::Sqlite => {
            ("sqlite::memory:".to_string(), "sqlite::memory:".to_string())
        }
        TestDatabaseBackend::Postgres => {
            let container = POSTGRES_TEST_CONTAINER
                .get_or_init(start_postgres_test_container)
                .await;
            (
                container.admin_database_url.clone(),
                container.database_url.clone(),
            )
        }
        TestDatabaseBackend::MySql => {
            let container = MYSQL_TEST_CONTAINER
                .get_or_init(start_mysql_test_container)
                .await;
            (
                container.admin_database_url.clone(),
                container.database_url.clone(),
            )
        }
    }
}

fn sanitized_database_name_prefix(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();

    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "asterforge".to_string()
    } else {
        trimmed.to_string()
    }
}

fn isolated_database_name(base_name: &str, max_len: usize) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let reserved = "_it_".len() + suffix.len();
    let max_prefix_len = max_len.saturating_sub(reserved).max(1);
    let prefix = sanitized_database_name_prefix(base_name)
        .chars()
        .take(max_prefix_len)
        .collect::<String>();

    format!("{prefix}_it_{suffix}")
}

fn database_name_from_url(url: &url::Url) -> Option<String> {
    url.path_segments()
        .and_then(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .rfind(|segment| !segment.is_empty())
                .map(str::to_string)
        })
        .filter(|value| !value.is_empty())
}

fn replace_database_name(mut url: url::Url, database_name: &str) -> String {
    url.set_path(&format!("/{database_name}"));
    url.to_string()
}

fn quote_database_identifier(backend: sea_orm::DbBackend, database_name: &str) -> String {
    match backend {
        sea_orm::DbBackend::Postgres => format!("\"{}\"", database_name.replace('"', "\"\"")),
        sea_orm::DbBackend::MySql => format!("`{}`", database_name.replace('`', "``")),
        _ => database_name.to_string(),
    }
}

fn quote_mysql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

async fn provision_isolated_test_database_url_with_template(
    admin_database_url: &str,
    database_url: &str,
    template_database_name: Option<&str>,
) -> String {
    if database_url == "sqlite::memory:" || database_url.starts_with("sqlite://") {
        return database_url.to_string();
    }

    use sea_orm::ConnectionTrait;

    let admin_cfg = aster_forge::config::DatabaseConfig {
        url: admin_database_url.to_string(),
        pool_size: 1,
        retry_count: 0,
    };
    let admin_db = aster_forge::db::connect_with_metrics(
        &admin_cfg,
        aster_forge::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("isolated test database admin connection should succeed");
    let backend = admin_db.get_database_backend();
    let parsed_url = url::Url::parse(database_url).expect("test database url should parse");
    let base_name = database_name_from_url(&parsed_url).unwrap_or_else(|| "asterforge".to_string());

    let isolated_name = match backend {
        sea_orm::DbBackend::Postgres => isolated_database_name(&base_name, 63),
        sea_orm::DbBackend::MySql => isolated_database_name(&base_name, 64),
        _ => return database_url.to_string(),
    };
    let test_backend = test_backend_from_database_backend(backend)
        .expect("isolated database provisioning only supports postgres/mysql");
    remember_shared_test_database(test_backend, &isolated_name);

    let create_sql = match (backend, template_database_name) {
        (sea_orm::DbBackend::Postgres, Some(template_database_name)) => format!(
            "CREATE DATABASE {} TEMPLATE {}",
            quote_database_identifier(backend, &isolated_name),
            quote_database_identifier(backend, template_database_name)
        ),
        _ => format!(
            "CREATE DATABASE {}",
            quote_database_identifier(backend, &isolated_name)
        ),
    };
    admin_db
        .execute_unprepared(&create_sql)
        .await
        .expect("isolated test database should create");

    replace_database_name(parsed_url, &isolated_name)
}

async fn provision_isolated_test_database_url(
    admin_database_url: &str,
    database_url: &str,
) -> String {
    provision_isolated_test_database_url_with_template(admin_database_url, database_url, None).await
}

async fn build_postgres_database_template() -> PostgresDatabaseTemplate {
    let (admin_database_url, database_url) =
        shared_test_database_urls(TestDatabaseBackend::Postgres).await;
    let template_database_url =
        provision_isolated_test_database_url(&admin_database_url, &database_url).await;

    let db_cfg = aster_forge::config::DatabaseConfig {
        url: template_database_url.clone(),
        pool_size: 1,
        retry_count: 0,
    };
    let db = aster_forge::db::connect_with_metrics(
        &db_cfg,
        aster_forge::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("postgres template database connection should succeed");

    migration::Migrator::up(&db, None)
        .await
        .expect("postgres template database migrations should succeed");
    db.close()
        .await
        .expect("postgres template database should close cleanly");

    let template_database_name = url::Url::parse(&template_database_url)
        .ok()
        .and_then(|url| database_name_from_url(&url))
        .expect("postgres template database name should exist");

    PostgresDatabaseTemplate {
        database_name: template_database_name,
    }
}

async fn resolve_test_database_url_for(backend: TestDatabaseBackend) -> String {
    let (admin_database_url, database_url) = shared_test_database_urls(backend).await;
    match backend {
        TestDatabaseBackend::Postgres => {
            let template = POSTGRES_DATABASE_TEMPLATE
                .get_or_init(build_postgres_database_template)
                .await;
            provision_isolated_test_database_url_with_template(
                &admin_database_url,
                &database_url,
                Some(&template.database_name),
            )
            .await
        }
        _ => provision_isolated_test_database_url(&admin_database_url, &database_url).await,
    }
}

async fn resolve_test_database_url() -> String {
    resolve_test_database_url_for(configured_test_database_backend()).await
}

#[allow(dead_code)]
pub async fn postgres_test_database_url() -> String {
    resolve_test_database_url_for(TestDatabaseBackend::Postgres).await
}

#[allow(dead_code)]
pub async fn mysql_test_database_url() -> String {
    resolve_test_database_url_for(TestDatabaseBackend::MySql).await
}

#[allow(dead_code)]
pub async fn setup() -> AppState {
    init_test_process_state();
    let database_url = resolve_test_database_url().await;
    setup_with_database_url(&database_url).await
}

#[allow(dead_code)]
pub async fn setup_with_memory_cache() -> AppState {
    let base = setup().await;
    let cache_config = aster_forge::config::CacheConfig {
        enabled: true,
        backend: "memory".to_string(),
        default_ttl: 60,
        ..Default::default()
    };
    let cache = aster_forge::cache::create_cache(&cache_config).await;

    AppState {
        db_handles: base.db_handles,
        config: base.config,
        runtime_config: base.runtime_config,
        cache,
        mail_sender: aster_forge::services::mail_service::memory_sender(),
        metrics: aster_forge::metrics_core::NoopMetrics::arc(),
        background_task_dispatch_wakeup:
            aster_forge::runtime::AppState::new_background_task_dispatch_wakeup(),
    }
}

fn should_use_mysql_schema_template(database_url: &str) -> bool {
    database_url.starts_with("mysql://")
        && configured_test_database_backend() == TestDatabaseBackend::MySql
}

async fn load_mysql_schema_template(
    db: &sea_orm::DatabaseConnection,
    database_name: String,
) -> MySqlSchemaTemplate {
    use sea_orm::{ConnectionTrait, Statement};

    let tables = db
        .query_all_raw(Statement::from_string(
            sea_orm::DbBackend::MySql,
            "SHOW FULL TABLES WHERE Table_type = 'BASE TABLE'",
        ))
        .await
        .expect("mysql schema template should list tables");

    let mut table_names: Vec<String> = tables
        .into_iter()
        .map(|row| {
            row.try_get_by_index(0)
                .expect("mysql schema template table name should exist")
        })
        .collect();
    table_names.sort();

    let mut create_table_sql = Vec::with_capacity(table_names.len());
    for table_name in &table_names {
        let ddl_row = db
            .query_one_raw(Statement::from_string(
                sea_orm::DbBackend::MySql,
                format!(
                    "SHOW CREATE TABLE {}",
                    quote_database_identifier(sea_orm::DbBackend::MySql, table_name)
                ),
            ))
            .await
            .expect("mysql schema template should load table ddl")
            .expect("mysql schema template show create table should return one row");

        let ddl: String = ddl_row
            .try_get_by_index(1)
            .expect("mysql schema template ddl should exist");
        create_table_sql.push(ddl);
    }

    MySqlSchemaTemplate {
        database_name,
        create_table_sql,
    }
}

async fn build_mysql_schema_template() -> MySqlSchemaTemplate {
    let (admin_database_url, database_url) =
        shared_test_database_urls(TestDatabaseBackend::MySql).await;
    let template_database_url =
        provision_isolated_test_database_url(&admin_database_url, &database_url).await;

    let db_cfg = aster_forge::config::DatabaseConfig {
        url: template_database_url.clone(),
        pool_size: 1,
        retry_count: 0,
    };
    let db = aster_forge::db::connect_with_metrics(
        &db_cfg,
        aster_forge::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("mysql schema template connection should succeed");

    migration::Migrator::up(&db, None)
        .await
        .expect("mysql schema template migrations should succeed");

    let template_database_name = url::Url::parse(&template_database_url)
        .ok()
        .and_then(|url| database_name_from_url(&url))
        .expect("mysql schema template database name should exist");

    load_mysql_schema_template(&db, template_database_name).await
}

async fn clone_mysql_schema_from_template(db: &sea_orm::DatabaseConnection) {
    use sea_orm::ConnectionTrait;

    let template = MYSQL_SCHEMA_TEMPLATE
        .get_or_init(build_mysql_schema_template)
        .await;

    set_foreign_key_checks(db, false)
        .await
        .expect("mysql schema clone should disable foreign key checks");

    for ddl in &template.create_table_sql {
        db.execute_unprepared(ddl)
            .await
            .expect("mysql schema clone should create table");
    }

    db.execute_unprepared(&format!(
        "INSERT INTO seaql_migrations SELECT * FROM {}.seaql_migrations",
        quote_database_identifier(sea_orm::DbBackend::MySql, &template.database_name)
    ))
    .await
    .expect("mysql schema clone should copy seaql_migrations rows");

    set_foreign_key_checks(db, true)
        .await
        .expect("mysql schema clone should restore foreign key checks");
}

#[allow(dead_code)]
pub async fn setup_with_database_url(database_url: &str) -> AppState {
    init_test_process_state();

    let db_cfg = aster_forge::config::DatabaseConfig {
        url: database_url.to_string(),
        pool_size: 1,
        retry_count: 0,
    };
    let writer = aster_forge::db::connect_with_metrics(
        &db_cfg,
        aster_forge::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("test database should connect");

    if should_use_mysql_schema_template(database_url) {
        clone_mysql_schema_from_template(&writer).await;
    } else {
        migration::Migrator::up(&writer, None)
            .await
            .expect("test database migrations should run");
    }

    aster_forge::services::system_config_service::ensure_defaults(&writer)
        .await
        .expect("system config defaults should seed");

    let test_dir = format!("/tmp/asterforge-test-{}", uuid::Uuid::new_v4());
    let temp_dir = format!("{test_dir}/temp");
    std::fs::create_dir_all(&temp_dir).expect("test temp dir should exist");

    let config = std::sync::Arc::new(aster_forge::config::Config {
        server: aster_forge::config::ServerConfig {
            temp_dir,
            ..Default::default()
        },
        database: db_cfg.clone(),
        auth: aster_forge::config::AuthConfig {
            jwt_secret: "test-secret-key-for-integration-tests".to_string(),
            bootstrap_insecure_cookies: true,
            ..Default::default()
        },
        cache: aster_forge::config::CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    });

    let _ = aster_forge::config::set_config_for_test(config.clone());

    let runtime_config = std::sync::Arc::new(aster_forge::config::RuntimeConfig::new());
    runtime_config
        .reload(&writer)
        .await
        .expect("runtime config should reload");

    let cache = aster_forge::cache::create_cache(&config.cache).await;
    let db_handles = aster_forge::db::connect_reader_for_writer_with_metrics(
        &db_cfg,
        writer,
        aster_forge::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("test db handles should initialize");

    AppState {
        db_handles,
        config,
        runtime_config,
        cache,
        mail_sender: aster_forge::services::mail_service::memory_sender(),
        metrics: aster_forge::metrics_core::NoopMetrics::arc(),
        background_task_dispatch_wakeup:
            aster_forge::runtime::AppState::new_background_task_dispatch_wakeup(),
    }
}

#[allow(dead_code)]
pub fn bearer_header(access_token: impl AsRef<str>) -> (&'static str, String) {
    ("Authorization", format!("Bearer {}", access_token.as_ref()))
}

#[allow(dead_code)]
pub fn access_cookie_header(access_token: impl AsRef<str>) -> String {
    format!("aster_access={}", access_token.as_ref())
}

#[allow(dead_code)]
pub fn refresh_cookie_header(
    refresh_token: impl AsRef<str>,
    csrf_token: impl AsRef<str>,
) -> String {
    format!(
        "aster_refresh={}; aster_csrf={}",
        refresh_token.as_ref(),
        csrf_token.as_ref()
    )
}

#[allow(dead_code)]
pub fn access_and_refresh_cookie_header(
    access_token: impl AsRef<str>,
    refresh_token: impl AsRef<str>,
    csrf_token: impl AsRef<str>,
) -> String {
    format!(
        "aster_access={}; aster_refresh={}; aster_csrf={}",
        access_token.as_ref(),
        refresh_token.as_ref(),
        csrf_token.as_ref()
    )
}

#[allow(dead_code)]
pub fn csrf_header(csrf_token: impl AsRef<str>) -> (&'static str, String) {
    ("X-CSRF-Token", csrf_token.as_ref().to_string())
}

#[allow(dead_code)]
pub fn extract_cookie<B>(resp: &actix_web::dev::ServiceResponse<B>, name: &str) -> Option<String> {
    resp.headers()
        .get_all(actix_web::http::header::SET_COOKIE)
        .filter_map(|value| value.to_str().ok())
        .find_map(|raw| {
            raw.split(';')
                .next()
                .and_then(|pair| pair.strip_prefix(&format!("{name}=")))
                .map(str::to_string)
        })
}

#[allow(dead_code)]
pub fn system_config_model(key: &str, value: &str) -> aster_forge::entities::system_config::Model {
    aster_forge::entities::system_config::Model {
        id: 0,
        key: key.to_string(),
        value: value.to_string(),
        value_type: aster_forge::types::SystemConfigValueType::String,
        requires_restart: false,
        is_sensitive: false,
        source: aster_forge::types::SystemConfigSource::System,
        visibility: aster_forge::types::SystemConfigVisibility::Private,
        namespace: String::new(),
        category: aster_forge::config::definitions::CONFIG_CATEGORY_SITE.to_string(),
        description: "test".to_string(),
        updated_at: chrono::Utc::now(),
        updated_by: None,
    }
}

#[macro_export]
macro_rules! create_test_app {
    ($state:expr) => {{
        use actix_web::{App, test, web};

        let state = $state;
        let metrics = state.metrics.clone();

        test::init_service(
            App::new()
                .wrap(aster_forge::api::middleware::request_id::RequestIdMiddleware)
                .wrap(aster_forge::api::middleware::cors::RuntimeCors)
                .wrap(aster_forge::api::middleware::security_headers::default_headers())
                .wrap(aster_forge::api::middleware::metrics::MetricsMiddleware)
                .app_data(web::PayloadConfig::new(10 * 1024 * 1024))
                .app_data(web::JsonConfig::default().limit(1024 * 1024))
                .app_data(web::Data::new(metrics))
                .app_data(web::Data::new(state))
                .service(aster_forge::api::routes::health::routes())
                .service(web::scope("/api/v1").configure(aster_forge::api::routes::configure_api)),
        )
        .await
    }};
}

#[macro_export]
macro_rules! assert_service_status {
    ($app:expr, $req:expr, $status:expr) => {{
        use actix_web::test;

        let result = test::try_call_service(&$app, $req).await;
        match result {
            Ok(resp) => assert_eq!(resp.status(), $status),
            Err(err) => assert_eq!(err.error_response().status(), $status),
        }
    }};
    ($app:expr, $req:expr, $status:expr, $msg:expr) => {{
        use actix_web::test;

        let result = test::try_call_service(&$app, $req).await;
        match result {
            Ok(resp) => assert_eq!(resp.status(), $status, $msg),
            Err(err) => assert_eq!(err.error_response().status(), $status, $msg),
        }
    }};
}

#[macro_export]
macro_rules! setup_admin {
    ($app:expr) => {{
        use actix_web::test;
        use serde_json::Value;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/setup")
            .peer_addr("127.0.0.1:12345".parse().unwrap())
            .set_json(serde_json::json!({
                "username": "admin",
                "email": "admin@example.com",
                "password": "password1234"
            }))
            .to_request();
        let resp = test::call_service(&$app, req).await;
        assert_eq!(resp.status(), 200, "admin setup should return 200");
        let access_token =
            common::extract_cookie(&resp, "aster_access").expect("admin setup access cookie missing");
        let body: Value = test::read_body_json(resp).await;
        assert!(body["data"]["expires_in"].is_number());
        access_token
    }};
}

#[macro_export]
macro_rules! register_user {
    ($app:expr, $username:expr, $email:expr, $password:expr) => {{
        use actix_web::test;
        use serde_json::Value;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/register")
            .peer_addr("127.0.0.1:12345".parse().unwrap())
            .set_json(serde_json::json!({
                "username": $username,
                "email": $email,
                "password": $password
            }))
            .to_request();
        let resp = test::call_service(&$app, req).await;
        assert_eq!(resp.status(), 200, "register should return 200");
        let access_token =
            common::extract_cookie(&resp, "aster_access").expect("register access cookie missing");
        let body: Value = test::read_body_json(resp).await;
        assert!(body["data"]["expires_in"].is_number());
        access_token
    }};
}

#[macro_export]
macro_rules! login_user {
    ($app:expr, $identifier:expr, $password:expr) => {{
        use actix_web::test;
        use serde_json::Value;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/login")
            .peer_addr("127.0.0.1:12345".parse().unwrap())
            .set_json(serde_json::json!({
                "identifier": $identifier,
                "password": $password
            }))
            .to_request();
        let resp = test::call_service(&$app, req).await;
        assert_eq!(resp.status(), 200, "login should return 200");
        let access_token =
            common::extract_cookie(&resp, "aster_access").expect("login access cookie missing");
        let body: Value = test::read_body_json(resp).await;
        assert!(body["data"]["expires_in"].is_number());
        access_token
    }};
}
