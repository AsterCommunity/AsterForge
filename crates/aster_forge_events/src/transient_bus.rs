use std::sync::Arc;

use tokio::sync::broadcast;

/// Process-local event broadcast paired with an optional shared transport.
///
/// The local channel is always available, including in single-process deployments and tests.
/// Products decide how to encode events for `R`, how to publish through it, and how remote items
/// are decoded back into the local channel.
pub struct TransientEventBus<T, R = ()> {
    local: broadcast::Sender<T>,
    transport: Option<Arc<R>>,
}

impl<T, R> Clone for TransientEventBus<T, R> {
    fn clone(&self) -> Self {
        Self {
            local: self.local.clone(),
            transport: self.transport.clone(),
        }
    }
}

impl<T, R> TransientEventBus<T, R>
where
    T: Clone,
{
    /// Creates a process-local bus without a shared transport.
    pub fn new(capacity: usize) -> Self {
        Self::from_optional_transport(capacity, None)
    }

    /// Creates a bus with a shared transport.
    pub fn with_transport(capacity: usize, transport: R) -> Self {
        Self::from_optional_transport(capacity, Some(transport))
    }

    /// Creates a bus from an optional shared transport.
    pub fn from_optional_transport(capacity: usize, transport: Option<R>) -> Self {
        let (local, _) = broadcast::channel(capacity.max(1));
        Self {
            local,
            transport: transport.map(Arc::new),
        }
    }

    /// Publishes one event to process-local subscribers.
    pub fn publish_local(&self, event: T) -> Result<usize, broadcast::error::SendError<T>> {
        self.local.send(event)
    }

    /// Subscribes to future process-local events.
    pub fn subscribe(&self) -> broadcast::Receiver<T> {
        self.local.subscribe()
    }

    /// Returns the number of active process-local subscribers.
    pub fn local_subscriber_count(&self) -> usize {
        self.local.receiver_count()
    }

    /// Returns whether a shared transport is configured.
    pub fn has_transport(&self) -> bool {
        self.transport.is_some()
    }

    /// Returns the shared transport, if configured.
    pub fn transport(&self) -> Option<Arc<R>> {
        self.transport.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::TransientEventBus;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn local_bus_delivers_to_all_current_subscribers() {
        let bus = TransientEventBus::<String>::new(4);
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();

        assert_eq!(bus.local_subscriber_count(), 2);
        assert_eq!(bus.publish_local("event".to_string()).expect("publish"), 2);
        assert_eq!(first.recv().await, Ok("event".to_string()));
        assert_eq!(second.recv().await, Ok("event".to_string()));
    }

    #[test]
    fn zero_capacity_is_clamped_and_publish_without_subscribers_returns_event() {
        let bus = TransientEventBus::<String>::new(0);

        let error = bus
            .publish_local("unobserved".to_string())
            .expect_err("publish without subscribers should report the undelivered event");
        assert_eq!(error.0, "unobserved");
    }

    #[tokio::test]
    async fn bounded_local_channel_reports_lag() {
        let bus = TransientEventBus::<u8>::new(1);
        let mut receiver = bus.subscribe();

        assert_eq!(bus.publish_local(1).expect("publish first"), 1);
        assert_eq!(bus.publish_local(2).expect("publish second"), 1);
        assert_eq!(
            receiver.recv().await,
            Err(broadcast::error::RecvError::Lagged(1))
        );
        assert_eq!(receiver.recv().await, Ok(2));
    }

    #[tokio::test]
    async fn cloned_bus_shares_local_subscribers() {
        let bus = TransientEventBus::<u8>::new(2);
        let cloned = bus.clone();
        let mut receiver = bus.subscribe();

        assert_eq!(cloned.publish_local(7).expect("publish from clone"), 1);
        assert_eq!(receiver.recv().await, Ok(7));
    }

    #[test]
    fn optional_transport_is_shared_across_clones() {
        let bus = TransientEventBus::<u8, String>::with_transport(2, "transport".to_string());
        let cloned = bus.clone();

        assert!(bus.has_transport());
        assert!(Arc::ptr_eq(
            &bus.transport().expect("transport"),
            &cloned.transport().expect("cloned transport")
        ));

        let local = TransientEventBus::<u8, String>::new(2);
        assert!(!local.has_transport());
        assert!(local.transport().is_none());
    }
}
