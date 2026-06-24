//! Typed task specification adapters.

use std::future::Future;
use std::pin::Pin;

use serde::{Serialize, de::DeserializeOwned};

use crate::{Result, TaskCoreError, TaskRecord, TaskRetryClass, TaskStepSpec};

/// Boxed future returned by task processors.
pub type TaskProcessFuture<'a, Error> =
    Pin<Box<dyn Future<Output = std::result::Result<(), Error>> + Send + 'a>>;

/// Product-owned typed task specification.
///
/// The generic parameters keep Forge independent from product state, persisted task model, runtime
/// config, execution context, error type, task kind enum, lane enum, and payload/result wrapper
/// enums. Product crates implement this trait for each task kind and register those specs with
/// [`crate::task_registry!`].
pub trait BackgroundTaskSpec<State, Task, Config, Context, Error>: Sync
where
    Task: TaskRecord<Self::Kind>,
{
    /// Product-owned task kind enum.
    type Kind: Copy + Eq + std::fmt::Debug + std::fmt::Display + Send + Sync + 'static;
    /// Product-owned task lane enum.
    type Lane: Copy + Eq + std::fmt::Debug + Send + Sync + 'static;
    /// Typed task payload.
    type Payload: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    /// Typed task result.
    type Result: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    /// Product task payload envelope enum.
    type PayloadEnvelope;
    /// Product task result envelope enum.
    type ResultEnvelope;

    /// Task kind handled by this spec.
    const KIND: Self::Kind;

    /// Initial step specs for this task kind.
    fn step_specs() -> &'static [TaskStepSpec];

    /// Dispatch lane used by this task kind.
    fn lane() -> Self::Lane;

    /// Maximum attempts for new tasks of this kind.
    fn max_attempts(_runtime_config: &Config) -> i32 {
        1
    }

    /// Wraps the typed payload into the product payload envelope.
    fn wrap_payload(payload: Self::Payload) -> Self::PayloadEnvelope;

    /// Wraps the typed result into the product result envelope.
    fn wrap_result(result: Self::Result) -> Self::ResultEnvelope;

    /// Processes the task.
    fn process<'a>(
        state: &'a State,
        task: &'a Task,
        context: Context,
    ) -> TaskProcessFuture<'a, Error>;

    /// Classifies a task failure for retry behavior.
    fn retry_class(_error: &Error) -> TaskRetryClass {
        TaskRetryClass::Manual
    }
}

/// Serializes a typed task payload.
pub fn serialize_payload<S, State, Task, Config, Context, Error>(
    payload: &S::Payload,
) -> Result<String>
where
    S: BackgroundTaskSpec<State, Task, Config, Context, Error>,
    Task: TaskRecord<S::Kind>,
{
    serde_json::to_string(payload).map_err(|error| {
        TaskCoreError::codec(format!("serialize {} task payload: {error}", S::KIND))
    })
}

/// Serializes a typed task result.
pub fn serialize_result<S, State, Task, Config, Context, Error>(
    result: &S::Result,
) -> Result<String>
where
    S: BackgroundTaskSpec<State, Task, Config, Context, Error>,
    Task: TaskRecord<S::Kind>,
{
    serde_json::to_string(result).map_err(|error| {
        TaskCoreError::codec(format!("serialize {} task result: {error}", S::KIND))
    })
}

/// Decodes a task payload as the typed payload for `S`.
pub fn decode_payload_as<S, State, Task, Config, Context, Error>(task: &Task) -> Result<S::Payload>
where
    S: BackgroundTaskSpec<State, Task, Config, Context, Error>,
    Task: TaskRecord<S::Kind>,
{
    if task.kind() != S::KIND {
        return Err(TaskCoreError::invalid_value(format!(
            "task #{} kind mismatch: expected {}, got {}",
            task.id(),
            S::KIND,
            task.kind()
        )));
    }

    serde_json::from_str(task.payload_json()).map_err(|error| {
        TaskCoreError::codec(format!(
            "parse payload for task #{} ({}): {error}",
            task.id(),
            task.kind()
        ))
    })
}

/// Decodes a task result as the typed result for `S`.
pub fn decode_result_as<S, State, Task, Config, Context, Error>(
    task: &Task,
) -> Result<Option<S::Result>>
where
    S: BackgroundTaskSpec<State, Task, Config, Context, Error>,
    Task: TaskRecord<S::Kind>,
{
    if task.kind() != S::KIND {
        return Err(TaskCoreError::invalid_value(format!(
            "task #{} kind mismatch: expected {}, got {}",
            task.id(),
            S::KIND,
            task.kind()
        )));
    }

    let Some(raw) = task.result_json() else {
        return Ok(None);
    };

    serde_json::from_str(raw).map(Some).map_err(|error| {
        TaskCoreError::codec(format!(
            "parse result for task #{} ({}): {error}",
            task.id(),
            task.kind()
        ))
    })
}

/// Object-safe task spec used by registries and dispatchers.
pub trait ErasedBackgroundTaskSpec<
    State,
    Task,
    Config,
    Context,
    Kind,
    Lane,
    PayloadEnvelope,
    ResultEnvelope,
    Error,
>: Sync where
    Task: TaskRecord<Kind>,
    Kind: Copy + Eq + std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    Lane: Copy + Eq + std::fmt::Debug + Send + Sync + 'static,
{
    /// Initial step specs for this task kind.
    fn step_specs(&self) -> &'static [TaskStepSpec];

    /// Dispatch lane used by this task kind.
    fn lane(&self) -> Lane;

    /// Maximum attempts for new tasks of this kind.
    fn max_attempts(&self, runtime_config: &Config) -> i32;

    /// Decodes the product task payload envelope.
    fn decode_payload(&self, task: &Task) -> Result<PayloadEnvelope>;

    /// Decodes the product task result envelope.
    fn decode_result(&self, task: &Task) -> Result<Option<ResultEnvelope>>;

    /// Classifies a task failure for retry behavior.
    fn retry_class(&self, error: &Error) -> TaskRetryClass;

    /// Processes the task.
    fn process<'a>(
        &self,
        state: &'a State,
        task: &'a Task,
        context: Context,
    ) -> TaskProcessFuture<'a, Error>;
}

/// Zero-sized adapter from typed task specs to object-safe task specs.
pub struct TaskSpecAdapter<S>(std::marker::PhantomData<S>);

impl<S> TaskSpecAdapter<S> {
    /// Creates a task spec adapter.
    pub const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<S> Default for TaskSpecAdapter<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, State, Task, Config, Context, Kind, Lane, PayloadEnvelope, ResultEnvelope, Error>
    ErasedBackgroundTaskSpec<
        State,
        Task,
        Config,
        Context,
        Kind,
        Lane,
        PayloadEnvelope,
        ResultEnvelope,
        Error,
    > for TaskSpecAdapter<S>
where
    S: BackgroundTaskSpec<
            State,
            Task,
            Config,
            Context,
            Error,
            Kind = Kind,
            Lane = Lane,
            PayloadEnvelope = PayloadEnvelope,
            ResultEnvelope = ResultEnvelope,
        > + Sync,
    Task: TaskRecord<Kind>,
    Kind: Copy + Eq + std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    Lane: Copy + Eq + std::fmt::Debug + Send + Sync + 'static,
{
    fn step_specs(&self) -> &'static [TaskStepSpec] {
        S::step_specs()
    }

    fn lane(&self) -> Lane {
        S::lane()
    }

    fn max_attempts(&self, runtime_config: &Config) -> i32 {
        S::max_attempts(runtime_config)
    }

    fn decode_payload(&self, task: &Task) -> Result<PayloadEnvelope> {
        Ok(S::wrap_payload(decode_payload_as::<
            S,
            State,
            Task,
            Config,
            Context,
            Error,
        >(task)?))
    }

    fn decode_result(&self, task: &Task) -> Result<Option<ResultEnvelope>> {
        Ok(decode_result_as::<S, State, Task, Config, Context, Error>(task)?.map(S::wrap_result))
    }

    fn retry_class(&self, error: &Error) -> TaskRetryClass {
        S::retry_class(error)
    }

    fn process<'a>(
        &self,
        state: &'a State,
        task: &'a Task,
        context: Context,
    ) -> TaskProcessFuture<'a, Error> {
        S::process(state, task, context)
    }
}
