//! Public-contract tests for ownership, durability, cancellation, and bounded admission.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use aster_forge_runtime::*;
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::time::{Instant, advance};

type Writer = ManagedBatchWriter<usize, &'static str>;
fn config(capacity: usize, batch: usize) -> BufferedBatchConfig {
    BufferedBatchConfig::new(capacity, batch, Duration::from_secs(10))
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(1)
}
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}
fn memory(
    config: BufferedBatchConfig,
    policy: OverflowPolicy,
) -> (Writer, Arc<Mutex<Vec<Vec<usize>>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let writes = records.clone();
    (
        Writer::new(config, policy, None, move |batch| {
            writes.lock().unwrap().push(batch);
            async { Ok(()) }
        }),
        records,
    )
}

#[test]
fn public_error_types_preserve_display_and_source_contracts() {
    use std::error::Error;
    let persistence = WriteFailure::new(WriteFailureClass::Permanent, std::io::Error::other("db"));
    assert!(persistence.to_string().contains("db"));
    assert!(persistence.source().is_some());
    for cause in [
        WriteFailureCause::Panicked,
        WriteFailureCause::Cancelled,
        WriteFailureCause::NotAttempted,
    ] {
        let failure = WriteFailure::<std::io::Error> {
            class: WriteFailureClass::UnknownCommit,
            cause,
        };
        assert!(failure.to_string().contains("write failed"));
        assert!(failure.source().is_none());
    }
    assert!(
        RecordError::<u8, std::io::Error>::NotRunning(1)
            .to_string()
            .contains("closed")
    );
    assert!(
        RecordError::<u8, std::io::Error>::Full(1)
            .to_string()
            .contains("capacity")
    );
    let direct: RecordError<u8, std::io::Error> = RecordError::DirectFailed(WriteFailure::new(
        WriteFailureClass::UnknownCommit,
        std::io::Error::other("db"),
    ));
    assert!(direct.to_string().contains("unconfirmed"));
    assert!(direct.source().is_some());
    assert_eq!(
        BufferFlushError::<std::io::Error>::NotStarted.to_string(),
        "writer has not started"
    );
    assert_eq!(
        BufferFlushError::<std::io::Error>::Stopped.to_string(),
        "writer stopped with unconfirmed records"
    );
    assert_eq!(
        BufferFlushError::<std::io::Error>::Deadline.to_string(),
        "flush deadline elapsed; records remain unconfirmed"
    );
    assert!(
        BufferFlushError::Write(WriteFailure::new(
            WriteFailureClass::Permanent,
            std::io::Error::other("db"),
        ))
        .source()
        .is_some()
    );
}

#[test]
fn construction_and_drop_work_without_a_tokio_runtime() {
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let writer = Writer::new(config(2, 1), OverflowPolicy::Reject, None, move |_| {
        called.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    assert_eq!(writer.snapshot().state, BufferedWriterState::Created);
    drop(writer);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn startup_admission_and_repeated_stop_boundaries() {
    let (writer, _) = memory(config(2, 1), OverflowPolicy::Reject);
    assert!(matches!(
        writer.record(7).await,
        Err(RecordError::NotRunning(7))
    ));
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::NotStarted)
    ));
    writer.start().await.unwrap();
    assert_eq!(writer.start().await, Err(BufferedWriterState::Running));
    writer.begin_drain();
    assert!(matches!(
        writer.record(8).await,
        Err(RecordError::NotRunning(8))
    ));
    assert_eq!(writer.start().await, Err(BufferedWriterState::Draining));
    for _ in 0..2 {
        let report = writer.stop(deadline()).await;
        assert_eq!(report.snapshot.state, BufferedWriterState::Stopped);
        assert_eq!(report.snapshot.persisted, 0);
    }
    assert_eq!(writer.start().await, Err(BufferedWriterState::Stopped));
    let (never_started, _) = memory(config(1, 1), OverflowPolicy::Reject);
    assert_eq!(
        never_started.stop(deadline()).await.snapshot.state,
        BufferedWriterState::Stopped
    );
    assert_eq!(
        never_started.start().await,
        Err(BufferedWriterState::Stopped)
    );
}

#[tokio::test(start_paused = true)]
async fn threshold_chunking_delay_and_manual_flush_reset() {
    let (writer, batches) = memory(config(8, 3), OverflowPolicy::Reject);
    writer.start().await.unwrap();
    for i in 0..7 {
        assert_eq!(writer.record(i).await.unwrap(), RecordAdmission::Queued);
    }
    settle().await;
    assert_eq!(*batches.lock().unwrap(), vec![vec![0, 1, 2], vec![3, 4, 5]]);
    advance(Duration::from_secs(9)).await;
    settle().await;
    assert_eq!(writer.snapshot().queued, 1);
    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(writer.snapshot().persisted, 7);
    writer.record(7).await.unwrap();
    writer.flush(deadline()).await.unwrap();
    advance(Duration::from_secs(9)).await;
    writer.record(8).await.unwrap();
    settle().await;
    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(
        writer.snapshot().queued,
        1,
        "previous partial-batch timer must not flush new records"
    );
    advance(Duration::from_secs(9)).await;
    settle().await;
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 9);
}

#[tokio::test(start_paused = true)]
async fn zero_bounds_and_zero_delay_normalize_without_hanging() {
    let (writer, batches) = memory(
        BufferedBatchConfig::new(0, 0, Duration::ZERO),
        OverflowPolicy::Reject,
    );
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    assert!(matches!(writer.record(2).await, Err(RecordError::Full(2))));
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1);
    assert_eq!(*batches.lock().unwrap(), vec![vec![1]]);
}

#[tokio::test(start_paused = true)]
async fn direct_overflow_acknowledges_only_the_direct_record() {
    let (writer, batches) = memory(config(1, 10), OverflowPolicy::DirectWrite);
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    assert_eq!(
        writer.record(2).await.unwrap(),
        RecordAdmission::DirectPersisted
    );
    assert_eq!(*batches.lock().unwrap(), vec![vec![2]]);
    let snapshot = writer.snapshot();
    assert_eq!(
        (
            snapshot.queued,
            snapshot.direct_pending,
            snapshot.persisted,
            snapshot.direct_persisted
        ),
        (1, 0, 1, 1)
    );
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 2);
}

#[tokio::test(start_paused = true)]
async fn failures_retain_original_records_and_only_safe_retry_resumes() {
    for class in [
        WriteFailureClass::Retryable,
        WriteFailureClass::Permanent,
        WriteFailureClass::UnknownCommit,
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let writer = Writer::new(config(4, 2), OverflowPolicy::Reject, None, move |_| {
            let attempt = called.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt == 0 {
                    Err(WriteFailure::new(class, "native-error"))
                } else {
                    Ok(())
                }
            }
        });
        writer.start().await.unwrap();
        writer.record(10).await.unwrap();
        writer.record(11).await.unwrap();
        let Err(BufferFlushError::Write(failure)) = writer.flush(deadline()).await else {
            panic!("expected write failure")
        };
        assert_eq!(failure.class, class);
        let WriteFailureCause::Persistence(source) = failure.cause else {
            panic!("lost original error")
        };
        assert_eq!(*source, "native-error");
        assert_eq!(writer.unconfirmed(), [10, 11]);
        assert_eq!(writer.snapshot().persisted, 0);
        advance(Duration::from_secs(100)).await;
        settle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no failure is retried implicitly"
        );
        writer.retry_failed();
        if class == WriteFailureClass::Retryable {
            writer.flush(deadline()).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(writer.snapshot().persisted, 2);
        } else {
            assert!(matches!(
                writer.flush(deadline()).await,
                Err(BufferFlushError::Write(_))
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        let report = writer.stop(deadline()).await;
        assert_eq!(
            report.snapshot.queued,
            if class == WriteFailureClass::Retryable {
                0
            } else {
                2
            }
        );
    }
}

#[tokio::test(start_paused = true)]
async fn successful_chunks_are_not_replayed_after_later_chunk_failure() {
    let (send, mut events) = mpsc::channel(10);
    let writer = Writer::new(
        config(5, 2),
        OverflowPolicy::Reject,
        Some(send),
        |items| async move {
            if items.contains(&2) {
                Err(WriteFailure::new(WriteFailureClass::Permanent, "bad chunk"))
            } else {
                Ok(())
            }
        },
    );
    writer.start().await.unwrap();
    for i in 0..5 {
        writer.record(i).await.unwrap();
    }
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    assert_eq!(writer.snapshot().persisted, 2);
    assert_eq!(writer.unconfirmed(), [2, 3, 4]);
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|e| e == BufferedWriterEvent::BatchPersisted)
    );
    writer.stop(deadline()).await;
}

#[tokio::test(start_paused = true)]
async fn direct_failure_retains_bounded_slot_and_can_be_explicitly_retried() {
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let writer = Writer::new(config(1, 9), OverflowPolicy::DirectWrite, None, move |_| {
        let attempt = called.fetch_add(1, Ordering::SeqCst);
        async move {
            if attempt == 0 {
                Err(WriteFailure::new(WriteFailureClass::Retryable, "offline"))
            } else {
                Ok(())
            }
        }
    });
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    assert!(matches!(
        writer.record(2).await,
        Err(RecordError::DirectFailed(_))
    ));
    assert_eq!(writer.unconfirmed(), [1, 2]);
    assert!(matches!(writer.record(3).await, Err(RecordError::Full(3))));
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    writer.retry_failed();
    writer.flush(deadline()).await.unwrap();
    assert_eq!(writer.snapshot().direct_persisted, 1);
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 2);
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn drain_timeout_cancels_and_joins_inflight_before_dependency_close() {
    let active = Arc::new(AtomicUsize::new(0));
    let inside = active.clone();
    let writer = Writer::new(config(1, 1), OverflowPolicy::DirectWrite, None, move |_| {
        let active = inside.clone();
        async move {
            active.fetch_add(1, Ordering::SeqCst);
            let _guard = Active(active);
            std::future::pending::<Result<(), WriteFailure<&'static str>>>().await
        }
    });
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    settle().await;
    assert_eq!(active.load(Ordering::SeqCst), 1);
    let direct = writer.record(2);
    tokio::pin!(direct);
    assert!(futures::poll!(&mut direct).is_pending());
    assert!(matches!(writer.record(3).await, Err(RecordError::Full(3))));
    let report = writer.stop(deadline()).await;
    assert!(report.deadline_elapsed);
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "database may close only after sink future dropped"
    );
    assert_eq!(
        (report.snapshot.queued, report.snapshot.direct_pending),
        (1, 1)
    );
    assert_eq!(
        report.snapshot.batch_failure.unwrap().class,
        WriteFailureClass::UnknownCommit
    );
    assert!(matches!(direct.await, Err(RecordError::DirectFailed(_))));
    assert_eq!(writer.unconfirmed(), [1, 2]);
    writer.retry_failed();
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 0);
}

#[tokio::test(start_paused = true)]
async fn cancelled_flush_and_stop_callers_leave_worker_owned_for_later_stop() {
    let gate = Arc::new(Semaphore::new(0));
    let persist = gate.clone();
    let writer = Arc::new(Writer::new(
        config(2, 1),
        OverflowPolicy::Reject,
        None,
        move |_| {
            let gate = persist.clone();
            async move {
                gate.acquire().await.unwrap().forget();
                Ok(())
            }
        },
    ));
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    {
        let flush = writer.flush(deadline());
        tokio::pin!(flush);
        assert!(futures::poll!(&mut flush).is_pending());
    } // Dropping the flush future must leave the write owned by the worker.
    let stopping = writer.clone();
    let task = tokio::spawn(async move {
        stopping
            .stop(Instant::now() + Duration::from_secs(100))
            .await
    });
    settle().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    gate.add_permits(1);
    let (first, second) = tokio::join!(writer.stop(deadline()), writer.stop(deadline()));
    assert_eq!(
        (first.snapshot.persisted, second.snapshot.persisted),
        (1, 1)
    );
    assert_eq!(
        writer.stop(deadline()).await.snapshot.state,
        BufferedWriterState::Stopped
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_direct_waiter_does_not_drop_accepted_overflow() {
    let gate = Arc::new(Semaphore::new(0));
    let persist = gate.clone();
    let writer = Arc::new(Writer::new(
        config(1, 9),
        OverflowPolicy::DirectWrite,
        None,
        move |_| {
            let gate = persist.clone();
            async move {
                gate.acquire().await.unwrap().forget();
                Ok(())
            }
        },
    ));
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let recording = writer.clone();
    let task = tokio::spawn(async move { recording.record(2).await });
    settle().await;
    assert_eq!(writer.snapshot().direct_pending, 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    gate.add_permits(2);
    let report = writer.stop(deadline()).await;
    assert_eq!(
        (report.snapshot.persisted, report.snapshot.direct_persisted),
        (2, 1)
    );
}

#[tokio::test(start_paused = true)]
async fn sink_panics_are_isolated_at_future_construction_and_polling() {
    for synchronous in [true, false] {
        let writer = Writer::new(config(2, 1), OverflowPolicy::Reject, None, move |_| {
            assert!(!synchronous, "sink construction panic");
            async { panic!("sink polling panic") }
        });
        writer.start().await.unwrap();
        writer.record(1).await.unwrap();
        assert!(matches!(
            writer.flush(deadline()).await,
            Err(BufferFlushError::Write(_))
        ));
        let failure = writer
            .stop(deadline())
            .await
            .snapshot
            .batch_failure
            .unwrap();
        assert_eq!(failure.class, WriteFailureClass::UnknownCommit);
        assert!(matches!(failure.cause, WriteFailureCause::Panicked));
        assert_eq!(writer.unconfirmed(), [1]);
    }
}

#[tokio::test(start_paused = true)]
async fn slow_or_panicking_observers_cannot_block_persistence() {
    let (events, mut receiver) = mpsc::channel(1);
    let writer = Writer::new(
        config(4, 1),
        OverflowPolicy::Reject,
        Some(events),
        |_| async { Ok(()) },
    );
    writer.start().await.unwrap();
    for i in 0..4 {
        writer.record(i).await.unwrap();
    }
    writer.flush(deadline()).await.unwrap();
    assert!(writer.snapshot().observer_events_dropped > 0);
    let observer = tokio::spawn(async move {
        receiver.recv().await;
        panic!("observer failure")
    });
    assert!(observer.await.unwrap_err().is_panic());
    writer.record(5).await.unwrap();
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 5);
}

#[tokio::test(start_paused = true)]
async fn instances_have_independent_queues_counters_and_shutdown() {
    let (one, first) = memory(config(2, 2), OverflowPolicy::Reject);
    let (two, second) = memory(config(2, 2), OverflowPolicy::Reject);
    one.start().await.unwrap();
    two.start().await.unwrap();
    one.record(1).await.unwrap();
    two.record(2).await.unwrap();
    one.stop(deadline()).await;
    assert_eq!(two.snapshot().state, BufferedWriterState::Running);
    assert_eq!(two.snapshot().persisted, 0);
    two.record(3).await.unwrap();
    two.stop(deadline()).await;
    assert_eq!(*first.lock().unwrap(), [vec![1]]);
    assert_eq!(*second.lock().unwrap(), [vec![2, 3]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_producers_drain_and_stop_account_for_every_accepted_record() {
    for policy in [OverflowPolicy::Reject, OverflowPolicy::DirectWrite] {
        // Keep the direct-policy batch below threshold so concurrent overflow must actually
        // exercise the direct lane, rather than depending on how fast the batch worker runs.
        let capacity = if matches!(policy, OverflowPolicy::DirectWrite) {
            1
        } else {
            32
        };
        let (writer, batches) = memory(config(capacity, 5), policy);
        let writer = Arc::new(writer);
        writer.start().await.unwrap();
        writer.record(usize::MAX).await.unwrap();
        let accepted = Arc::new(Mutex::new(vec![usize::MAX]));
        let start = Arc::new(tokio::sync::Barrier::new(9));
        let mut producers = Vec::new();
        for producer in 0..8 {
            let (writer, accepted, start) = (writer.clone(), accepted.clone(), start.clone());
            producers.push(tokio::spawn(async move {
                start.wait().await;
                for i in 0..100 {
                    let item = producer * 100 + i;
                    match writer.record(item).await {
                        Ok(RecordAdmission::Queued | RecordAdmission::DirectPersisted) => {
                            accepted.lock().unwrap().push(item);
                        }
                        Err(RecordError::Full(returned) | RecordError::NotRunning(returned)) => {
                            assert_eq!(returned, item);
                        }
                        result => panic!("unexpected result: {result:?}"),
                    }
                    tokio::task::yield_now().await;
                }
            }));
        }
        start.wait().await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        if matches!(policy, OverflowPolicy::DirectWrite) {
            tokio::time::timeout(Duration::from_secs(1), async {
                while writer.snapshot().direct_persisted == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("concurrent accounting must exercise acknowledged direct writes");
        }
        let (one, two) = tokio::join!(writer.stop(deadline()), writer.stop(deadline()));
        for producer in producers {
            producer.await.unwrap();
        }
        assert_eq!(one.snapshot.persisted, two.snapshot.persisted);
        let mut durable: Vec<_> = batches.lock().unwrap().iter().flatten().copied().collect();
        durable.sort_unstable();
        let mut expected = accepted.lock().unwrap().clone();
        expected.sort_unstable();
        assert_eq!(durable, expected);
        assert_eq!(durable.len() as u64, one.snapshot.persisted);
        assert!(writer.unconfirmed().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn drop_cancels_owner_task_without_a_detached_timer_or_write() {
    let active = Arc::new(AtomicUsize::new(0));
    let inside = active.clone();
    let entered = Arc::new(Notify::new());
    let notify = entered.clone();
    let writer = Writer::new(config(1, 1), OverflowPolicy::Reject, None, move |_| {
        let (active, notify) = (inside.clone(), notify.clone());
        async move {
            active.fetch_add(1, Ordering::SeqCst);
            let _guard = Active(active);
            notify.notify_one();
            std::future::pending().await
        }
    });
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    entered.notified().await;
    drop(writer);
    settle().await;
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn batch_failure_still_drains_accepted_direct_record() {
    let gate = Arc::new(Semaphore::new(0));
    let entered = Arc::new(Notify::new());
    let (persist, notify) = (gate.clone(), entered.clone());
    let writer = Writer::new(
        config(1, 1),
        OverflowPolicy::DirectWrite,
        None,
        move |items| {
            let (gate, entered) = (persist.clone(), notify.clone());
            async move {
                if items == [1] {
                    entered.notify_one();
                    gate.acquire().await.unwrap().forget();
                    Err(WriteFailure::new(
                        WriteFailureClass::Permanent,
                        "batch rejected",
                    ))
                } else {
                    Ok(())
                }
            }
        },
    );
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    entered.notified().await;
    let direct = writer.record(2);
    tokio::pin!(direct);
    assert!(futures::poll!(&mut direct).is_pending());
    gate.add_permits(1);
    let report = writer.stop(deadline()).await;
    assert_eq!(direct.await.unwrap(), RecordAdmission::DirectPersisted);
    assert_eq!(
        (
            report.snapshot.persisted,
            report.snapshot.queued,
            report.snapshot.direct_pending
        ),
        (1, 1, 0)
    );
    assert_eq!(report.snapshot.batch_failure_records, 1);
}

#[tokio::test(start_paused = true)]
async fn cancelled_inflight_direct_write_is_unknown_and_never_replayed() {
    let entered = Arc::new(Notify::new());
    let notify = entered.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let writer = Writer::new(config(1, 9), OverflowPolicy::DirectWrite, None, move |_| {
        let (notify, calls) = (notify.clone(), called.clone());
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            notify.notify_one();
            std::future::pending().await
        }
    });
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let direct = writer.record(2);
    tokio::pin!(direct);
    assert!(futures::poll!(&mut direct).is_pending());
    entered.notified().await;
    let report = writer.stop(deadline()).await;
    assert!(report.deadline_elapsed);
    assert_eq!(
        report.snapshot.direct_failure.unwrap().class,
        WriteFailureClass::UnknownCommit
    );
    assert!(matches!(direct.await, Err(RecordError::DirectFailed(_))));
    writer.retry_failed();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(writer.unconfirmed(), [1, 2]);
}

#[tokio::test(start_paused = true)]
async fn concurrent_start_allows_exactly_one_owner_worker() {
    let (writer, _) = memory(config(1, 1), OverflowPolicy::Reject);
    let (one, two) = tokio::join!(writer.start(), writer.start());
    assert_ne!(one.is_ok(), two.is_ok());
    assert!(one == Err(BufferedWriterState::Running) || two == Err(BufferedWriterState::Running));
    writer.record(1).await.unwrap();
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_partial_batch_owner_cancels_timer_without_invoking_sink() {
    let (writer, batches) = memory(config(2, 2), OverflowPolicy::Reject);
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    settle().await;
    drop(writer);
    advance(Duration::from_secs(20)).await;
    settle().await;
    assert!(batches.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn unknown_commit_that_reached_storage_is_not_duplicated_by_flush_retry_or_stop() {
    let durable = Arc::new(Mutex::new(Vec::new()));
    let storage = durable.clone();
    let writer = Writer::new(config(2, 2), OverflowPolicy::Reject, None, move |items| {
        storage.lock().unwrap().extend(items);
        async {
            Err(WriteFailure::new(
                WriteFailureClass::UnknownCommit,
                "commit response lost",
            ))
        }
    });
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    writer.record(2).await.unwrap();
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    writer.retry_failed();
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    let report = writer.stop(deadline()).await;
    assert_eq!(
        report.snapshot.persisted, 0,
        "writer never received a durable acknowledgment"
    );
    assert_eq!(
        *durable.lock().unwrap(),
        [1, 2],
        "unknown result must never be automatically replayed"
    );
    assert_eq!(writer.unconfirmed(), [1, 2]);
}

#[tokio::test(start_paused = true)]
async fn unrepresentable_delay_still_allows_explicit_drain() {
    let (writer, _) = memory(
        BufferedBatchConfig::new(2, 2, Duration::MAX),
        OverflowPolicy::Reject,
    );
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    settle().await;
    assert_eq!(writer.snapshot().persisted, 0);
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1);
}

#[derive(Debug)]
struct PanicOnClone {
    id: usize,
    should_panic: Arc<std::sync::atomic::AtomicBool>,
}
impl Clone for PanicOnClone {
    fn clone(&self) -> Self {
        assert!(
            !self.should_panic.swap(false, Ordering::SeqCst),
            "record clone panicked"
        );
        Self {
            id: self.id,
            should_panic: self.should_panic.clone(),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn worker_panic_outside_sink_closes_admission_and_wakes_batch_flush() {
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let called = sink_calls.clone();
    let writer = ManagedBatchWriter::<PanicOnClone, &'static str>::new(
        config(2, 1),
        OverflowPolicy::Reject,
        None,
        move |_| {
            called.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        },
    );
    writer.start().await.unwrap();
    writer
        .record(PanicOnClone {
            id: 1,
            should_panic: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        })
        .await
        .unwrap();
    let before = Instant::now();
    let Err(BufferFlushError::Write(error)) = writer.flush(deadline()).await else {
        panic!("worker failure must reach the flush waiter immediately")
    };
    assert_eq!(
        Instant::now(),
        before,
        "worker panic must not wait for the caller deadline"
    );
    assert!(matches!(error.cause, WriteFailureCause::Panicked));
    assert_eq!(error.class, WriteFailureClass::UnknownCommit);
    assert_eq!(writer.snapshot().state, BufferedWriterState::Stopped);
    assert_eq!(sink_calls.load(Ordering::SeqCst), 0);
    let report = writer.stop(deadline()).await;
    assert_eq!(
        (
            report.snapshot.queued,
            report.snapshot.batch_failure_records,
            report.snapshot.persisted
        ),
        (1, 1, 0)
    );
    assert_eq!(writer.unconfirmed()[0].id, 1);
}

#[tokio::test(start_paused = true)]
async fn worker_panic_outside_sink_wakes_accepted_direct_waiter_and_retains_both_lanes() {
    let writer = ManagedBatchWriter::<PanicOnClone, &'static str>::new(
        config(1, 10),
        OverflowPolicy::DirectWrite,
        None,
        |_| async { Ok(()) },
    );
    writer.start().await.unwrap();
    writer
        .record(PanicOnClone {
            id: 1,
            should_panic: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
        .await
        .unwrap();
    let before = Instant::now();
    let direct = writer.record(PanicOnClone {
        id: 2,
        should_panic: Arc::new(std::sync::atomic::AtomicBool::new(true)),
    });
    let Err(RecordError::DirectFailed(error)) =
        tokio::time::timeout_at(deadline(), direct).await.unwrap()
    else {
        panic!("accepted direct waiter must see the worker failure")
    };
    assert_eq!(Instant::now(), before);
    assert!(matches!(error.cause, WriteFailureCause::Panicked));
    let report = writer.stop(deadline()).await;
    assert_eq!(
        (
            report.snapshot.queued,
            report.snapshot.direct_pending,
            report.snapshot.persisted
        ),
        (1, 1, 0)
    );
    assert_eq!(
        writer
            .unconfirmed()
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[tokio::test(start_paused = true)]
async fn ready_sinks_yield_to_other_runtime_tasks_before_draining_the_whole_queue() {
    let entered = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (notify, called) = (entered.clone(), calls.clone());
    let writer = Writer::new(config(1024, 1), OverflowPolicy::Reject, None, move |_| {
        if called.fetch_add(1, Ordering::SeqCst) == 0 {
            notify.notify_one();
        }
        async { Ok(()) }
    });
    writer.start().await.unwrap();
    for item in 0..1024 {
        writer.record(item).await.unwrap();
    }
    entered.notified().await;
    assert!(
        calls.load(Ordering::SeqCst) < 1024,
        "a ready sink must not monopolize the executor until every queued record is drained"
    );
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1024);
}

#[tokio::test(start_paused = true)]
async fn flush_does_not_wait_for_records_admitted_after_its_first_poll() {
    let gate = Arc::new(Semaphore::new(0));
    let block = gate.clone();
    let writer = Writer::new(config(2, 1), OverflowPolicy::Reject, None, move |items| {
        let gate = block.clone();
        async move {
            if items == [2] {
                gate.acquire().await.unwrap().forget();
            }
            Ok(())
        }
    });
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let flush = writer.flush(deadline());
    tokio::pin!(flush);
    assert!(futures::poll!(&mut flush).is_pending());
    writer.record(2).await.unwrap();
    let later_flush = writer.flush(deadline());
    tokio::pin!(later_flush);
    assert!(futures::poll!(&mut later_flush).is_pending());
    settle().await;
    assert_eq!(
        (writer.snapshot().persisted, writer.snapshot().queued),
        (1, 1)
    );
    assert!(
        flush.await.is_ok(),
        "later records must not extend an existing flush indefinitely"
    );
    assert!(futures::poll!(&mut later_flush).is_pending());
    gate.add_permits(1);
    later_flush.await.unwrap();
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 2);
}

#[tokio::test(start_paused = true)]
async fn later_batch_failure_does_not_poison_an_already_acknowledged_flush_scope() {
    let writer = Writer::new(
        config(2, 1),
        OverflowPolicy::Reject,
        None,
        |items| async move {
            if items == [2] {
                Err(WriteFailure::new(
                    WriteFailureClass::UnknownCommit,
                    "later commit response lost",
                ))
            } else {
                Ok(())
            }
        },
    );
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let flush = writer.flush(deadline());
    tokio::pin!(flush);
    assert!(futures::poll!(&mut flush).is_pending());
    writer.record(2).await.unwrap();
    settle().await;
    assert_eq!(writer.snapshot().persisted, 1);
    assert!(
        flush.await.is_ok(),
        "failure outside the captured flush scope cannot undo a durable acknowledgment"
    );
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    assert_eq!(writer.unconfirmed(), [2]);
    writer.stop(deadline()).await;
}

#[tokio::test(start_paused = true)]
async fn expired_flush_does_not_schedule_a_new_write_without_budget() {
    let (writer, batches) = memory(config(2, 2), OverflowPolicy::Reject);
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    assert!(matches!(
        writer.flush(Instant::now()).await,
        Err(BufferFlushError::Deadline)
    ));
    settle().await;
    assert!(
        batches.lock().unwrap().is_empty(),
        "expired flush must not force a fresh database attempt"
    );
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1);
}

#[tokio::test(start_paused = true)]
async fn forced_partial_flush_does_not_pull_later_records_into_its_transaction() {
    let (writer, batches) = memory(config(4, 3), OverflowPolicy::Reject);
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let flush = writer.flush(deadline());
    tokio::pin!(flush);
    assert!(futures::poll!(&mut flush).is_pending());
    writer.record(2).await.unwrap();
    settle().await;
    flush.await.unwrap();
    assert_eq!(*batches.lock().unwrap(), [vec![1]]);
    assert_eq!(writer.unconfirmed(), [2]);
    advance(Duration::from_secs(9)).await;
    settle().await;
    assert_eq!(writer.snapshot().persisted, 1);
    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(*batches.lock().unwrap(), [vec![1], vec![2]]);
    writer.stop(deadline()).await;
}

#[tokio::test(start_paused = true)]
async fn later_direct_admission_and_failure_do_not_extend_the_original_flush_scope() {
    for later_fails in [false, true] {
        let first = Arc::new(Semaphore::new(0));
        let second = Arc::new(Semaphore::new(0));
        let entered = Arc::new(Notify::new());
        let (one, two, notify) = (first.clone(), second.clone(), entered.clone());
        let writer = Writer::new(
            config(1, 10),
            OverflowPolicy::DirectWrite,
            None,
            move |items| {
                let (first, second, entered) = (one.clone(), two.clone(), notify.clone());
                async move {
                    if items == [1] {
                        entered.notify_one();
                        first.acquire().await.unwrap().forget();
                    } else if later_fails {
                        return Err(WriteFailure::new(
                            WriteFailureClass::UnknownCommit,
                            "later direct commit unknown",
                        ));
                    } else {
                        second.acquire().await.unwrap().forget();
                    }
                    Ok(())
                }
            },
        );
        writer.start().await.unwrap();
        writer.record(1).await.unwrap();
        let flush = writer.flush(deadline());
        tokio::pin!(flush);
        assert!(futures::poll!(&mut flush).is_pending());
        entered.notified().await;
        let direct = writer.record(2);
        tokio::pin!(direct);
        assert!(futures::poll!(&mut direct).is_pending());
        first.add_permits(1);
        settle().await;
        flush.await.unwrap();
        assert_eq!(writer.snapshot().persisted, 1);
        if later_fails {
            assert!(matches!(direct.await, Err(RecordError::DirectFailed(_))));
            assert!(matches!(
                writer.flush(deadline()).await,
                Err(BufferFlushError::Write(_))
            ));
            assert_eq!(writer.unconfirmed(), [2]);
        } else {
            let later_flush = writer.flush(deadline());
            tokio::pin!(later_flush);
            assert!(futures::poll!(&mut later_flush).is_pending());
            second.add_permits(1);
            assert_eq!(direct.await.unwrap(), RecordAdmission::DirectPersisted);
            later_flush.await.unwrap();
        }
        let report = writer.stop(deadline()).await;
        assert_eq!(report.snapshot.direct_pending, usize::from(later_fails));
        assert_eq!(report.snapshot.direct_persisted, u64::from(!later_fails));
    }
}

#[tokio::test(start_paused = true)]
async fn direct_acknowledgment_cannot_satisfy_a_captured_batch_watermark() {
    let gate = Arc::new(Semaphore::new(0));
    let block = gate.clone();
    let writer = Writer::new(
        config(1, 10),
        OverflowPolicy::DirectWrite,
        None,
        move |items| {
            let gate = block.clone();
            async move {
                if items == [1] {
                    gate.acquire().await.unwrap().forget();
                }
                Ok(())
            }
        },
    );
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let direct = writer.record(2);
    tokio::pin!(direct);
    assert!(futures::poll!(&mut direct).is_pending());
    let flush = writer.flush(deadline());
    tokio::pin!(flush);
    assert!(futures::poll!(&mut flush).is_pending());
    assert_eq!(direct.await.unwrap(), RecordAdmission::DirectPersisted);
    assert_eq!(writer.snapshot().direct_persisted, 1);
    assert!(
        futures::poll!(&mut flush).is_pending(),
        "direct persistence cannot acknowledge the queued record"
    );
    gate.add_permits(1);
    flush.await.unwrap();
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 2);
}

#[tokio::test(start_paused = true)]
async fn empty_expired_flush_succeeds_without_forcing_a_future_admission() {
    let (writer, batches) = memory(config(2, 2), OverflowPolicy::Reject);
    writer.start().await.unwrap();
    writer.flush(Instant::now()).await.unwrap();
    writer.record(1).await.unwrap();
    settle().await;
    assert!(batches.lock().unwrap().is_empty());
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1);
}

#[tokio::test(start_paused = true)]
async fn flush_scope_is_captured_when_polled_instead_of_when_constructed() {
    let (writer, batches) = memory(config(4, 4), OverflowPolicy::Reject);
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let flush = writer.flush(deadline());
    writer.record(2).await.unwrap();
    flush.await.unwrap();
    assert_eq!(*batches.lock().unwrap(), [vec![1, 2]]);
    assert_eq!(writer.snapshot().persisted, 2);
    writer.stop(deadline()).await;
}

#[tokio::test(start_paused = true)]
async fn concurrent_flush_timeout_does_not_cancel_another_waiters_owned_write() {
    let gate = Arc::new(Semaphore::new(0));
    let block = gate.clone();
    let writer = Writer::new(config(2, 2), OverflowPolicy::Reject, None, move |_| {
        let gate = block.clone();
        async move {
            gate.acquire().await.unwrap().forget();
            Ok(())
        }
    });
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    let short = writer.flush(Instant::now() + Duration::from_millis(10));
    let long = writer.flush(deadline());
    tokio::pin!(short, long);
    assert!(futures::poll!(&mut short).is_pending());
    assert!(futures::poll!(&mut long).is_pending());
    advance(Duration::from_millis(10)).await;
    assert!(matches!(short.await, Err(BufferFlushError::Deadline)));
    assert!(futures::poll!(&mut long).is_pending());
    assert_eq!(writer.unconfirmed(), [1]);
    gate.add_permits(1);
    long.await.unwrap();
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1);
}

#[tokio::test(start_paused = true)]
async fn a_flush_started_before_stop_reports_stopped_after_worker_join() {
    let gate = Arc::new(Semaphore::new(0));
    let block = gate.clone();
    let writer = Arc::new(Writer::new(
        config(1, 1),
        OverflowPolicy::Reject,
        None,
        move |_| {
            let gate = block.clone();
            async move {
                gate.acquire().await.unwrap().forget();
                Ok(())
            }
        },
    ));
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    writer.stop(Instant::now()).await;
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Stopped)
    ));
}

#[tokio::test(start_paused = true)]
async fn simultaneous_lane_failures_retry_only_safe_records_without_replaying_unknown_commits() {
    let classes = [
        WriteFailureClass::Retryable,
        WriteFailureClass::Permanent,
        WriteFailureClass::UnknownCommit,
    ];
    for batch_class in classes {
        for direct_class in classes {
            simultaneous_lane_failure_contract(batch_class, direct_class).await;
        }
    }
}

async fn simultaneous_lane_failure_contract(
    batch_class: WriteFailureClass,
    direct_class: WriteFailureClass,
) {
    let attempts = Arc::new(Mutex::new([0; 2]));
    let durable = Arc::new(Mutex::new(Vec::new()));
    let (calls, committed) = (attempts.clone(), durable.clone());
    let writer = Writer::new(
        config(2, 2),
        OverflowPolicy::DirectWrite,
        None,
        move |items| {
            let direct = items == [3];
            let lane = usize::from(direct);
            let class = if direct { direct_class } else { batch_class };
            let attempt = {
                let mut calls = calls.lock().unwrap();
                calls[lane] += 1;
                calls[lane]
            };
            // An unknown commit deliberately writes before returning failure: checking only
            // writer counters would miss a duplicate physical insert after an unsafe replay.
            if attempt > 1 || class == WriteFailureClass::UnknownCommit {
                committed.lock().unwrap().extend(items);
            }
            async move {
                if attempt == 1 {
                    Err(WriteFailure::new(
                        class,
                        if direct { "direct" } else { "batch" },
                    ))
                } else {
                    Ok(())
                }
            }
        },
    );
    writer.start().await.unwrap();
    writer.record(1).await.unwrap();
    writer.record(2).await.unwrap();
    let direct = writer.record(3);
    tokio::pin!(direct);
    assert!(futures::poll!(&mut direct).is_pending());
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    let Err(RecordError::DirectFailed(failure)) = direct.await else {
        panic!("direct failure must reach its original waiter")
    };
    assert_eq!(failure.class, direct_class);
    assert_eq!(writer.unconfirmed(), [1, 2, 3]);
    assert_eq!(*attempts.lock().unwrap(), [1, 1]);
    advance(Duration::from_secs(100)).await;
    settle().await;
    assert_eq!(
        *attempts.lock().unwrap(),
        [1, 1],
        "neither lane retries implicitly"
    );

    writer.retry_failed();
    let all_safe =
        batch_class == WriteFailureClass::Retryable && direct_class == WriteFailureClass::Retryable;
    let result = writer.flush(deadline()).await;
    assert_eq!(result.is_ok(), all_safe);
    let report = writer.stop(deadline()).await;
    let batch_safe = batch_class == WriteFailureClass::Retryable;
    let direct_safe = direct_class == WriteFailureClass::Retryable;
    assert_eq!(
        report.snapshot.persisted,
        2 * u64::from(batch_safe) + u64::from(direct_safe)
    );
    assert_eq!(report.snapshot.direct_persisted, u64::from(direct_safe));
    assert_eq!(
        *attempts.lock().unwrap(),
        [1 + usize::from(batch_safe), 1 + usize::from(direct_safe)]
    );
    let mut expected_unconfirmed = Vec::new();
    if !batch_safe {
        expected_unconfirmed.extend([1, 2]);
    }
    if !direct_safe {
        expected_unconfirmed.push(3);
    }
    assert_eq!(writer.unconfirmed(), expected_unconfirmed);
    let mut expected_durable = Vec::new();
    if batch_class != WriteFailureClass::Permanent {
        expected_durable.extend([1, 2]);
    }
    if direct_class != WriteFailureClass::Permanent {
        expected_durable.push(3);
    }
    let mut actual = durable.lock().unwrap().clone();
    actual.sort_unstable();
    assert_eq!(
        actual, expected_durable,
        "unknown commits must never be replayed"
    );
    writer.retry_failed();
    assert_eq!(writer.unconfirmed(), expected_unconfirmed);
}
