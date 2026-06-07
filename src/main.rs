//! AsterForge service entrypoint.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use actix_web::{App, HttpServer, middleware, web};
use tokio_util::sync::CancellationToken;

const HTTP_SHUTDOWN_TIMEOUT_SECS: u64 = 8;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "jemalloc", not(target_env = "msvc"), target_os = "linux"))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static malloc_conf: Option<&'static std::ffi::c_char> = Some(unsafe {
    union Conf {
        bytes: &'static u8,
        ptr: &'static std::ffi::c_char,
    }

    // `narenas:1` lowers idle memory for the self-hosted default profile, but
    // can become allocator contention under high concurrency.
    Conf {
        bytes: &b"narenas:1,dirty_decay_ms:1000,muzzy_decay_ms:1000,background_thread:true\0"[0],
    }
    .ptr
});

#[cfg(all(
    feature = "jemalloc",
    not(target_env = "msvc"),
    any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )
))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static malloc_conf: Option<&'static std::ffi::c_char> = Some(unsafe {
    union Conf {
        bytes: &'static u8,
        ptr: &'static std::ffi::c_char,
    }

    Conf {
        bytes: &b"narenas:1,dirty_decay_ms:1000,muzzy_decay_ms:1000\0"[0],
    }
    .ptr
});

#[cfg(all(debug_assertions, not(feature = "jemalloc")))]
#[global_allocator]
static GLOBAL: aster_forge::alloc::TrackingAlloc = aster_forge::alloc::TrackingAlloc;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    aster_forge::runtime::panic::install_panic_hook();
    dotenvy::dotenv().ok();

    let config = aster_forge::config::init_config()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let logging = aster_forge::runtime::logging::init_logging(&config.logging);
    let _log_guard = logging.guard;
    if let Some(warning) = logging.warning {
        tracing::warn!("{warning}");
    }

    let prepared = aster_forge::runtime::startup::prepare(config)
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?;

    let host = prepared.state.config.server.host.clone();
    let port = prepared.state.config.server.port;
    let workers = if prepared.state.config.server.workers == 0 {
        num_cpus::get()
    } else {
        prepared.state.config.server.workers
    };

    tracing::info!(host = %host, port, workers, "starting AsterForge HTTP service");

    let shutdown_token = CancellationToken::new();
    let state = web::Data::new(prepared.state);
    let shutdown_db_handles = state.get_ref().db_handles.clone();
    let shutdown_data = web::Data::new(shutdown_token.clone());
    let metrics_data = web::Data::new(state.get_ref().metrics.clone());
    let background_tasks = aster_forge::runtime::tasks::spawn_runtime_background_tasks(
        state.clone(),
        shutdown_token.clone(),
    );

    let app_state = state.clone();
    let app_shutdown_data = shutdown_data.clone();
    let app_metrics_data = metrics_data.clone();

    let server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Compress::default())
            .wrap(middleware::Logger::default())
            .wrap(aster_forge::api::middleware::request_id::RequestIdMiddleware)
            .wrap(aster_forge::api::middleware::cors::RuntimeCors)
            .wrap(aster_forge::api::middleware::security_headers::default_headers())
            .wrap(aster_forge::api::middleware::metrics::MetricsMiddleware)
            .app_data(app_state.clone())
            .app_data(app_shutdown_data.clone())
            .app_data(app_metrics_data.clone())
            .configure(aster_forge::api::configure)
    })
    .workers(workers)
    .bind((host.as_str(), port))?
    .shutdown_timeout(HTTP_SHUTDOWN_TIMEOUT_SECS)
    .disable_signals()
    .run();

    let handle = server.handle();
    let shutdown_signal = shutdown_token.clone();
    tokio::spawn(async move {
        aster_forge::runtime::shutdown::wait_for_signal().await;
        shutdown_signal.cancel();
        handle.stop(true).await;
    });

    let server_result = server.await;
    tracing::info!("server stopped");
    aster_forge::runtime::shutdown::record_server_shutdown(state.get_ref()).await;
    aster_forge::runtime::shutdown::perform_shutdown(background_tasks, shutdown_db_handles).await;
    server_result
}
