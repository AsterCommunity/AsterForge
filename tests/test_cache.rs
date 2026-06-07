//! Integration tests for cache backend contracts.

use aster_forge::cache::{CacheExt, create_cache};
use aster_forge::config::CacheConfig;

fn cache_config(enabled: bool, backend: &str, default_ttl: u64) -> CacheConfig {
    CacheConfig {
        enabled,
        backend: backend.to_string(),
        redis_url: String::new(),
        default_ttl,
    }
}

#[tokio::test]
async fn create_cache_disabled_uses_noop_backend_with_reservations() {
    let cache = create_cache(&cache_config(false, "memory", 60)).await;

    assert_eq!(cache.backend_name(), "noop");
    cache.health_check().await.unwrap();
    cache
        .set_bytes("ignored", b"value".to_vec(), Some(60))
        .await;
    assert_eq!(cache.get_bytes("ignored").await, None);
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
async fn memory_cache_round_trips_json_and_ignores_invalid_json() {
    let cache = create_cache(&cache_config(true, "memory", 60)).await;

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
async fn memory_cache_delete_and_invalidate_prefix_remove_entries_and_reservations() {
    let cache = create_cache(&cache_config(true, "unknown-backend", 60)).await;

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
async fn memory_cache_set_if_absent_is_atomic_for_concurrent_callers() {
    let cache = create_cache(&cache_config(true, "memory", 60)).await;
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
async fn memory_cache_zero_ttl_entries_expire_immediately() {
    let cache = create_cache(&cache_config(true, "memory", 60)).await;

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

#[tokio::test]
async fn redis_backend_with_invalid_url_falls_back_to_memory() {
    let cache = create_cache(&CacheConfig {
        enabled: true,
        backend: "redis".to_string(),
        redis_url: "not a redis url".to_string(),
        default_ttl: 60,
    })
    .await;

    assert_eq!(cache.backend_name(), "memory");
    cache
        .set_bytes("fallback", b"value".to_vec(), Some(60))
        .await;
    assert_eq!(cache.get_bytes("fallback").await, Some(b"value".to_vec()));
}
