//! Static configuration loader integration tests.

use std::fs;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_config_uses_generated_data_paths() {
    let config = {{crate_name}}::config::AppConfig::default();

    assert_eq!(config.server.temp_dir, ".tmp");
    assert_eq!(
        config.database.url,
        "sqlite://{{project-name}}.db?mode=rwc"
    );
    assert_eq!(config.logging.file, "{{project-name}}.log");
}

#[test]
fn default_config_file_is_created_under_data_dir() {
    let _guard = EnvGuard::capture();
    let dir = unique_project_temp_dir();
    fs::create_dir_all(&dir).expect("create temp config dir");
    let config_path = dir.join("data").join("config.toml");

    unsafe {
        std::env::set_var(
            {{crate_name}}::config::CONFIG_ENV_VAR,
            &config_path,
        );
    }
    let loaded = {{crate_name}}::config::load().expect("load config");
    let generated = fs::read_to_string(&config_path).expect("read generated config");

    assert_eq!(
        loaded.server.temp_dir,
        runtime_relative_path(dir.join("data").join(".tmp"))
    );
    assert_eq!(
        loaded.database.url,
        format!(
            "sqlite://{}?mode=rwc",
            runtime_relative_path(dir.join("data").join("{{project-name}}.db"))
        )
    );
    assert!(config_path.exists());
    assert!(generated.contains("# {{project-name}} configuration file"));
    assert!(generated.contains(r#"temp_dir = ".tmp""#));
    assert!(generated.contains(r#"url = "sqlite://{{project-name}}.db?mode=rwc""#));
}

#[test]
fn config_file_overrides_are_loaded_and_relative_paths_resolve_from_config_dir() {
    let _guard = EnvGuard::capture();
    let dir = unique_project_temp_dir();
    let config_path = dir.join("data").join("config.toml");
    fs::create_dir_all(
        config_path
            .parent()
            .expect("config path should have parent"),
    )
    .expect("create temp config dir");
    fs::write(
        &config_path,
        r#"
[server]
temp_dir = ".tmp"

[database]
url = "sqlite://custom.db?mode=rwc"

[logging]
file = "service.log"
"#,
    )
    .expect("write config");

    unsafe {
        std::env::set_var(
            {{crate_name}}::config::CONFIG_ENV_VAR,
            &config_path,
        );
    }
    let loaded = {{crate_name}}::config::load().expect("load config");

    assert_eq!(
        loaded.server.temp_dir,
        runtime_relative_path(dir.join("data").join(".tmp"))
    );
    assert_eq!(
        loaded.database.url,
        format!(
            "sqlite://{}?mode=rwc",
            runtime_relative_path(dir.join("data").join("custom.db"))
        )
    );
    assert_eq!(
        loaded.logging.file,
        runtime_relative_path(dir.join("data").join("service.log"))
    );
}

#[test]
fn environment_overrides_static_config() {
    let _guard = EnvGuard::capture();

    unsafe {
        std::env::remove_var({{crate_name}}::config::CONFIG_ENV_VAR);
        std::env::set_var("ASTER__SERVER__HOST", "0.0.0.0");
    }

    let loaded = {{crate_name}}::config::load().expect("load config");

    assert_eq!(loaded.server.host, "0.0.0.0");
}

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    config_path: Option<std::ffi::OsString>,
    server_host: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn capture() -> Self {
        let lock = ENV_LOCK.lock().expect("env lock poisoned");
        Self {
            _lock: lock,
            config_path: std::env::var_os({{crate_name}}::config::CONFIG_ENV_VAR),
            server_host: std::env::var_os("ASTER__SERVER__HOST"),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env_var(
                {{crate_name}}::config::CONFIG_ENV_VAR,
                self.config_path.clone(),
            );
            restore_env_var("ASTER__SERVER__HOST", self.server_host.clone());
        }
    }
}

unsafe fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => unsafe {
            std::env::set_var(key, value);
        },
        None => unsafe {
            std::env::remove_var(key);
        },
    }
}

fn unique_project_temp_dir() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::current_dir()
        .expect("resolve current dir")
        .join("target")
        .join("config-loader-tests")
        .join(format!("{{project-name}}-{nanos}"))
}

fn runtime_relative_path(path: std::path::PathBuf) -> String {
    let cwd = std::env::current_dir().expect("resolve current dir");
    path.strip_prefix(&cwd)
        .expect("test path should be under current dir")
        .to_string_lossy()
        .into_owned()
}
