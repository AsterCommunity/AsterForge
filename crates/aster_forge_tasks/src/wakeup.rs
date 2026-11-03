//! Coalescing broadcast notifications for independently polling consumers.

use tokio::sync::watch;

/// Per-instance enqueue/configuration signal shared by any number of consumers.
///
/// Each runner subscribes before its startup claim. Unlike a shared `Notify::notify_one`,
/// a notification reaches every subscriber; unlike `notify_waiters`, a subscribed runner
/// retains an unread change while its claim is in flight. Repeated changes coalesce.
/// Durable storage and periodic polling remain authoritative, including across processes.
#[derive(Debug, Clone)]
pub struct TaskWakeup {
    sender: watch::Sender<()>,
    id: uuid::Uuid,
}

impl Default for TaskWakeup {
    fn default() -> Self {
        Self {
            sender: watch::channel(()).0,
            id: uuid::Uuid::new_v4(),
        }
    }
}

impl TaskWakeup {
    /// Notify after durable enqueue/configuration commit. No payload or credentials are stored.
    pub fn notify(&self) {
        self.sender.send_replace(());
        tracing::debug!(
            wakeup_id = %self.id,
            subscribers = self.sender.receiver_count(),
            "task consumers notified"
        );
    }

    pub(crate) fn id(&self) -> uuid::Uuid {
        self.id
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<()> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn all_subscribers_retain_notifications_without_registered_waiters() {
        let wakeup = TaskWakeup::default();
        let mut first = wakeup.subscribe();
        let mut second = wakeup.subscribe();
        wakeup.notify();
        wakeup.notify();
        first.changed().await.unwrap();
        second.changed().await.unwrap();
        assert!(!first.has_changed().unwrap());
        assert!(!second.has_changed().unwrap());
        assert!(!TaskWakeup::default().subscribe().has_changed().unwrap());
    }
    #[tokio::test]
    async fn canceled_wait_and_separate_instances_do_not_lose_or_share_changes() {
        let first = TaskWakeup::default();
        let second = TaskWakeup::default();
        let mut receiver = first.subscribe();
        let isolated = second.subscribe();
        {
            let waiting = receiver.changed();
            tokio::pin!(waiting);
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    waiting.as_mut().poll(cx).is_pending()
                ))
                .await
            );
        }
        first.notify();
        receiver.changed().await.unwrap();
        assert!(!isolated.has_changed().unwrap());
        assert_ne!(first.id(), second.id());
    }
}
