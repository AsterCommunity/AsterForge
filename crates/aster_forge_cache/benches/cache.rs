//! Benchmarks for Forge cache primitives.

use std::sync::Arc;

use aster_forge_cache::bloom::{BloomConfig, BloomFilter};
use aster_forge_cache::{CacheBackend, MemoryCache};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn bench_bloom_check(c: &mut Criterion) {
    let filter = Arc::new(
        BloomFilter::new(BloomConfig::new(1_000, 0.001)).expect("valid Bloom configuration"),
    );
    let keys: Vec<String> = (0..1_000).map(|index| format!("key_{index}")).collect();
    filter.insert_many(keys.iter().map(String::as_str));

    c.bench_function("bloom/check_hit", |b| {
        b.iter(|| filter.contains("key_500"));
    });
    c.bench_function("bloom/check_miss", |b| {
        b.iter(|| filter.contains("nonexistent_key"));
    });
}

fn bench_bloom_insert(c: &mut Criterion) {
    let filter =
        BloomFilter::new(BloomConfig::new(100_000, 0.001)).expect("valid Bloom configuration");
    let counter = std::sync::atomic::AtomicU64::new(0);

    c.bench_function("bloom/insert", |b| {
        b.iter(|| {
            let index = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            filter.insert(&format!("key_{index}"));
        });
    });
}

fn bench_bloom_bulk_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("bloom/bulk_insert");
    for size in [100_u64, 500, 1_000] {
        let keys: Vec<String> = (0..size).map(|index| format!("bulk_key_{index}")).collect();
        group.throughput(Throughput::Elements(size));
        group.bench_with_input(BenchmarkId::new("keys", size), &keys, |b, keys| {
            b.iter_batched(
                || {
                    BloomFilter::new(BloomConfig::new(size as usize, 0.001))
                        .expect("valid Bloom configuration")
                },
                |filter| filter.insert_many(keys.iter().map(String::as_str)),
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_memory_get(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime");
    let cache = Arc::new(MemoryCache::new(3_600));
    runtime.block_on(async {
        for index in 0..1_000 {
            cache
                .set_bytes(&format!("key_{index}"), vec![0x5a; 256], None)
                .await;
        }
    });

    c.bench_function("memory/get_hit", |b| {
        b.to_async(&runtime).iter(|| {
            let cache = Arc::clone(&cache);
            async move { cache.get_bytes("key_500").await }
        });
    });
    c.bench_function("memory/get_miss", |b| {
        b.to_async(&runtime).iter(|| {
            let cache = Arc::clone(&cache);
            async move { cache.get_bytes("missing").await }
        });
    });
}

fn bench_memory_set_and_delete(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime");
    let cache = Arc::new(MemoryCache::new(3_600));
    let counter = std::sync::atomic::AtomicU64::new(0);

    c.bench_function("memory/set", |b| {
        b.to_async(&runtime).iter(|| {
            let cache = Arc::clone(&cache);
            let index = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async move {
                cache
                    .set_bytes(&format!("insert_{index}"), vec![0x5a; 256], None)
                    .await;
            }
        });
    });
    c.bench_function("memory/delete", |b| {
        b.to_async(&runtime).iter(|| {
            let cache = Arc::clone(&cache);
            let index = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async move {
                let key = format!("delete_{index}");
                cache.set_bytes(&key, vec![0x5a; 256], None).await;
                cache.delete(&key).await;
            }
        });
    });
}

criterion_group!(
    benches,
    bench_bloom_check,
    bench_bloom_insert,
    bench_bloom_bulk_insert,
    bench_memory_get,
    bench_memory_set_and_delete,
);
criterion_main!(benches);
