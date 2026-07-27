use std::{collections::HashSet, sync::Mutex};

use aster_forge_cloud_files_core::{CloudFilesStoreError, CloudFilesStoreErrorKind, StoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailurePoint {
    BeforeMutationIntentPersist,
    AfterMutationIntentPersistBeforeReturn,
    AfterRemoteOutcomePersistBeforeReturn,
    AfterPlatformReconciledPersistBeforeReturn,
    AfterChangeBatchPersistBeforeReturn,
    AfterChangeEffectsReplayablePersistBeforeReturn,
    AfterCursorCommitPersistBeforeReturn,
    AfterContentEvictionIntentPersistBeforeReturn,
    AfterContentEvictionEffectPersistBeforeReturn,
    AfterContentEvictionMetadataPersistBeforeReturn,
    AfterContentEvictionCompletionPersistBeforeReturn,
    AfterContentCacheWriteIntentPersistBeforeReturn,
    AfterContentCacheWritePhysicalPersistBeforeReturn,
    AfterContentCacheWriteCoveragePersistBeforeReturn,
    AfterContentCacheWriteCompletionPersistBeforeReturn,
    AfterContentUploadIntentPersistBeforeReturn,
    AfterContentUploadCheckpointPersistBeforeReturn,
    AfterContentUploadRemoteCommitPersistBeforeReturn,
    AfterContentUploadOutcomePersistBeforeReturn,
    AfterContentUploadMetadataPersistBeforeReturn,
    AfterContentUploadCompletionPersistBeforeReturn,
}

#[derive(Debug, Default)]
pub struct FailureInjector {
    armed: Mutex<HashSet<FailurePoint>>,
}

impl FailureInjector {
    pub fn arm(&self, point: FailurePoint) {
        self.armed
            .lock()
            .expect("failure injector mutex should not be poisoned")
            .insert(point);
    }

    pub fn trip(&self, point: FailurePoint) -> StoreResult<()> {
        let should_fail = self
            .armed
            .lock()
            .expect("failure injector mutex should not be poisoned")
            .remove(&point);
        if should_fail {
            return Err(CloudFilesStoreError::new(
                CloudFilesStoreErrorKind::PersistenceFailure,
                format!("injected crash at {point:?}"),
            ));
        }
        Ok(())
    }
}
