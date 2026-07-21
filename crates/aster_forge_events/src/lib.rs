//! Product-neutral transient event transport primitives.
//!
//! This crate owns broker connection lifecycle, reconnect backoff, shutdown cancellation, and
//! raw payload delivery. Products own payload schemas, authorization, origin filtering, and the
//! local event semantics built on top of the transport.

mod supervisor;
mod transient_bus;

#[cfg(feature = "redis")]
mod redis_transport;

pub use supervisor::{
    EventConnectionObservation, EventConnectionState, EventReconnectPolicy,
    EventSubscriptionSource, EventSubscriptionUpdate, supervise_event_subscription,
};
pub use transient_bus::TransientEventBus;

#[cfg(feature = "redis")]
pub use redis_transport::{
    EventConnectionObserver, RedisEventBus, RedisEventBusError, RedisEventReconnectPolicy,
    RedisEventSubscription,
};
