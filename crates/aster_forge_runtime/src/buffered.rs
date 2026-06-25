//! Generic buffered batch writer for runtime side-effect queues.
//!
//! This module owns the product-neutral mechanics behind a common Aster pattern: accept records
//! quickly, flush when a batch threshold is reached, flush a partial batch after a short delay, and
//! fall back to a direct write when the in-memory queue is full. Products still own the record type,
//! persistence calls, error mapping, and any audit or metrics semantics.

use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex, MutexGuard as StdMutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use tokio::sync::{Mutex as AsyncMutex, MutexGuard};
use tokio_util::sync::CancellationToken;

type WriteFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type WriteBatch<T> = dyn Fn(Vec<T>) -> WriteFuture + Send + Sync;
type WriteOne<T> = dyn Fn(T) -> WriteFuture + Send + Sync;

/// Runtime buffering policy for [`BufferedBatchWriter`].
#[derive(Debug, Clone)]
pub struct BufferedBatchConfig {
    /// Maximum number of records kept in memory before overflow records use the direct writer.
    pub queue_capacity: usize,
    /// Number of queued records that triggers an immediate flush.
    pub batch_size: usize,
    /// Delay before flushing a partial batch.
    pub delayed_flush_after: Duration,
    /// Label used in overflow warning logs.
    pub overflow_label: &'static str,
}

impl BufferedBatchConfig {
    /// Creates a buffering policy.
    pub fn new(
        queue_capacity: usize,
        batch_size: usize,
        delayed_flush_after: Duration,
        overflow_label: &'static str,
    ) -> Self {
        Self {
            queue_capacity,
            batch_size,
            delayed_flush_after,
            overflow_label,
        }
    }
}

/// Generic asynchronous batch writer with threshold, delayed, and overflow flushing.
pub struct BufferedBatchWriter<T> {
    config: BufferedBatchConfig,
    buffer: Mutex<Vec<T>>,
    flush_lock: AsyncMutex<()>,
    flush_pending: AtomicBool,
    delayed_flush_pending: AtomicBool,
    shutdown_token: CancellationToken,
    write_batch: Arc<WriteBatch<T>>,
    write_one: Arc<WriteOne<T>>,
}

struct FlushPendingReset<T> {
    writer: Arc<BufferedBatchWriter<T>>,
    armed: bool,
}

impl<T> Drop for FlushPendingReset<T> {
    fn drop(&mut self) {
        if self.armed {
            self.writer.flush_pending.store(false, Ordering::Release);
        }
    }
}

impl<T> FlushPendingReset<T> {
    fn reset(&mut self) {
        self.writer.flush_pending.store(false, Ordering::Release);
        self.armed = false;
    }
}

struct DelayedFlushPendingReset<T> {
    writer: Arc<BufferedBatchWriter<T>>,
    armed: bool,
}

impl<T> Drop for DelayedFlushPendingReset<T> {
    fn drop(&mut self) {
        if self.armed {
            self.writer
                .delayed_flush_pending
                .store(false, Ordering::Release);
        }
    }
}

impl<T> DelayedFlushPendingReset<T> {
    fn reset(&mut self) {
        self.writer
            .delayed_flush_pending
            .store(false, Ordering::Release);
        self.armed = false;
    }
}

impl<T> BufferedBatchWriter<T>
where
    T: Send + 'static,
{
    /// Creates a writer from product-owned persistence callbacks.
    pub fn new<BatchFn, BatchFuture, OneFn, OneFuture>(
        config: BufferedBatchConfig,
        write_batch: BatchFn,
        write_one: OneFn,
    ) -> Self
    where
        BatchFn: Fn(Vec<T>) -> BatchFuture + Send + Sync + 'static,
        BatchFuture: Future<Output = ()> + Send + 'static,
        OneFn: Fn(T) -> OneFuture + Send + Sync + 'static,
        OneFuture: Future<Output = ()> + Send + 'static,
    {
        let batch_capacity = config.batch_size.max(1);
        Self {
            config,
            buffer: Mutex::new(Vec::with_capacity(batch_capacity)),
            flush_lock: AsyncMutex::new(()),
            flush_pending: AtomicBool::new(false),
            delayed_flush_pending: AtomicBool::new(false),
            shutdown_token: CancellationToken::new(),
            write_batch: Arc::new(move |items| Box::pin(write_batch(items))),
            write_one: Arc::new(move |item| Box::pin(write_one(item))),
        }
    }

    /// Records one item, scheduling a threshold or delayed flush as needed.
    pub async fn record(self: &Arc<Self>, item: T) {
        let mut overflow_item = None;
        let should_flush;
        let should_schedule_delayed_flush;
        {
            let mut buffer = self.lock_buffer();
            if buffer.len() >= self.config.queue_capacity {
                overflow_item = Some(item);
                should_flush = false;
                should_schedule_delayed_flush = false;
            } else {
                let was_empty = buffer.is_empty();
                buffer.push(item);
                should_flush = buffer.len() >= self.config.batch_size;
                should_schedule_delayed_flush = !should_flush && was_empty;
            }
        }

        if let Some(item) = overflow_item {
            tracing::warn!(
                capacity = self.config.queue_capacity,
                queue = self.config.overflow_label,
                "buffered writer queue is full; falling back to direct write"
            );
            self.schedule_flush();
            (self.write_one)(item).await;
            return;
        }

        if should_flush {
            self.schedule_flush();
        } else if should_schedule_delayed_flush {
            self.schedule_delayed_flush();
        }
    }

    /// Flushes the current buffer and schedules any remaining buffered items.
    pub async fn flush(self: &Arc<Self>) {
        let _guard = self.flush_lock.lock().await;
        self.flush_buffer().await;
        if self.lock_buffer().is_empty() {
            self.flush_pending.store(false, Ordering::Release);
            self.delayed_flush_pending.store(false, Ordering::Release);
        }
        self.schedule_buffered_flush();
    }

    /// Cancels delayed flush tasks. Call [`BufferedBatchWriter::flush`] afterwards during shutdown.
    pub fn cancel(&self) {
        self.shutdown_token.cancel();
    }

    /// Holds the flush lock for deterministic downstream tests.
    #[doc(hidden)]
    pub async fn lock_flush_for_test(&self) -> MutexGuard<'_, ()> {
        self.flush_lock.lock().await
    }

    fn schedule_flush(self: &Arc<Self>) {
        if self
            .flush_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let writer = Arc::clone(self);
        drop(tokio::spawn(async move {
            let mut pending_reset = FlushPendingReset {
                writer: Arc::clone(&writer),
                armed: true,
            };
            {
                let _guard = writer.flush_lock.lock().await;
                writer.flush_buffer().await;
            }
            pending_reset.reset();
            writer.schedule_buffered_flush();
        }));
    }

    fn schedule_delayed_flush(self: &Arc<Self>) {
        if self
            .delayed_flush_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let writer = Arc::clone(self);
        drop(tokio::spawn(async move {
            let mut pending_reset = DelayedFlushPendingReset {
                writer: Arc::clone(&writer),
                armed: true,
            };
            let delayed_flush_after = writer.config.delayed_flush_after;
            tokio::select! {
                biased;
                _ = writer.shutdown_token.cancelled() => return,
                _ = tokio::time::sleep(delayed_flush_after) => {}
            }

            {
                let _guard = writer.flush_lock.lock().await;
                writer.flush_buffer().await;
            }
            pending_reset.reset();
            writer.schedule_buffered_flush();
        }));
    }

    fn schedule_buffered_flush(self: &Arc<Self>) {
        let buffered_count = self.lock_buffer().len();
        if buffered_count >= self.config.batch_size {
            self.schedule_flush();
        } else if buffered_count > 0 {
            self.schedule_delayed_flush();
        }
    }

    async fn flush_buffer(&self) {
        let mut items = {
            let mut buffer = self.lock_buffer();
            if buffer.is_empty() {
                return;
            }
            std::mem::take(&mut *buffer)
        };

        let batch_size = self.config.batch_size.max(1);
        while !items.is_empty() {
            let chunk_len = items.len().min(batch_size);
            let chunk = items.drain(..chunk_len).collect::<Vec<_>>();
            (self.write_batch)(chunk).await;
        }
    }

    fn lock_buffer(&self) -> StdMutexGuard<'_, Vec<T>> {
        match self.buffer.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!(
                    queue = self.config.overflow_label,
                    "buffered writer mutex was poisoned; continuing with recovered buffer"
                );
                poisoned.into_inner()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::{BufferedBatchConfig, BufferedBatchWriter};

    fn config(delayed_flush_after: Duration) -> BufferedBatchConfig {
        BufferedBatchConfig::new(5, 3, delayed_flush_after, "test")
    }

    async fn wait_for_count(count: &AtomicUsize, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let current = count.load(Ordering::SeqCst);
            if current == expected {
                return;
            }
            assert!(
                current < expected,
                "count exceeded expected value: expected {expected}, got {current}"
            );
            assert!(
                Instant::now() < deadline,
                "timed out waiting for count {expected}; last count was {current}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn threshold_batch_flushes_immediately() {
        let count = Arc::new(AtomicUsize::new(0));
        let batch_count = Arc::clone(&count);
        let one_count = Arc::clone(&count);
        let writer = Arc::new(BufferedBatchWriter::new(
            config(Duration::from_secs(5)),
            move |items: Vec<usize>| {
                let batch_count = Arc::clone(&batch_count);
                async move {
                    batch_count.fetch_add(items.len(), Ordering::SeqCst);
                }
            },
            move |_item| {
                let one_count = Arc::clone(&one_count);
                async move {
                    one_count.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

        writer.record(1).await;
        writer.record(2).await;
        writer.record(3).await;

        wait_for_count(&count, 3).await;
        writer.cancel();
    }

    #[tokio::test]
    async fn partial_batch_flushes_after_delay() {
        let count = Arc::new(AtomicUsize::new(0));
        let batch_count = Arc::clone(&count);
        let one_count = Arc::clone(&count);
        let writer = Arc::new(BufferedBatchWriter::new(
            config(Duration::from_millis(20)),
            move |items: Vec<usize>| {
                let batch_count = Arc::clone(&batch_count);
                async move {
                    batch_count.fetch_add(items.len(), Ordering::SeqCst);
                }
            },
            move |_item| {
                let one_count = Arc::clone(&one_count);
                async move {
                    one_count.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

        writer.record(1).await;

        wait_for_count(&count, 1).await;
        writer.cancel();
    }

    #[tokio::test]
    async fn cancel_stops_delayed_flush_until_manual_flush() {
        let count = Arc::new(AtomicUsize::new(0));
        let batch_count = Arc::clone(&count);
        let one_count = Arc::clone(&count);
        let writer = Arc::new(BufferedBatchWriter::new(
            config(Duration::from_millis(20)),
            move |items: Vec<usize>| {
                let batch_count = Arc::clone(&batch_count);
                async move {
                    batch_count.fetch_add(items.len(), Ordering::SeqCst);
                }
            },
            move |_item| {
                let one_count = Arc::clone(&one_count);
                async move {
                    one_count.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

        writer.record(1).await;
        writer.cancel();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);

        writer.flush().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn overflow_writes_extra_item_directly_and_flushes_buffer() {
        let count = Arc::new(AtomicUsize::new(0));
        let batch_count = Arc::clone(&count);
        let one_count = Arc::clone(&count);
        let writer = Arc::new(BufferedBatchWriter::new(
            config(Duration::from_secs(5)),
            move |items: Vec<usize>| {
                let batch_count = Arc::clone(&batch_count);
                async move {
                    batch_count.fetch_add(items.len(), Ordering::SeqCst);
                }
            },
            move |_item| {
                let one_count = Arc::clone(&one_count);
                async move {
                    one_count.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));
        let flush_guard = writer.lock_flush_for_test().await;

        for index in 0..5 {
            writer.record(index).await;
        }
        writer.record(10_000).await;

        assert_eq!(count.load(Ordering::SeqCst), 1);
        drop(flush_guard);

        wait_for_count(&count, 6).await;
        writer.cancel();
    }
}
