//! Integration tests for Redis backend edge semantics against a real server.
//!
//! These tests pin the zero-TTL contract and the circuit-breaker error classification that the
//! fake-driver unit tests cover mechanically: a real Redis rejects `SETEX 0`, and a real
//! `WRONGTYPE` reply is a command error that must not degrade the backend.

use aster_forge_cache::{CacheBackend, RedisCache};
use aster_forge_test::{redis::RedisTestContainer, suite::TestContainerSuite};
use redis::AsyncCommands;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{OnceLock, atomic};

fn test_suite() -> &'static TestContainerSuite {
    static SUITE: OnceLock<TestContainerSuite> = OnceLock::new();
    SUITE.get_or_init(|| TestContainerSuite::new("asterforge-cache"))
}

/// The shared container keeps data across runs, so keys must be unique per test process.
fn unique_key(name: &str) -> String {
    static COUNTER: atomic::AtomicUsize = AtomicUsize::new(0);
    format!(
        "asterforge-cache-it:{}:{}:{name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

async fn redis_cache() -> (RedisTestContainer, RedisCache) {
    let container = RedisTestContainer::start(test_suite()).await;
    let cache = RedisCache::new(container.url(), 60)
        .await
        .expect("Redis cache should connect to the test container");
    (container, cache)
}

#[tokio::test]
async fn zero_ttl_set_expires_immediately_and_keeps_backend_healthy() {
    let (_container, cache) = redis_cache().await;
    let key = unique_key("zero-set");

    cache.set_bytes(&key, b"v1".to_vec(), Some(60)).await;
    assert_eq!(cache.get_bytes(&key).await, Some(b"v1".to_vec()));

    cache.set_bytes(&key, b"v2".to_vec(), Some(0)).await;
    assert_eq!(cache.get_bytes(&key).await, None);

    cache
        .health_check()
        .await
        .expect("a zero-TTL write must not degrade the backend");
    cache.set_bytes(&key, b"v3".to_vec(), Some(60)).await;
    assert_eq!(cache.get_bytes(&key).await, Some(b"v3".to_vec()));

    cache.delete(&key).await;
}

#[tokio::test]
async fn zero_ttl_set_if_absent_reports_absence_without_retaining() {
    let (_container, cache) = redis_cache().await;
    let key = unique_key("zero-nx");

    assert!(
        cache
            .set_bytes_if_absent(&key, b"v".to_vec(), Some(0))
            .await
    );
    assert_eq!(cache.get_bytes(&key).await, None);
    assert!(
        cache
            .set_bytes_if_absent(&key, b"v2".to_vec(), Some(0))
            .await,
        "nothing is retained, so a second zero-TTL insert also succeeds"
    );

    cache.set_bytes(&key, b"live".to_vec(), Some(60)).await;
    assert!(
        !cache
            .set_bytes_if_absent(&key, b"v".to_vec(), Some(0))
            .await,
        "an existing live value rejects the insert"
    );
    assert_eq!(cache.get_bytes(&key).await, Some(b"live".to_vec()));

    cache.delete(&key).await;
}

#[tokio::test]
async fn wrongtype_reply_is_a_single_operation_error_not_an_outage() {
    let (container, cache) = redis_cache().await;
    let list_key = unique_key("wrongtype");
    let data_key = unique_key("wrongtype-data");

    let client = redis::Client::open(container.url()).expect("redis client should open");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis connection should establish");
    conn.rpush::<_, _, ()>(&list_key, "item")
        .await
        .expect("seed a list key");

    // GET on a list key answers WRONGTYPE: the read reports a miss...
    assert_eq!(cache.get_bytes(&list_key).await, None);
    // ...and the backend keeps asking Redis instead of hiding behind the fallback.
    assert_eq!(cache.get_bytes(&list_key).await, None);

    cache
        .health_check()
        .await
        .expect("WRONGTYPE must not open the fallback circuit");
    cache
        .set_bytes(&data_key, b"value".to_vec(), Some(60))
        .await;
    assert_eq!(cache.get_bytes(&data_key).await, Some(b"value".to_vec()));

    cache.delete(&data_key).await;
    conn.del::<_, ()>(&list_key)
        .await
        .expect("clean up list key");
}

#[tokio::test]
async fn one_second_ttl_expires_on_the_server() {
    let (_container, cache) = redis_cache().await;
    let key = unique_key("ttl-boundary");

    cache.set_bytes(&key, b"short".to_vec(), Some(1)).await;
    assert_eq!(cache.get_bytes(&key).await, Some(b"short".to_vec()));

    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert_eq!(cache.get_bytes(&key).await, None);
}
