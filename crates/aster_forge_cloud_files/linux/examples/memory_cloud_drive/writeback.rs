#[async_trait]
impl LinuxWritebackStore for MemoryWritebackStore {
    async fn activate_mount(
        &self,
        scope: &CloudScope,
        session_generation: SessionGeneration,
    ) -> StoreResult<Vec<LocalContentSnapshot>> {
        let mut state = lock(&self.state);
        if state
            .active_generation
            .is_some_and(|current| session_generation < current)
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "synthetic mount generation regressed",
            ));
        }
        state.active_generation = Some(session_generation);
        self.persist(&state)?;
        Ok(state
            .files
            .iter()
            .filter(|(key, file)| key.scope() == scope && file.snapshot.is_some())
            .filter_map(|(_, file)| file.snapshot.clone())
            .collect())
    }

    async fn open_recovered(
        &self,
        key: &CloudItemKey,
        session_generation: SessionGeneration,
    ) -> StoreResult<Option<LinuxWriteSession>> {
        let mut state = lock(&self.state);
        if state.active_generation != Some(session_generation) {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "recovered open used a fenced synthetic mount",
            ));
        }
        let Some(snapshot) = state.files.get(key).and_then(|file| file.snapshot.clone()) else {
            return Ok(None);
        };
        state.next_session = state.next_session.checked_add(1).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory write session exhausted",
            )
        })?;
        let session = LinuxWriteSessionId::new(format!("memory-session-{}", state.next_session))
            .map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory write session id was invalid",
                )
            })?;
        state.sessions.insert(
            session.clone(),
            WriteSessionBinding {
                key: key.clone(),
                generation: session_generation,
            },
        );
        Ok(Some(LinuxWriteSession::from_recovered(
            session,
            snapshot,
            session_generation,
        )))
    }

    async fn open_existing(
        &self,
        request: &LinuxWriteOpenRequest,
        base_content: Bytes,
    ) -> StoreResult<LinuxWriteSession> {
        let base_size = u64::try_from(base_content.len()).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory base content length exceeded u64",
            )
        })?;
        if base_size != request.base_size() {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory base content did not match metadata size",
            ));
        }
        let mut state = lock(&self.state);
        if state.active_generation != Some(request.session_generation())
            || state.active_session_state != Some(SessionState::Accepting)
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "fresh open used a fenced synthetic mount",
            ));
        }
        state.next_session = state.next_session.checked_add(1).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory write session exhausted",
            )
        })?;
        let session = LinuxWriteSessionId::new(format!("memory-session-{}", state.next_session))
            .map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory write session id was invalid",
                )
            })?;
        let file = state
            .files
            .entry(request.key().clone())
            .or_insert_with(|| StagedFile {
                bytes: Arc::from(base_content.as_ref()),
                snapshot: None,
                base_revision: request.base_revision().clone(),
            });
        let size = u64::try_from(file.bytes.len()).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory staged file length exceeded u64",
            )
        })?;
        state.sessions.insert(
            session.clone(),
            WriteSessionBinding {
                key: request.key().clone(),
                generation: request.session_generation(),
            },
        );
        Ok(LinuxWriteSession::new(
            session,
            request.key().clone(),
            size,
            request.session_generation(),
        ))
    }

    async fn read(
        &self,
        session: &LinuxWriteSessionId,
        offset: u64,
        size: u32,
    ) -> StoreResult<Bytes> {
        let state = lock(&self.state);
        let key = Self::session_key(&state, session)?;
        let file = state.files.get(&key).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "missing memory staged file",
            )
        })?;
        let start = usize::try_from(offset)
            .unwrap_or(file.bytes.len())
            .min(file.bytes.len());
        let end = start.saturating_add(size as usize).min(file.bytes.len());
        Ok(Bytes::copy_from_slice(&file.bytes[start..end]))
    }

    async fn write(
        &self,
        session: &LinuxWriteSessionId,
        offset: u64,
        bytes: Bytes,
    ) -> StoreResult<LinuxWriteCommit> {
        let mut state = lock(&self.state);
        Self::require_active_session(&state, session)?;
        let key = Self::session_key(&state, session)?;
        let file = state.files.get(&key).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "missing memory staged file",
            )
        })?;
        let start = usize::try_from(offset).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory write offset exceeded usize",
            )
        })?;
        let end = start.checked_add(bytes.len()).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory write end overflowed usize",
            )
        })?;
        let mut next_bytes = file.bytes.to_vec();
        if next_bytes.len() < end {
            next_bytes.resize(end, 0);
        }
        next_bytes[start..end].copy_from_slice(&bytes);
        let commit = self.commit(&mut state, session, next_bytes.into())?;
        self.upload_notify.notify_one();
        Ok(commit)
    }

    async fn truncate(
        &self,
        session: &LinuxWriteSessionId,
        size: u64,
    ) -> StoreResult<LinuxWriteCommit> {
        let mut state = lock(&self.state);
        Self::require_active_session(&state, session)?;
        let key = Self::session_key(&state, session)?;
        let file = state.files.get(&key).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "missing memory staged file",
            )
        })?;
        let size = usize::try_from(size).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory truncate size exceeded usize",
            )
        })?;
        let mut next_bytes = file.bytes.to_vec();
        next_bytes.resize(size, 0);
        let commit = self.commit(&mut state, session, next_bytes.into())?;
        self.upload_notify.notify_one();
        Ok(commit)
    }

    async fn sync(
        &self,
        session: &LinuxWriteSessionId,
        _data_only: bool,
    ) -> StoreResult<Option<LinuxWriteCommit>> {
        let state = lock(&self.state);
        Self::require_active_session(&state, session)?;
        let key = Self::session_key(&state, session)?;
        Ok(state
            .files
            .get(&key)
            .and_then(|file| file.snapshot.clone())
            .map(LinuxWriteCommit::new))
    }

    async fn close(&self, session: &LinuxWriteSessionId) -> StoreResult<()> {
        lock(&self.state).sessions.remove(session).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "missing memory write session",
            )
        })?;
        Ok(())
    }
}
