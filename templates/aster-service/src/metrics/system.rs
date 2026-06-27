//! Periodic process-level metrics updater.

use std::sync::OnceLock;

use crate::metrics::registry::{PROCESS_STARTED_AT, get_metrics};

pub async fn system_metrics_updater_task(shutdown_token: tokio_util::sync::CancellationToken) {
    use parking_lot::Mutex;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        tokio::select! {
            biased;
            _ = shutdown_token.cancelled() => break,
            _ = interval.tick() => {}
        }

        if shutdown_token.is_cancelled() {
            break;
        }

        let Some(metrics) = get_metrics() else {
            continue;
        };

        let update = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let pid = Pid::from_u32(std::process::id());
            let sys_mutex = SYSTEM.get_or_init(|| Mutex::new(System::new()));
            let mut sys = sys_mutex.lock();
            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing().with_memory().with_cpu(),
            );
            if let Some(process) = sys.process(pid) {
                metrics
                    .process_memory_rss_bytes
                    .set(process.memory() as f64);
                let cpu_millis = i64::try_from(process.accumulated_cpu_time()).unwrap_or(i64::MAX);
                metrics.process_cpu_milliseconds_total.set(cpu_millis);
            }
            let uptime = PROCESS_STARTED_AT
                .get()
                .map(std::time::Instant::elapsed)
                .unwrap_or_default()
                .as_secs_f64();
            metrics.uptime_seconds.set(uptime);
        }));

        if let Err(panic) = update {
            tracing::error!(panic = %panic_message(panic), "system metrics updater panicked");
        }
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}
