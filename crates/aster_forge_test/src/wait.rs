//! Readiness polling helpers for integration tests.

use std::future::Future;
use std::time::{Duration, Instant};

/// Polls `check` until it returns `true` or `timeout` elapses.
///
/// Returns whether the condition was observed before the deadline. A final attempt is made
/// after the last sleep when the deadline passes between attempts.
pub async fn wait_until<F, Fut>(timeout: Duration, interval: Duration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if check().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::wait_until;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn wait_until_returns_true_once_condition_holds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        let ready = wait_until(
            Duration::from_secs(1),
            Duration::from_millis(5),
            move || {
                let counter = counter.clone();
                async move { counter.fetch_add(1, Ordering::SeqCst) + 1 >= 3 }
            },
        )
        .await;

        assert!(ready);
        assert!(attempts.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn wait_until_times_out_when_condition_never_holds() {
        let ready = wait_until(
            Duration::from_millis(30),
            Duration::from_millis(5),
            || async { false },
        )
        .await;

        assert!(!ready);
    }
}
