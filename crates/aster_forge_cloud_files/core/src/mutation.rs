//! Durable product-neutral mutation identities, intents, outcomes, and recovery states.

use std::{fmt, num::NonZeroU64, time::SystemTime};

use crate::{
    CloudFilesCoreError, CloudItem, CloudItemId, CloudItemKey, CloudItemKind, CloudScope,
    ContentRevision, LocalContentSnapshot, MetadataRevision, Result,
};

macro_rules! opaque_string {
    ($name:ident, $field:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            /// Creates a non-empty opaque value and preserves it exactly.
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.is_empty() {
                    return Err(CloudFilesCoreError::empty($field));
                }
                Ok(Self(value))
            }

            /// Returns the opaque value.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the wrapper and returns the opaque value.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($name))
                    .field("byte_len", &self.0.len())
                    .finish()
            }
        }
    };
}

opaque_string!(
    OperationId,
    "operation id",
    "Stable identity of one durable mutation journal operation."
);
opaque_string!(
    IdempotencyKey,
    "idempotency key",
    "Opaque backend reconciliation key used to detect an already committed mutation."
);
opaque_string!(
    PlatformRequestCorrelation,
    "platform request correlation",
    "Owned correlation token for a platform request; native pointers and handles are excluded."
);
opaque_string!(
    RemoteReconciliationKey,
    "remote reconciliation key",
    "Opaque key used by status queries or change-feed reconciliation."
);

/// Monotonic fence for one platform connection, extension instance, or mount session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionGeneration(NonZeroU64);

impl SessionGeneration {
    /// Creates a non-zero session generation.
    pub const fn new(value: u64) -> Result<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(CloudFilesCoreError::InvalidSessionGeneration),
        }
    }

    /// Returns the numeric fence value.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Lifecycle of one active platform session generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// New requests may be accepted.
    Accepting,
    /// New requests are rejected while accepted work is being classified for drain or cancel.
    Closing,
    /// Accepted work is being drained, cancelled, or explicitly failed.
    Draining,
    /// The generation no longer accepts completions that mutate active session state.
    Closed,
}

impl SessionState {
    /// Returns whether moving from this state to `next` is a valid idempotent lifecycle step.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Accepting, Self::Accepting | Self::Closing)
                | (Self::Closing, Self::Closing | Self::Draining)
                | (Self::Draining, Self::Draining | Self::Closed)
                | (Self::Closed, Self::Closed)
        )
    }
}

/// How a local or remote effect entered the shared mutation/reconciliation mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationOrigin {
    /// A native operation waits for durable acceptance and the configured remote outcome.
    PlatformCommand,
    /// A native operation requires an allow, deny, or acknowledgement before local completion.
    PlatformPreflight,
    /// The local or platform effect already happened and must be journaled and reconciled.
    PlatformObserved,
    /// A durable remote change is being reconciled into platform state.
    RemoteChange,
}

/// Conditional revisions captured when a mutation intent is created.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationPreconditions {
    metadata_revision: Option<MetadataRevision>,
    content_revision: Option<ContentRevision>,
}

impl MutationPreconditions {
    /// Creates independent metadata and content revision preconditions.
    pub const fn new(
        metadata_revision: Option<MetadataRevision>,
        content_revision: Option<ContentRevision>,
    ) -> Self {
        Self {
            metadata_revision,
            content_revision,
        }
    }

    /// Returns the expected metadata revision, when one is required.
    pub const fn metadata_revision(&self) -> Option<&MetadataRevision> {
        self.metadata_revision.as_ref()
    }

    /// Returns the expected content revision, when one is required.
    pub const fn content_revision(&self) -> Option<&ContentRevision> {
        self.content_revision.as_ref()
    }
}

/// Product-neutral desired effect of a durable mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesiredMutation {
    /// Creates a child below an existing directory.
    Create {
        /// Namespace/root scope in which the child will be created.
        scope: CloudScope,
        /// Stable identity of the desired parent.
        parent_id: CloudItemId,
        /// Desired child name, preserved without product normalization.
        name: String,
        /// Desired item kind.
        kind: CloudItemKind,
    },
    /// Replaces local content for an existing file.
    ModifyContent {
        /// Stable identity of the file being modified.
        key: CloudItemKey,
    },
    /// Changes the parent, name, or both for an existing item.
    ModifyMetadata {
        /// Stable identity preserved by the metadata mutation.
        key: CloudItemKey,
        /// Desired parent when this mutation moves the item.
        parent_id: Option<CloudItemId>,
        /// Desired name when this mutation renames the item.
        name: Option<String>,
    },
    /// Deletes an existing item.
    Delete {
        /// Stable identity of the item being deleted.
        key: CloudItemKey,
    },
}

impl DesiredMutation {
    /// Creates a child intent with a non-empty opaque name.
    pub fn create(
        scope: CloudScope,
        parent_id: CloudItemId,
        name: impl Into<String>,
        kind: CloudItemKind,
    ) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(CloudFilesCoreError::empty("mutation create name"));
        }
        Ok(Self::Create {
            scope,
            parent_id,
            name,
            kind,
        })
    }

    /// Creates a content replacement intent.
    pub const fn modify_content(key: CloudItemKey) -> Self {
        Self::ModifyContent { key }
    }

    /// Creates a metadata intent that changes the parent, name, or both.
    pub fn modify_metadata(
        key: CloudItemKey,
        parent_id: Option<CloudItemId>,
        name: Option<String>,
    ) -> Result<Self> {
        if parent_id.is_none() && name.is_none() {
            return Err(CloudFilesCoreError::invalid_mutation_intent(
                "metadata mutation must change parent or name",
            ));
        }
        if name.as_ref().is_some_and(String::is_empty) {
            return Err(CloudFilesCoreError::empty("mutation metadata name"));
        }
        Ok(Self::ModifyMetadata {
            key,
            parent_id,
            name,
        })
    }

    /// Creates a delete intent.
    pub const fn delete(key: CloudItemKey) -> Self {
        Self::Delete { key }
    }

    /// Returns the namespace/root scope affected by this mutation.
    pub const fn scope(&self) -> &CloudScope {
        match self {
            Self::Create { scope, .. } => scope,
            Self::ModifyContent { key }
            | Self::ModifyMetadata { key, .. }
            | Self::Delete { key } => key.scope(),
        }
    }

    /// Returns the existing item key when the mutation targets an already identified item.
    pub const fn existing_item_key(&self) -> Option<&CloudItemKey> {
        match self {
            Self::Create { .. } => None,
            Self::ModifyContent { key }
            | Self::ModifyMetadata { key, .. }
            | Self::Delete { key } => Some(key),
        }
    }

    const fn requires_local_content(&self) -> bool {
        matches!(self, Self::ModifyContent { .. })
    }
}

/// Retry metadata persisted with a mutation intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationRetryMetadata {
    attempt: u32,
    not_before: Option<SystemTime>,
}

impl MutationRetryMetadata {
    /// Creates retry metadata from an attempt count and optional absolute retry time.
    pub const fn new(attempt: u32, not_before: Option<SystemTime>) -> Self {
        Self {
            attempt,
            not_before,
        }
    }

    /// Returns how many remote attempts have already started.
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns the earliest retry time, when one is scheduled.
    pub const fn not_before(&self) -> Option<SystemTime> {
        self.not_before
    }
}

impl Default for MutationRetryMetadata {
    fn default() -> Self {
        Self::new(0, None)
    }
}

/// In-memory intent that must be durably inserted before acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationIntent {
    operation_id: OperationId,
    idempotency_key: IdempotencyKey,
    origin: MutationOrigin,
    session_generation: SessionGeneration,
    desired: DesiredMutation,
    preconditions: MutationPreconditions,
    local_content: Option<LocalContentSnapshot>,
    platform_correlation: Option<PlatformRequestCorrelation>,
    remote_reconciliation_key: Option<RemoteReconciliationKey>,
    retry: MutationRetryMetadata,
}

impl MutationIntent {
    /// Creates an intent. Call [`Self::validate_for_persistence`] before durable insertion.
    pub const fn new(
        operation_id: OperationId,
        idempotency_key: IdempotencyKey,
        origin: MutationOrigin,
        session_generation: SessionGeneration,
        desired: DesiredMutation,
        preconditions: MutationPreconditions,
    ) -> Self {
        Self {
            operation_id,
            idempotency_key,
            origin,
            session_generation,
            desired,
            preconditions,
            local_content: None,
            platform_correlation: None,
            remote_reconciliation_key: None,
            retry: MutationRetryMetadata::new(0, None),
        }
    }

    /// Attaches an owned local-content reference.
    pub fn with_local_content(mut self, local_content: LocalContentSnapshot) -> Self {
        self.local_content = Some(local_content);
        self
    }

    /// Attaches an owned platform correlation token.
    pub fn with_platform_correlation(
        mut self,
        platform_correlation: PlatformRequestCorrelation,
    ) -> Self {
        self.platform_correlation = Some(platform_correlation);
        self
    }

    /// Attaches a backend status/change-feed reconciliation key.
    pub fn with_remote_reconciliation_key(
        mut self,
        remote_reconciliation_key: RemoteReconciliationKey,
    ) -> Self {
        self.remote_reconciliation_key = Some(remote_reconciliation_key);
        self
    }

    /// Replaces persisted retry metadata.
    pub fn with_retry(mut self, retry: MutationRetryMetadata) -> Self {
        self.retry = retry;
        self
    }

    /// Validates invariants needed before the intent becomes recoverable.
    pub fn validate_for_persistence(&self) -> Result<()> {
        if self.desired.requires_local_content() && self.local_content.is_none() {
            return Err(CloudFilesCoreError::invalid_mutation_intent(
                "content mutation requires a local content reference",
            ));
        }
        if !self.desired.requires_local_content() && self.local_content.is_some() {
            return Err(CloudFilesCoreError::invalid_mutation_intent(
                "local content snapshot is only valid for content mutation",
            ));
        }
        if let (Some(key), Some(snapshot)) = (
            self.desired.existing_item_key(),
            self.local_content.as_ref(),
        ) && snapshot.item_key() != key
        {
            return Err(CloudFilesCoreError::invalid_mutation_intent(
                "local content snapshot item does not match the mutation target",
            ));
        }
        Ok(())
    }

    /// Returns the operation identity.
    pub const fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// Returns the backend idempotency key.
    pub const fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// Returns how the mutation entered the shared mechanism.
    pub const fn origin(&self) -> MutationOrigin {
        self.origin
    }

    /// Returns the platform session generation captured by the intent.
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }

    /// Returns the desired product-neutral mutation.
    pub const fn desired(&self) -> &DesiredMutation {
        &self.desired
    }

    /// Returns independent metadata/content revision preconditions.
    pub const fn preconditions(&self) -> &MutationPreconditions {
        &self.preconditions
    }

    /// Returns the owned local-content reference, when needed.
    pub const fn local_content(&self) -> Option<&LocalContentSnapshot> {
        self.local_content.as_ref()
    }

    /// Returns the owned platform request correlation token.
    pub const fn platform_correlation(&self) -> Option<&PlatformRequestCorrelation> {
        self.platform_correlation.as_ref()
    }

    /// Returns the remote status/change-feed reconciliation key.
    pub const fn remote_reconciliation_key(&self) -> Option<&RemoteReconciliationKey> {
        self.remote_reconciliation_key.as_ref()
    }

    /// Returns persisted retry metadata.
    pub const fn retry(&self) -> &MutationRetryMetadata {
        &self.retry
    }
}

/// Durable journal state. `Detected` is deliberately absent because it is not recoverable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationState {
    /// The complete intent is durable and may be acknowledged according to ingress policy.
    IntentPersisted,
    /// A remote effect is in flight.
    RemoteApplying,
    /// Transport completion did not prove whether the backend committed the effect.
    RemoteOutcomeUnknown,
    /// A committed, already-committed, or precondition-failed outcome is durable.
    RemoteOutcomeKnown,
    /// Required platform state has been reconciled with the durable remote outcome.
    PlatformReconciled,
    /// The operation is terminal and replay must not repeat its remote effect.
    Completed,
}

/// Product-neutral result of one conditional remote mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationRemoteOutcome {
    /// The backend committed the requested effect during this attempt.
    Committed {
        /// Current item state when the operation has one; delete may return `None`.
        item: Option<CloudItem>,
    },
    /// Reconciliation proved that an earlier attempt already committed the effect.
    AlreadyCommitted {
        /// Current item state when the operation has one; delete may return `None`.
        item: Option<CloudItem>,
    },
    /// One or more revision preconditions no longer match remote state.
    PreconditionFailed {
        /// Current metadata revision when known.
        metadata_revision: Option<MetadataRevision>,
        /// Current content revision when known.
        content_revision: Option<ContentRevision>,
    },
    /// The transport result does not prove whether the backend committed the effect.
    RemoteOutcomeUnknown,
}

impl MutationRemoteOutcome {
    const fn durable_state(&self) -> MutationState {
        match self {
            Self::RemoteOutcomeUnknown => MutationState::RemoteOutcomeUnknown,
            Self::Committed { .. }
            | Self::AlreadyCommitted { .. }
            | Self::PreconditionFailed { .. } => MutationState::RemoteOutcomeKnown,
        }
    }
}

/// Result of applying an idempotent mutation record transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationRecordTransition {
    /// The durable record changed state.
    Applied,
    /// The record already contained the same transition result.
    AlreadyApplied,
}

/// Recoverable durable mutation journal record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationRecord {
    intent: MutationIntent,
    state: MutationState,
    remote_outcome: Option<MutationRemoteOutcome>,
}

impl MutationRecord {
    /// Converts a validated in-memory intent into its first recoverable journal state.
    pub fn persist(intent: MutationIntent) -> Result<Self> {
        intent.validate_for_persistence()?;
        Ok(Self {
            intent,
            state: MutationState::IntentPersisted,
            remote_outcome: None,
        })
    }

    /// Returns the immutable persisted intent.
    pub const fn intent(&self) -> &MutationIntent {
        &self.intent
    }

    /// Returns the current durable state.
    pub const fn state(&self) -> MutationState {
        self.state
    }

    /// Returns the durable remote outcome, when one has been recorded.
    pub const fn remote_outcome(&self) -> Option<&MutationRemoteOutcome> {
        self.remote_outcome.as_ref()
    }

    /// Marks that a remote effect has started.
    pub fn begin_remote_apply(&mut self) -> Result<MutationRecordTransition> {
        match self.state {
            MutationState::IntentPersisted => {
                self.state = MutationState::RemoteApplying;
                Ok(MutationRecordTransition::Applied)
            }
            MutationState::RemoteApplying
            | MutationState::RemoteOutcomeUnknown
            | MutationState::RemoteOutcomeKnown
            | MutationState::PlatformReconciled
            | MutationState::Completed => Ok(MutationRecordTransition::AlreadyApplied),
        }
    }

    /// Durably records a known or unknown remote outcome.
    pub fn record_remote_outcome(
        &mut self,
        outcome: MutationRemoteOutcome,
    ) -> Result<MutationRecordTransition> {
        let next_state = outcome.durable_state();
        if self.remote_outcome.as_ref() == Some(&outcome) {
            return Ok(MutationRecordTransition::AlreadyApplied);
        }
        if !matches!(
            self.state,
            MutationState::RemoteApplying | MutationState::RemoteOutcomeUnknown
        ) {
            return Err(CloudFilesCoreError::invalid_mutation_transition(
                "remote outcome must follow remote apply or reconcile an unknown outcome",
            ));
        }
        if self.state == MutationState::RemoteOutcomeUnknown
            && matches!(outcome, MutationRemoteOutcome::RemoteOutcomeUnknown)
        {
            return Ok(MutationRecordTransition::AlreadyApplied);
        }

        self.state = next_state;
        self.remote_outcome = Some(outcome);
        Ok(MutationRecordTransition::Applied)
    }

    /// Marks required platform effects as reconciled.
    pub fn mark_platform_reconciled(&mut self) -> Result<MutationRecordTransition> {
        match self.state {
            MutationState::RemoteOutcomeKnown => {
                self.state = MutationState::PlatformReconciled;
                Ok(MutationRecordTransition::Applied)
            }
            MutationState::PlatformReconciled | MutationState::Completed => {
                Ok(MutationRecordTransition::AlreadyApplied)
            }
            _ => Err(CloudFilesCoreError::invalid_mutation_transition(
                "platform reconciliation requires a known remote outcome",
            )),
        }
    }

    /// Marks the operation complete after platform reconciliation.
    pub fn complete(&mut self) -> Result<MutationRecordTransition> {
        match self.state {
            MutationState::PlatformReconciled => {
                self.state = MutationState::Completed;
                Ok(MutationRecordTransition::Applied)
            }
            MutationState::Completed => Ok(MutationRecordTransition::AlreadyApplied),
            _ => Err(CloudFilesCoreError::invalid_mutation_transition(
                "completion requires platform reconciliation",
            )),
        }
    }
}
