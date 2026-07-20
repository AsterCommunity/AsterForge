//! Integration coverage for cache backend construction and runtime semantics.
//!
//! These tests exercise the public factory and trait-object API instead of private implementation
//! details. The Redis test uses a real container so fallback behavior and wire-level round trips
//! stay aligned with production dependencies.

use aster_forge_cache::{CacheConfig, CacheExt, create_cache};
#[cfg(feature = "redis")]
use aster_forge_test::{redis::RedisTestContainer, suite::TestContainerSuite};
#[cfg(feature = "redis")]
use std::sync::Arc;
#[cfg(feature = "redis")]
use tokio::time::{Duration, Instant, sleep};

fn cache_config(backend: &str, default_ttl: u64) -> CacheConfig {
    CacheConfig {
        backend: backend.to_string(),
        endpoint: String::new(),
        default_ttl,
    }
}

#[cfg(feature = "redis")]
fn test_suite() -> &'static TestContainerSuite {
    static SUITE: std::sync::OnceLock<TestContainerSuite> = std::sync::OnceLock::new();
    SUITE.get_or_init(|| TestContainerSuite::new("asterforge-cache"))
}

/// The shared container keeps data across runs, so keys must be unique per test process.
#[cfg(feature = "redis")]
fn unique_key(name: &str) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!(
        "asterforge-cache-test:{}:{}:{name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

#[cfg(feature = "redis")]
async fn wait_for_redis_cache(endpoint: String) -> Arc<dyn aster_forge_cache::CacheBackend> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let config = CacheConfig {
        backend: "redis".to_string(),
        endpoint,
        default_ttl: 60,
    };

    loop {
        let cache = create_cache(&config).await;
        if cache.backend_name() == "redis" {
            return cache;
        }
        assert!(
            Instant::now() < deadline,
            "Redis test container did not accept cache connections before timeout"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn test_create_cache_memory_backend_preserves_runtime_semantics() {
    let cache = create_cache(&cache_config("memory", 60)).await;

    assert_eq!(cache.backend_name(), "memory");
    cache.health_check().await.unwrap();
    cache.set_bytes("stored", b"value".to_vec(), Some(60)).await;
    assert_eq!(cache.get_bytes("stored").await, Some(b"value".to_vec()));
    assert_eq!(cache.take_bytes("stored").await, Some(b"value".to_vec()));
    assert_eq!(cache.take_bytes("stored").await, None);
    assert!(
        cache
            .set_bytes_if_absent("reservation", b"first".to_vec(), Some(60))
            .await
    );
    assert!(
        !cache
            .set_bytes_if_absent("reservation", b"second".to_vec(), Some(60))
            .await
    );

    cache.delete("reservation").await;
    assert!(
        cache
            .set_bytes_if_absent("reservation", b"third".to_vec(), Some(60))
            .await
    );
}

#[tokio::test]
async fn test_memory_cache_round_trips_json_and_ignores_invalid_json() {
    let cache = create_cache(&cache_config("memory", 60)).await;

    assert_eq!(cache.backend_name(), "memory");
    cache.set("json", &vec!["alpha", "beta"], Some(60)).await;
    let stored = cache.get::<Vec<String>>("json").await.unwrap();
    assert_eq!(stored, vec!["alpha".to_string(), "beta".to_string()]);

    cache
        .set_bytes("json", b"not-json".to_vec(), Some(60))
        .await;
    assert_eq!(cache.get::<Vec<String>>("json").await, None);
}

#[tokio::test]
async fn test_memory_cache_delete_and_invalidate_prefix_remove_entries_and_reservations() {
    let cache = create_cache(&cache_config("unknown-backend", 60)).await;

    cache.set_bytes("folder:1", b"one".to_vec(), Some(60)).await;
    cache.set_bytes("folder:2", b"two".to_vec(), Some(60)).await;
    cache
        .set_bytes("other:1", b"three".to_vec(), Some(60))
        .await;
    assert!(
        cache
            .set_bytes_if_absent("folder:reserved", b"reserved".to_vec(), Some(60))
            .await
    );

    cache.invalidate_prefix("folder:").await;

    assert_eq!(cache.get_bytes("folder:1").await, None);
    assert_eq!(cache.get_bytes("folder:2").await, None);
    assert_eq!(cache.get_bytes("other:1").await, Some(b"three".to_vec()));
    assert!(
        cache
            .set_bytes_if_absent("folder:reserved", b"new".to_vec(), Some(60))
            .await
    );

    cache.delete("other:1").await;
    assert_eq!(cache.get_bytes("other:1").await, None);
}

#[tokio::test]
async fn test_memory_cache_set_if_absent_is_atomic_for_concurrent_callers() {
    let cache = create_cache(&cache_config("memory", 60)).await;
    let mut tasks = Vec::new();

    for i in 0..24 {
        let cache = cache.clone();
        tasks.push(tokio::spawn(async move {
            cache
                .set_bytes_if_absent("nonce", format!("value-{i}").into_bytes(), Some(60))
                .await
        }));
    }

    let inserted = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|result| result.expect("cache reservation task should not panic"))
        .filter(|value| *value)
        .count();

    assert_eq!(inserted, 1);
    assert!(cache.get_bytes("nonce").await.is_some());
}

#[tokio::test]
async fn test_memory_cache_zero_ttl_entries_expire_immediately() {
    let cache = create_cache(&cache_config("memory", 60)).await;

    cache.set_bytes("expired", b"value".to_vec(), Some(0)).await;
    assert_eq!(cache.get_bytes("expired").await, None);

    assert!(
        cache
            .set_bytes_if_absent("zero-reservation", b"first".to_vec(), Some(0))
            .await
    );
    assert_eq!(cache.get_bytes("zero-reservation").await, None);
    assert!(
        cache
            .set_bytes_if_absent("zero-reservation", b"second".to_vec(), Some(0))
            .await
    );
}

#[cfg(feature = "redis")]
#[tokio::test]
async fn test_redis_backend_with_invalid_url_falls_back_to_memory() {
    let cache = create_cache(&CacheConfig {
        backend: "redis".to_string(),
        endpoint: "not a redis url".to_string(),
        default_ttl: 60,
    })
    .await;

    assert_eq!(cache.backend_name(), "memory");
    cache
        .set_bytes("fallback", b"value".to_vec(), Some(60))
        .await;
    assert_eq!(cache.get_bytes("fallback").await, Some(b"value".to_vec()));
}

#[cfg(feature = "redis")]
#[tokio::test]
async fn test_redis_cache_round_trips_against_real_redis_container() {
    let container = RedisTestContainer::start(test_suite()).await;
    let cache = wait_for_redis_cache(container.url().to_string()).await;

    assert_eq!(cache.backend_name(), "redis");
    cache.health_check().await.unwrap();

    let json_key = unique_key("json");
    let bytes_key = unique_key("bytes");
    let nonce_key = unique_key("nonce");
    let prefix_base = unique_key("folder");

    cache.set(&json_key, &vec!["alpha", "beta"], Some(60)).await;
    assert_eq!(
        cache.get::<Vec<String>>(&json_key).await.unwrap(),
        vec!["alpha".to_string(), "beta".to_string()]
    );

    cache
        .set_bytes(&bytes_key, b"value".to_vec(), Some(60))
        .await;
    assert_eq!(cache.get_bytes(&bytes_key).await, Some(b"value".to_vec()));

    assert!(
        cache
            .set_bytes_if_absent(&nonce_key, b"first".to_vec(), Some(60))
            .await
    );
    assert!(
        !cache
            .set_bytes_if_absent(&nonce_key, b"second".to_vec(), Some(60))
            .await
    );
    assert_eq!(cache.get_bytes(&nonce_key).await, Some(b"first".to_vec()));

    let folder_prefix = format!("{prefix_base}:folder:");
    let other_key = format!("{prefix_base}:other:1");
    cache
        .set_bytes(&format!("{folder_prefix}1"), b"one".to_vec(), Some(60))
        .await;
    cache
        .set_bytes(&format!("{folder_prefix}2"), b"two".to_vec(), Some(60))
        .await;
    cache
        .set_bytes(&other_key, b"three".to_vec(), Some(60))
        .await;
    cache.invalidate_prefix(&folder_prefix).await;
    assert_eq!(cache.get_bytes(&format!("{folder_prefix}1")).await, None);
    assert_eq!(cache.get_bytes(&format!("{folder_prefix}2")).await, None);
    assert_eq!(cache.get_bytes(&other_key).await, Some(b"three".to_vec()));

    cache.delete(&other_key).await;
    assert_eq!(cache.get_bytes(&other_key).await, None);
}
