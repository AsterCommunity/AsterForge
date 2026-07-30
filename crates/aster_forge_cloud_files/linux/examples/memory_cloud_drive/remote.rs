#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct DurableRemoteDocument {
    committed: Vec<DurableRemoteCreate>,
    uploads: Vec<DurableRemoteUpload>,
    items: Option<Vec<DurableMutationFile>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableRemoteCreate {
    idempotency_key: String,
    item: Option<DurableMutationFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableRemoteUpload {
    idempotency_key: String,
    session_id: String,
    bytes: Vec<u8>,
    committed: Option<DurableRemoteContent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableRemoteContent {
    item: DurableMutationFile,
    bytes: Vec<u8>,
}

#[derive(Clone, Default)]
struct RemoteMutationState {
    committed: HashMap<IdempotencyKey, Option<CloudItem>>,
    items: HashMap<CloudItemKey, CloudItem>,
    uploads: HashMap<IdempotencyKey, RemoteUploadState>,
}

#[derive(Clone)]
struct RemoteUploadState {
    session_id: ContentUploadSessionId,
    bytes: Vec<u8>,
    committed: Option<(CloudItem, Bytes)>,
}

struct MemoryRemoteMutationBackend {
    namespace_store: Arc<MemoryWritebackStore>,
    state: Mutex<RemoteMutationState>,
    state_file: Option<PathBuf>,
    commit_pause: Duration,
    upload_commit_pause: Duration,
}

impl MemoryRemoteMutationBackend {
    fn new(
        namespace_store: Arc<MemoryWritebackStore>,
        state_directory: Option<&Path>,
    ) -> ExampleResult<Self> {
        let state_file = state_directory.map(|directory| directory.join("remote.json"));
        let mut state = RemoteMutationState::default();
        if let Some(path) = state_file.as_ref() {
            let document = match fs::read(path) {
                Ok(bytes) => serde_json::from_slice::<DurableRemoteDocument>(&bytes)?,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    DurableRemoteDocument::default()
                }
                Err(error) => return Err(Box::new(error)),
            };
            for entry in document.committed {
                let key = IdempotencyKey::new(entry.idempotency_key)?;
                if state
                    .committed
                    .insert(
                        key,
                        entry.item.map(DurableMutationFile::into_item).transpose()?,
                    )
                    .is_some()
                {
                    return Err(
                        "synthetic remote ledger contained duplicate idempotency keys".into(),
                    );
                }
            }
            for entry in document.uploads {
                let key = IdempotencyKey::new(entry.idempotency_key)?;
                let session_id = ContentUploadSessionId::new(entry.session_id)?;
                let committed = entry
                    .committed
                    .map(|content| -> ExampleResult<(CloudItem, Bytes)> {
                        Ok((content.item.into_item()?, Bytes::from(content.bytes)))
                    })
                    .transpose()?;
                if state
                    .uploads
                    .insert(
                        key,
                        RemoteUploadState {
                            session_id,
                            bytes: entry.bytes,
                            committed,
                        },
                    )
                    .is_some()
                {
                    return Err("synthetic remote ledger contained duplicate upload keys".into());
                }
            }
            match document.items {
                Some(items) => {
                    for item in items {
                        let item = item.into_item()?;
                        if state.items.insert(item.key().clone(), item).is_some() {
                            return Err(
                                "synthetic remote namespace contained duplicate items".into()
                            );
                        }
                    }
                }
                None => {
                    let (fixture, _) = MemoryCloud::fixture()?;
                    state.items = fixture.items;
                    for item in state.committed.values().flatten() {
                        state.items.insert(item.key().clone(), item.clone());
                    }
                }
            }
        }
        if state_file.is_none() {
            let (fixture, _) = MemoryCloud::fixture()?;
            state.items = fixture.items;
        }
        let pause_ms = std::env::var("ASTER_FORGE_CLOUD_FILES_EXAMPLE_REMOTE_COMMIT_PAUSE_MS")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()?
            .unwrap_or_default();
        let upload_pause_ms =
            std::env::var("ASTER_FORGE_CLOUD_FILES_EXAMPLE_UPLOAD_COMMIT_PAUSE_MS")
                .ok()
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or_default();
        Ok(Self {
            namespace_store,
            state: Mutex::new(state),
            state_file,
            commit_pause: Duration::from_millis(pause_ms),
            upload_commit_pause: Duration::from_millis(upload_pause_ms),
        })
    }

    fn persist(&self, state: &RemoteMutationState) -> ExampleResult<()> {
        let Some(state_file) = self.state_file.as_ref() else {
            return Ok(());
        };
        let mut committed = state
            .committed
            .iter()
            .map(|(idempotency_key, item)| {
                Ok(DurableRemoteCreate {
                    idempotency_key: idempotency_key.as_str().to_owned(),
                    item: item
                        .as_ref()
                        .map(DurableMutationFile::from_item)
                        .transpose()?,
                })
            })
            .collect::<StoreResult<Vec<_>>>()?;
        committed.sort_by(|left, right| left.idempotency_key.cmp(&right.idempotency_key));
        let mut uploads = state
            .uploads
            .iter()
            .map(|(idempotency_key, upload)| {
                let committed = upload
                    .committed
                    .as_ref()
                    .map(|(item, bytes)| {
                        Ok(DurableRemoteContent {
                            item: DurableMutationFile::from_item(item)?,
                            bytes: bytes.to_vec(),
                        })
                    })
                    .transpose()?;
                Ok(DurableRemoteUpload {
                    idempotency_key: idempotency_key.as_str().to_owned(),
                    session_id: upload.session_id.as_str().to_owned(),
                    bytes: upload.bytes.clone(),
                    committed,
                })
            })
            .collect::<StoreResult<Vec<_>>>()?;
        uploads.sort_by(|left, right| left.idempotency_key.cmp(&right.idempotency_key));
        let mut items = state
            .items
            .values()
            .map(DurableMutationFile::from_item)
            .collect::<StoreResult<Vec<_>>>()?;
        items.sort_by(|left, right| {
            (&left.namespace_id, &left.root_id, &left.item_id).cmp(&(
                &right.namespace_id,
                &right.root_id,
                &right.item_id,
            ))
        });
        let bytes = serde_json::to_vec_pretty(&DurableRemoteDocument {
            committed,
            uploads,
            items: Some(items),
        })?;
        let parent = state_file
            .parent()
            .ok_or("synthetic remote ledger path omitted its parent")?;
        fs::create_dir_all(parent)?;
        let temporary = state_file.with_extension("json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, state_file)?;
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
        Ok(())
    }

    fn mutation_outcome(
        &self,
        intent: &MutationIntent,
        state: &mut RemoteMutationState,
    ) -> BackendResult<Option<CloudItem>> {
        match intent.desired() {
            DesiredMutation::Create {
                scope,
                parent_id,
                name,
                kind,
            } => {
                let (local, size) = self
                    .namespace_store
                    .namespace_item_for_operation(intent.operation_id())
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
                if local.key().scope() != scope
                    || local.parent_id() != Some(parent_id)
                    || local.name() != name
                    || local.kind() != *kind
                {
                    return Err(CloudBackendError::new(
                        CloudBackendErrorKind::InvalidResponse,
                    ));
                }
                let metadata = MetadataRevision::from_slice(
                    format!("remote-create-m-{}", intent.operation_id().as_str()).as_bytes(),
                )
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
                let item = match kind {
                    CloudItemKind::File => CloudItem::file(
                        local.key().clone(),
                        parent_id.clone(),
                        name,
                        metadata,
                        CloudContentMetadata::new(
                            ContentRevision::from_slice(
                                format!("remote-create-c-{}", intent.operation_id().as_str())
                                    .as_bytes(),
                            )
                            .map_err(|_| {
                                CloudBackendError::new(CloudBackendErrorKind::InvalidResponse)
                            })?,
                            None,
                            size,
                        ),
                    ),
                    CloudItemKind::Directory => CloudItem::directory(
                        local.key().clone(),
                        Some(parent_id.clone()),
                        name,
                        metadata,
                    ),
                }
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
                state.items.insert(item.key().clone(), item.clone());
                Ok(Some(item))
            }
            DesiredMutation::ModifyMetadata {
                key,
                parent_id,
                name,
            } => {
                let current = state
                    .items
                    .get(key)
                    .cloned()
                    .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
                let parent_id = parent_id
                    .as_ref()
                    .or_else(|| current.parent_id())
                    .cloned()
                    .ok_or_else(|| {
                        CloudBackendError::new(CloudBackendErrorKind::InvalidResponse)
                    })?;
                let name = name.as_deref().unwrap_or_else(|| current.name());
                let metadata = MetadataRevision::from_slice(
                    format!("remote-rename-m-{}", intent.operation_id().as_str()).as_bytes(),
                )
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
                let item = match current.kind() {
                    CloudItemKind::File => CloudItem::file(
                        key.clone(),
                        parent_id.clone(),
                        name,
                        metadata,
                        current.content().cloned().ok_or_else(|| {
                            CloudBackendError::new(CloudBackendErrorKind::InvalidResponse)
                        })?,
                    ),
                    CloudItemKind::Directory => {
                        CloudItem::directory(key.clone(), Some(parent_id.clone()), name, metadata)
                    }
                }
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
                state.items.retain(|other_key, other| {
                    other_key == key
                        || other.parent_id() != Some(&parent_id)
                        || other.name() != name
                });
                state.items.insert(key.clone(), item.clone());
                Ok(Some(item))
            }
            DesiredMutation::Delete { key } => {
                if state.items.remove(key).is_none() {
                    return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
                }
                Ok(None)
            }
            DesiredMutation::ModifyContent { .. } => {
                Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported))
            }
        }
    }

    fn upload_item(&self, intent: &ContentUploadIntent) -> BackendResult<CloudItem> {
        let key = intent.base_cache_key().item_key();
        let (parent_id, name) = {
            let state = lock(&self.namespace_store.state);
            let (item, _) = MemoryWritebackStore::current_namespace_item(&state, key)
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::NotFound))?
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
            let parent_id = item
                .parent_id()
                .cloned()
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
            (parent_id, item.name().to_owned())
        };
        let content_revision = ContentRevision::from_slice(
            format!("remote-upload-c-{}", intent.operation_id().as_str()).as_bytes(),
        )
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        CloudItem::file(
            key.clone(),
            parent_id,
            name,
            MetadataRevision::from_slice(
                format!("remote-upload-m-{}", intent.operation_id().as_str()).as_bytes(),
            )
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?,
            CloudContentMetadata::new(content_revision, None, intent.snapshot().size()),
        )
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

#[async_trait]
impl CloudMutationBackend for MemoryRemoteMutationBackend {
    async fn apply_mutation(
        &self,
        intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let key = intent.idempotency_key().clone();
        if let Some(item) = lock(&self.state).committed.get(&key).cloned() {
            return Ok(MutationRemoteOutcome::AlreadyCommitted { item });
        }
        let item = {
            let mut state = lock(&self.state);
            if let Some(existing) = state.committed.get(&key).cloned() {
                return Ok(MutationRemoteOutcome::AlreadyCommitted { item: existing });
            }
            let mut next = state.clone();
            let item = self.mutation_outcome(intent, &mut next)?;
            next.committed.insert(key, item.clone());
            self.persist(&next).map_err(|_| {
                CloudBackendError::new(CloudBackendErrorKind::TemporarilyUnavailable)
            })?;
            *state = next;
            item
        };
        if !self.commit_pause.is_zero() {
            tokio::time::sleep(self.commit_pause).await;
        }
        Ok(MutationRemoteOutcome::Committed { item })
    }

    async fn reconcile_mutation(
        &self,
        intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        Ok(
            match lock(&self.state).committed.get(intent.idempotency_key()) {
                Some(item) => MutationRemoteOutcome::AlreadyCommitted { item: item.clone() },
                None => MutationRemoteOutcome::RemoteOutcomeUnknown,
            },
        )
    }
}

#[async_trait]
impl CloudContentUploadBackend for MemoryRemoteMutationBackend {
    async fn start_upload(
        &self,
        intent: &ContentUploadIntent,
    ) -> BackendResult<ContentUploadSession> {
        let key = intent.idempotency_key().clone();
        let mut state = lock(&self.state);
        if let Some(upload) = state.uploads.get(&key) {
            return Ok(ContentUploadSession::new(
                upload.session_id.clone(),
                u64::try_from(upload.bytes.len())
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?,
            ));
        }
        let session_id = ContentUploadSessionId::new(format!(
            "memory-upload-session-{}",
            intent.operation_id().as_str()
        ))
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        let mut next = state.clone();
        next.uploads.insert(
            key,
            RemoteUploadState {
                session_id: session_id.clone(),
                bytes: Vec::new(),
                committed: None,
            },
        );
        self.persist(&next)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::TemporarilyUnavailable))?;
        *state = next;
        Ok(ContentUploadSession::new(session_id, 0))
    }

    async fn upload_chunk(
        &self,
        intent: &ContentUploadIntent,
        chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        let key = intent.idempotency_key().clone();
        let mut state = lock(&self.state);
        let upload = state
            .uploads
            .get(&key)
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
        if upload.session_id != *chunk.session_id()
            || u64::try_from(upload.bytes.len())
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?
                != chunk.offset()
        {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        let mut next = state.clone();
        let upload = next
            .uploads
            .get_mut(&key)
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
        upload.bytes.extend_from_slice(chunk.bytes());
        let accepted_offset = u64::try_from(upload.bytes.len())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        self.persist(&next)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::TemporarilyUnavailable))?;
        *state = next;
        Ok(ContentUploadChunkAck::new(
            chunk.session_id().clone(),
            accepted_offset,
        ))
    }

    async fn reconcile_upload_chunk(
        &self,
        intent: &ContentUploadIntent,
        session: &ContentUploadSession,
        chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        chunk
            .validate_for(intent, session)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        let state = lock(&self.state);
        let upload = state
            .uploads
            .get(intent.idempotency_key())
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
        if upload.session_id != *session.id() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        let start = usize::try_from(chunk.offset())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        let end = usize::try_from(chunk.end_exclusive())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        if upload.bytes.get(start..end) != Some(chunk.bytes().as_ref()) {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        Ok(ContentUploadChunkAck::new(
            chunk.session_id().clone(),
            chunk.end_exclusive(),
        ))
    }

    async fn commit_upload(
        &self,
        intent: &ContentUploadIntent,
        session: &ContentUploadSession,
    ) -> BackendResult<MutationRemoteOutcome> {
        let key = intent.idempotency_key().clone();
        let item;
        {
            let mut state = lock(&self.state);
            let current = state
                .uploads
                .get(&key)
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
            if current.session_id != *session.id()
                || u64::try_from(current.bytes.len())
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?
                    != intent.snapshot().size()
            {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::PreconditionFailed,
                ));
            }
            if let Some((item, _)) = current.committed.clone() {
                return Ok(MutationRemoteOutcome::AlreadyCommitted { item: Some(item) });
            }
            item = self.upload_item(intent)?;
            let mut next = state.clone();
            let upload = next
                .uploads
                .get_mut(&key)
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
            upload.committed = Some((item.clone(), Bytes::from(upload.bytes.clone())));
            self.persist(&next).map_err(|_| {
                CloudBackendError::new(CloudBackendErrorKind::TemporarilyUnavailable)
            })?;
            *state = next;
        }
        if !self.upload_commit_pause.is_zero() {
            tokio::time::sleep(self.upload_commit_pause).await;
        }
        Ok(MutationRemoteOutcome::Committed { item: Some(item) })
    }

    async fn reconcile_upload(
        &self,
        intent: &ContentUploadIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let state = lock(&self.state);
        match state.uploads.get(intent.idempotency_key()) {
            Some(RemoteUploadState {
                committed: Some((item, _)),
                ..
            }) => Ok(MutationRemoteOutcome::AlreadyCommitted {
                item: Some(item.clone()),
            }),
            Some(_) => Ok(MutationRemoteOutcome::RemoteOutcomeUnknown),
            None => Err(CloudBackendError::new(CloudBackendErrorKind::NotFound)),
        }
    }
}
