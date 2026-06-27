//! Product process entrypoint.
//!
//! Keep `main.rs` thin. The entrypoint installs common process hooks, prepares product resources,
//! and then hands control to the Forge runtime component graph.

use std::io;
use std::path::Path;

/// Runs the service until a shutdown signal is received.
pub async fn run() -> io::Result<()> {
    aster_forge_panic::install_panic_hook(aster_forge_panic::PanicHookConfig::new(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_REPOSITORY"),
    ));

    let config = crate::config::load().map_err(to_io_error)?;
    prepare_runtime_directories(&config).map_err(to_io_error)?;
    let logging = aster_forge_logging::init_logging(&config.logging);
    if let Some(warning) = logging.warning {
        eprintln!("logging warning: {warning}");
    }
    let _logging_guard = logging.guard;

    let state = crate::runtime::assembly::prepare_state(config)
        .await
        .map_err(to_io_error)?;
    crate::runtime::assembly::run(state).await
}

fn to_io_error(error: impl ToString) -> io::Error {
    io::Error::other(error.to_string())
}

fn prepare_runtime_directories(config: &crate::config::AppConfig) -> crate::errors::Result<()> {
    std::fs::create_dir_all(&config.server.temp_dir)?;
    create_sqlite_parent_dir(&config.database.url)?;
    if !config.logging.file.is_empty() {
        create_parent_dir(&config.logging.file)?;
    }
    Ok(())
}

fn create_sqlite_parent_dir(url: &str) -> crate::errors::Result<()> {
    let Some(path_and_query) = url.strip_prefix("sqlite://") else {
        return Ok(());
    };
    if path_and_query.starts_with(":memory:") || path_and_query.contains("mode=memory") {
        return Ok(());
    }
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _query)| path);
    create_parent_dir(path)
}

fn create_parent_dir(path: &str) -> crate::errors::Result<()> {
    let parent = Path::new(path).parent();
    if let Some(parent) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
