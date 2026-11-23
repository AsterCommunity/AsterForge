use sea_orm::{Database, DatabaseConnection};
use std::time::Duration;

pub(crate) async fn connect_with_retry(database_url: &str, service: &str) -> DatabaseConnection {
    let deadline = tokio::time::Instant::now() + Duration::from_mins(1);
    loop {
        match Database::connect(database_url).await {
            Ok(database) => return database,
            Err(error) if tokio::time::Instant::now() >= deadline => {
                panic!("{service} test database did not become ready: {error}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

/// Probe once under a total budget, including connection, query and pool shutdown.
/// Dropping a timed-out probe also drops its owned connection/pool; no cleanup await can hold
/// the fixture lock beyond that same deadline.
pub(crate) async fn fixture_is_usable(url: &str) -> bool {
    use sea_orm::ConnectionTrait;
    let budget = Duration::from_secs(5);
    let mut options = sea_orm::ConnectOptions::new(url);
    options
        .max_connections(1)
        .connect_timeout(budget)
        .sqlx_logging(false);
    bounded_probe(
        budget,
        Box::pin(async move {
            match Database::connect(options).await {
                Ok(connection) => {
                    let usable = connection.execute_unprepared("SELECT 1").await.is_ok();
                    connection.close().await.is_ok() && usable
                }
                Err(_) => false,
            }
        }),
    )
    .await
}

async fn bounded_probe(budget: Duration, probe: impl std::future::Future<Output = bool>) -> bool {
    tokio::time::timeout(budget, probe).await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct ConnectionGuard<'a>(&'a AtomicBool);
    impl Drop for ConnectionGuard<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn probe_timeout_cancels_connect_query_and_close_without_retaining_resources() {
        for stalled_stage in 0..3 {
            let released = AtomicBool::new(false);
            let probe = async {
                let _connection = ConnectionGuard(&released);
                for stage in 0..3 {
                    if stage == stalled_stage {
                        std::future::pending::<()>().await;
                    }
                }
                true
            };
            assert!(!bounded_probe(Duration::from_millis(10), probe).await);
            assert!(released.load(Ordering::SeqCst));
        }
        assert!(bounded_probe(Duration::from_secs(1), async { true }).await);
        assert!(!bounded_probe(Duration::from_secs(1), async { false }).await);
    }
}
