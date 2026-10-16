//! Caller-owned, failure-aware buffering. Construction never starts work.
//!
//! Records remain in memory until persistence acknowledges them. Sink errors pause the affected
//! lane; only explicitly retryable failures can be resumed. Dropping the owner cancels/aborts its
//! worker, while `stop` additionally joins it before dependencies may close.

use std::collections::VecDeque;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use futures::FutureExt;
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Bounded in-memory buffering policy. Zero capacities normalize to one.
#[derive(Debug, Clone, Copy)]
pub struct BufferedBatchConfig {
    /// Maximum unacknowledged batch records, including the batch currently being written.
    pub queue_capacity: usize,
    /// Immediate-flush threshold and maximum records passed to a batch sink call.
    pub batch_size: usize,
    /// Maximum delay from the first queued record before attempting a partial batch.
    pub delayed_flush_after: std::time::Duration,
}

impl BufferedBatchConfig {
    /// Creates a policy; the writer normalizes zero capacity and batch size to one.
    #[must_use]
    pub const fn new(
        queue_capacity: usize,
        batch_size: usize,
        delayed_flush_after: std::time::Duration,
    ) -> Self {
        Self {
            queue_capacity,
            batch_size,
            delayed_flush_after,
        }
    }
}

/// Whether an unacknowledged write can safely be replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteFailureClass {
    /// Persistence guarantees no records committed; an explicit retry is safe.
    Retryable,
    /// Persistence rejected the records; retrying unchanged records is inappropriate.
    Permanent,
    /// Some or all records may have committed. Reconcile before any replay.
    UnknownCommit,
}

/// Original persistence error, or a worker-side interruption with unknown outcome.
#[derive(Debug)]
pub enum WriteFailureCause<E> {
    /// The original error, including its native classification.
    Persistence(Arc<E>),
    /// The sink or worker panicked before completing outcome bookkeeping.
    Panicked,
    /// The sink future was cancelled before acknowledgment.
    Cancelled,
    /// Accepted overflow was cancelled while waiting; persistence was never invoked.
    NotAttempted,
}

/// A failure retained alongside the unconfirmed records.
#[derive(Debug)]
pub struct WriteFailure<E> {
    /// Replay eligibility.
    pub class: WriteFailureClass,
    /// Original failure cause.
    pub cause: WriteFailureCause<E>,
}

impl<E> Clone for WriteFailure<E> {
    fn clone(&self) -> Self {
        Self {
            class: self.class,
            cause: match &self.cause {
                WriteFailureCause::Persistence(error) => {
                    WriteFailureCause::Persistence(error.clone())
                }
                WriteFailureCause::Panicked => WriteFailureCause::Panicked,
                WriteFailureCause::Cancelled => WriteFailureCause::Cancelled,
                WriteFailureCause::NotAttempted => WriteFailureCause::NotAttempted,
            },
        }
    }
}

impl<E> WriteFailure<E> {
    /// Classifies an error without erasing its concrete type.
    #[must_use]
    pub fn new(class: WriteFailureClass, error: E) -> Self {
        Self {
            class,
            cause: WriteFailureCause::Persistence(Arc::new(error)),
        }
    }
}

/// Explicit admission policy when the bounded batch queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowPolicy {
    /// Reject and return ownership of the record.
    Reject,
    /// Use a separate single-record persistence lane, bounded to one outstanding record.
    DirectWrite,
}

/// Writer lifecycle. A stopped writer cannot be restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferedWriterState {
    /// Construction has completed without creating a worker.
    Created,
    /// Admission and background persistence are enabled.
    Running,
    /// Admission is closed; accepted records can still be persisted.
    Draining,
    /// The worker has terminated; unconfirmed records require explicit recovery.
    Stopped,
}

/// Successful admission does not imply batch durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordAdmission {
    /// Accepted into memory only.
    Queued,
    /// Only this overflow record was acknowledged by persistence.
    DirectPersisted,
}

/// Admission failure. Rejected records are returned; accepted failed direct writes stay in memory.
#[derive(Debug)]
pub enum RecordError<T, E> {
    /// Admission is closed or startup has not completed.
    NotRunning(T),
    /// The configured queue/direct-lane bound was reached.
    Full(T),
    /// An accepted direct write failed and remains unconfirmed in the writer.
    DirectFailed(WriteFailure<E>),
}

/// Flush failure; inspect the snapshot and unconfirmed records before recovery.
#[derive(Debug)]
pub enum BufferFlushError<E> {
    /// Start must be called explicitly before flushing records.
    NotStarted,
    /// The worker has stopped with records still awaiting reconciliation.
    Stopped,
    /// A retained batch or direct record failed.
    Write(WriteFailure<E>),
    /// The caller's deadline elapsed. No durability is implied.
    Deadline,
}

/// Low-cardinality events delivered with `try_send`; persistence never calls observer code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferedWriterEvent {
    /// A record was accepted into the batch queue.
    Queued,
    /// One overflow record was acknowledged.
    DirectPersisted,
    /// One batch was acknowledged.
    BatchPersisted,
    /// A sink failed or was interrupted.
    Failed,
    /// Admission was rejected.
    Rejected,
    /// The worker has exited.
    Stopped,
}

/// Observable per-instance state. Counts include in-flight, unacknowledged records.
#[derive(Debug)]
pub struct BufferedWriterSnapshot<E> {
    /// Current admission/worker lifecycle.
    pub state: BufferedWriterState,
    /// Unacknowledged batch records, including any in-flight chunk.
    pub queued: usize,
    /// Unacknowledged overflow records (zero or one).
    pub direct_pending: usize,
    /// Total acknowledged records across both lanes.
    pub persisted: u64,
    /// Acknowledged overflow records only; does not imply batch durability.
    pub direct_persisted: u64,
    /// Events dropped because the bounded observer channel was full or closed.
    pub observer_events_dropped: u64,
    /// Failure blocking the front chunk; originals remain in queue order.
    pub batch_failure: Option<WriteFailure<E>>,
    /// Front records affected by `batch_failure`; later queued records were not attempted.
    pub batch_failure_records: usize,
    /// Failure blocking the retained overflow record.
    pub direct_failure: Option<WriteFailure<E>>,
}

/// Drain result. Once returned, the worker is joined and database dependencies may close.
#[derive(Debug)]
pub struct BufferStopReport<E> {
    /// Whether this stop caller exhausted its drain budget.
    pub deadline_elapsed: bool,
    /// Final state after joining the worker; remaining records have no confirmed durability.
    pub snapshot: BufferedWriterSnapshot<E>,
}

struct Direct<T, E> {
    item: T,
    reply: Option<oneshot::Sender<Result<RecordAdmission, WriteFailure<E>>>>,
    failure: Option<WriteFailure<E>>,
}

struct State<T, E> {
    lifecycle: BufferedWriterState,
    queue: VecDeque<T>,
    oldest: Option<Instant>,
    direct: Option<Direct<T, E>>,
    failure: Option<WriteFailure<E>>,
    failure_records: usize,
    force: bool,
    persisted: u64,
    direct_persisted: u64,
    observer_dropped: u64,
}

type Sink<T, E> = dyn Fn(Vec<T>) -> Pin<Box<dyn Future<Output = Result<(), WriteFailure<E>>> + Send>>
    + Send
    + Sync;

struct Core<T, E> {
    config: BufferedBatchConfig,
    overflow: OverflowPolicy,
    state: Mutex<State<T, E>>,
    wake: Notify,
    changed: Notify,
    cancel: CancellationToken,
    sink: Box<Sink<T, E>>,
    events: Option<mpsc::Sender<BufferedWriterEvent>>,
}

impl<T, E> Core<T, E> {
    fn state(&self) -> MutexGuard<'_, State<T, E>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn event(&self, state: &mut State<T, E>, event: BufferedWriterEvent) {
        if let Some(events) = &self.events
            && events.try_send(event).is_err()
        {
            state.observer_dropped = state.observer_dropped.saturating_add(1);
        }
    }
}

/// Explicitly started and stopped buffering with one owned worker and bounded overflow.
///
/// The sink must be cancellation-safe: interruption means an unknown commit, not a safe retry.
/// `T: Clone` permits retaining originals while an owned copy is passed to persistence.
/// Observers receive a bounded channel created by the caller; callbacks run outside persistence.
/// No automatic retry occurs, including for `Retryable` failures.
pub struct ManagedBatchWriter<T, E> {
    core: Arc<Core<T, E>>,
    worker: AsyncMutex<Option<JoinHandle<()>>>,
}

impl<T, E> Drop for ManagedBatchWriter<T, E> {
    fn drop(&mut self) {
        self.core.cancel.cancel();
        if let Some(worker) = self.worker.get_mut().take() {
            worker.abort();
        }
    }
}

impl<T: Clone + Send + 'static, E: Send + Sync + 'static> ManagedBatchWriter<T, E> {
    /// Constructs an idle owner. Call `start` from the runtime startup phase.
    #[must_use]
    pub fn new<F, Fut>(
        config: BufferedBatchConfig,
        overflow: OverflowPolicy,
        events: Option<mpsc::Sender<BufferedWriterEvent>>,
        sink: F,
    ) -> Self
    where
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), WriteFailure<E>>> + Send + 'static,
    {
        let config = BufferedBatchConfig {
            queue_capacity: config.queue_capacity.max(1),
            batch_size: config.batch_size.max(1),
            ..config
        };
        Self {
            core: Arc::new(Core {
                config,
                overflow,
                events,
                state: Mutex::new(State {
                    lifecycle: BufferedWriterState::Created,
                    queue: VecDeque::new(),
                    oldest: None,
                    direct: None,
                    failure: None,
                    failure_records: 0,
                    force: false,
                    persisted: 0,
                    direct_persisted: 0,
                    observer_dropped: 0,
                }),
                wake: Notify::new(),
                changed: Notify::new(),
                cancel: CancellationToken::new(),
                sink: Box::new(move |items| Box::pin(sink(items))),
            }),
            worker: AsyncMutex::new(None),
        }
    }

    /// Starts exactly once. Returns the current state if startup has already been attempted.
    ///
    /// # Errors
    /// Returns the current state unless this is the first start from `Created`.
    pub async fn start(&self) -> Result<(), BufferedWriterState> {
        let mut worker = self.worker.lock().await;
        let mut state = self.core.state();
        if state.lifecycle != BufferedWriterState::Created {
            return Err(state.lifecycle);
        }
        state.lifecycle = BufferedWriterState::Running;
        let core = self.core.clone();
        *worker = Some(tokio::spawn(async move {
            // Sink panics are handled per write. Also supervise the entire worker: record Clone
            // implementations and other code outside the sink must not strand admission waiters.
            if AssertUnwindSafe(run(core.clone()))
                .catch_unwind()
                .await
                .is_err()
            {
                core.fail_worker(WriteFailureCause::Panicked);
            }
        }));
        Ok(())
    }

    /// Admits a record or returns it when rejected. Cancellation of a direct waiter does not
    /// cancel its accepted record: the owned worker still persists or retains it.
    ///
    /// # Errors
    /// Rejection returns the record. A direct persistence failure retains it in the owner.
    pub async fn record(&self, item: T) -> Result<RecordAdmission, RecordError<T, E>> {
        let reply = {
            let mut state = self.core.state();
            if state.lifecycle != BufferedWriterState::Running {
                self.core.event(&mut state, BufferedWriterEvent::Rejected);
                return Err(RecordError::NotRunning(item));
            }
            if state.queue.len() < self.core.config.queue_capacity {
                if state.queue.is_empty() {
                    state.oldest = Some(Instant::now());
                }
                state.queue.push_back(item);
                self.core.event(&mut state, BufferedWriterEvent::Queued);
                self.core.wake.notify_one();
                return Ok(RecordAdmission::Queued);
            }
            if self.core.overflow == OverflowPolicy::Reject || state.direct.is_some() {
                self.core.event(&mut state, BufferedWriterEvent::Rejected);
                return Err(RecordError::Full(item));
            }
            let (send, receive) = oneshot::channel();
            state.direct = Some(Direct {
                item,
                reply: Some(send),
                failure: None,
            });
            receive
        };
        self.core.wake.notify_one();
        reply
            .await
            .unwrap_or_else(|_| {
                Err(WriteFailure {
                    class: WriteFailureClass::UnknownCommit,
                    cause: WriteFailureCause::Cancelled,
                })
            })
            .map_err(RecordError::DirectFailed)
    }

    /// Atomically stops admission. Accepted records remain owned and can still flush.
    pub fn begin_drain(&self) {
        let mut state = self.core.state();
        if state.lifecycle != BufferedWriterState::Stopped {
            state.lifecycle = BufferedWriterState::Draining;
        }
        self.core.wake.notify_one();
    }

    /// Flushes accepted records with a caller deadline, retaining failures and in-flight items.
    /// Dropping this future does not cancel persistence; `stop` owns worker termination.
    /// Continues draining an unaffected lane even if the other lane has failed.
    ///
    /// # Errors
    /// Returns a lifecycle error, the original write failure, or deadline exhaustion.
    pub async fn flush(&self, deadline: Instant) -> Result<(), BufferFlushError<E>> {
        loop {
            let changed = self.core.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.core.state();
                if state.lifecycle == BufferedWriterState::Created {
                    return Err(BufferFlushError::NotStarted);
                }
                let work_pending = (!state.queue.is_empty() && state.failure.is_none())
                    || state.direct.as_ref().is_some_and(|d| d.failure.is_none());
                if !work_pending
                    && let Some(failure) = state
                        .failure
                        .as_ref()
                        .or_else(|| state.direct.as_ref().and_then(|d| d.failure.as_ref()))
                {
                    return Err(BufferFlushError::Write(failure.clone()));
                }
                if state.queue.is_empty() && state.direct.is_none() {
                    return Ok(());
                }
                if state.lifecycle == BufferedWriterState::Stopped {
                    return Err(BufferFlushError::Stopped);
                }
                state.force = true;
            }
            self.core.wake.notify_one();
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(BufferFlushError::Deadline);
            }
        }
    }

    /// Clears only failures classified as safe to retry. Unknown/permanent records stay blocked.
    pub fn retry_failed(&self) {
        let mut state = self.core.state();
        if state.lifecycle == BufferedWriterState::Stopped {
            return;
        }
        if state
            .failure
            .as_ref()
            .is_some_and(|f| f.class == WriteFailureClass::Retryable)
        {
            state.failure = None;
            state.failure_records = 0;
            state.force = true;
        }
        if let Some(direct) = &mut state.direct
            && direct
                .failure
                .as_ref()
                .is_some_and(|f| f.class == WriteFailureClass::Retryable)
        {
            direct.failure = None;
        }
        self.core.wake.notify_one();
    }

    /// Stops admission, drains until deadline/failure, then cancels and joins the owned worker.
    /// Repeated/concurrent calls share the same handle; cancelling a stop caller leaves the handle
    /// available for another caller. Dependencies must close only after this future returns.
    pub async fn stop(&self, deadline: Instant) -> BufferStopReport<E> {
        self.begin_drain();
        let result = self.flush(deadline).await;
        self.core.cancel.cancel();
        let mut worker = self.worker.lock().await;
        if let Some(task) = worker.as_mut()
            && let Err(error) = task.await
        {
            self.core.fail_worker(if error.is_panic() {
                WriteFailureCause::Panicked
            } else {
                WriteFailureCause::Cancelled
            });
        }
        *worker = None;
        {
            let mut state = self.core.state();
            state.lifecycle = BufferedWriterState::Stopped;
        }
        BufferStopReport {
            deadline_elapsed: matches!(result, Err(BufferFlushError::Deadline)),
            snapshot: self.snapshot(),
        }
    }

    /// Captures independent state and counters, without invoking observer code.
    #[must_use]
    pub fn snapshot(&self) -> BufferedWriterSnapshot<E> {
        let state = self.core.state();
        BufferedWriterSnapshot {
            state: state.lifecycle,
            queued: state.queue.len(),
            direct_pending: usize::from(state.direct.is_some()),
            persisted: state.persisted,
            direct_persisted: state.direct_persisted,
            observer_events_dropped: state.observer_dropped,
            batch_failure: state.failure.clone(),
            batch_failure_records: state.failure_records,
            direct_failure: state.direct.as_ref().and_then(|d| d.failure.clone()),
        }
    }

    /// Copies all unacknowledged records for reconciliation; never implicitly replays them.
    /// Stop first to obtain a stable recovery snapshot.
    #[must_use]
    pub fn unconfirmed(&self) -> Vec<T> {
        let state = self.core.state();
        state
            .queue
            .iter()
            .chain(state.direct.iter().map(|d| &d.item))
            .cloned()
            .collect()
    }
}

async fn run<T: Clone + Send + 'static, E: Send + Sync + 'static>(core: Arc<Core<T, E>>) {
    // Alternate ready lanes so a flood of overflow records cannot starve a queued batch.
    let mut prefer_batch = false;
    loop {
        let wake = core.wake.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        if core.cancel.is_cancelled() {
            break;
        }
        let (work, due) = {
            let state = core.state();
            let due = state
                .oldest
                .and_then(|at| at.checked_add(core.config.delayed_flush_after));
            let batch_ready = !state.queue.is_empty()
                && state.failure.is_none()
                && (state.force
                    || state.lifecycle == BufferedWriterState::Draining
                    || state.queue.len() >= core.config.batch_size
                    || due.is_some_and(|at| at <= Instant::now()));
            if let Some(direct) = &state.direct
                && direct.failure.is_none()
                && !(prefer_batch && batch_ready)
            {
                (Some((vec![direct.item.clone()], true)), due)
            } else if batch_ready {
                (
                    Some((
                        state
                            .queue
                            .iter()
                            .take(core.config.batch_size)
                            .cloned()
                            .collect::<Vec<_>>(),
                        false,
                    )),
                    due,
                )
            } else {
                (None, if state.failure.is_none() { due } else { None })
            }
        };
        let Some((items, direct)) = work else {
            tokio::select! {
                biased;
                () = core.cancel.cancelled() => break,
                () = &mut wake => {},
                () = async { match due { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => {},
            }
            continue;
        };
        prefer_batch = direct;
        let count = items.len();
        let result = tokio::select! {
            biased;
            () = core.cancel.cancelled() => Err(WriteFailure { class: WriteFailureClass::UnknownCommit, cause: WriteFailureCause::Cancelled }),
            result = AssertUnwindSafe(async { (core.sink)(items).await }).catch_unwind() => {
                result.unwrap_or_else(|_| Err(WriteFailure { class: WriteFailureClass::UnknownCommit, cause: WriteFailureCause::Panicked }))
            }
        };
        core.complete(direct, count, &result);
        core.changed.notify_waiters();
    }
    core.finish();
}

impl<T, E> Core<T, E> {
    // An unexpected worker exit has no reliable write boundary. Preserve existing native
    // failures, conservatively quarantine other records, and wake every accepted direct waiter.
    fn fail_worker(&self, cause: WriteFailureCause<E>) {
        let failure = WriteFailure {
            class: WriteFailureClass::UnknownCommit,
            cause,
        };
        {
            let mut state = self.state();
            if !state.queue.is_empty() && state.failure.is_none() {
                state.failure = Some(failure.clone());
                state.failure_records = state.queue.len();
            }
            if let Some(direct) = &mut state.direct {
                if direct.failure.is_none() {
                    direct.failure = Some(failure.clone());
                }
                if let Some(reply) = direct.reply.take() {
                    let _ = reply.send(Err(failure));
                }
            }
            self.event(&mut state, BufferedWriterEvent::Failed);
        }
        self.finish();
    }

    // The worker is the sole consumer. Originals are removed only after acknowledgment, while
    // producers may append under the same short-held state lock during sink I/O.
    fn complete(&self, direct: bool, count: usize, result: &Result<(), WriteFailure<E>>) {
        let mut state = self.state();
        if direct {
            if let Some(mut pending) = state.direct.take() {
                if let Some(reply) = pending.reply.take() {
                    let _ = reply.send(
                        result
                            .as_ref()
                            .map(|()| RecordAdmission::DirectPersisted)
                            .map_err(Clone::clone),
                    );
                }
                if let Err(failure) = result {
                    pending.failure = Some(failure.clone());
                    state.direct = Some(pending);
                } else {
                    state.persisted += 1;
                    state.direct_persisted += 1;
                }
            }
        } else if let Err(failure) = result {
            state.failure = Some(failure.clone());
            state.failure_records = count;
        } else {
            state.queue.drain(..count);
            state.persisted += count as u64;
            if state.queue.is_empty() {
                state.oldest = None;
                state.force = false;
            }
        }
        self.event(
            &mut state,
            if result.is_err() {
                BufferedWriterEvent::Failed
            } else if direct {
                BufferedWriterEvent::DirectPersisted
            } else {
                BufferedWriterEvent::BatchPersisted
            },
        );
    }

    // Wake every direct/flush waiter on exit. An overflow slot not yet attempted is safe to
    // recover; an interrupted sink was already marked UnknownCommit by the worker.
    fn finish(&self) {
        {
            let mut state = self.state();
            // A direct admission may still be waiting behind another in-flight operation.
            if let Some(pending) = &mut state.direct
                && pending.failure.is_none()
            {
                let failure = WriteFailure {
                    class: WriteFailureClass::Retryable,
                    cause: WriteFailureCause::NotAttempted,
                };
                if let Some(reply) = pending.reply.take() {
                    let _ = reply.send(Err(failure.clone()));
                }
                pending.failure = Some(failure);
            }
            state.lifecycle = BufferedWriterState::Stopped;
            self.event(&mut state, BufferedWriterEvent::Stopped);
        }
        self.changed.notify_waiters();
    }
}

impl<E: std::fmt::Display> std::fmt::Display for WriteFailure<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "write failed ({:?}): ", self.class)?;
        match &self.cause {
            WriteFailureCause::Persistence(error) => error.fmt(f),
            WriteFailureCause::Panicked => {
                f.write_str("sink or worker panicked; commit outcome unknown")
            }
            WriteFailureCause::Cancelled => f.write_str("sink interrupted; commit outcome unknown"),
            WriteFailureCause::NotAttempted => {
                f.write_str("cancelled before persistence was attempted")
            }
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for WriteFailure<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            WriteFailureCause::Persistence(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}
impl<T, E: std::fmt::Display> std::fmt::Display for RecordError<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning(_) => f.write_str("writer admission is closed or not started"),
            Self::Full(_) => f.write_str("writer admission capacity exhausted"),
            Self::DirectFailed(error) => {
                write!(f, "accepted overflow record remains unconfirmed: {error}")
            }
        }
    }
}
impl<T: std::fmt::Debug, E: std::error::Error + 'static> std::error::Error for RecordError<T, E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DirectFailed(error) => Some(error),
            _ => None,
        }
    }
}
impl<E: std::fmt::Display> std::fmt::Display for BufferFlushError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted => f.write_str("writer has not started"),
            Self::Stopped => f.write_str("writer stopped with unconfirmed records"),
            Self::Write(error) => error.fmt(f),
            Self::Deadline => f.write_str("flush deadline elapsed; records remain unconfirmed"),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for BufferFlushError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Write(error) => Some(error),
            _ => None,
        }
    }
}
