#[cfg(not(windows))]
fn main() {
    eprintln!("memory_cloud_drive runs only on Windows 10 version 1709 or newer");
}

#[cfg(windows)]
mod windows_example {
    use std::{
        collections::HashMap,
        error::Error,
        path::PathBuf,
        sync::{Arc, Mutex, mpsc::RecvTimeoutError},
        time::Duration,
    };

    use aster_forge_cloud_files_core::{
        Alignment, BackendResult, CloudBackendError, CloudBackendErrorKind, CloudContentBackend,
        CloudContentMetadata, CloudItem, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId,
        CloudScope, ContentReadRange, ContentReadRequest, ContentReadResponse, ContentRevision,
        HydrationCoordinator, MetadataRevision, SessionGeneration,
    };
    use aster_forge_cloud_files_windows::{
        WindowsCallbackRequest, WindowsFetchDataFailure, WindowsFileIdentity, WindowsFileTimes,
        WindowsHardLinkPolicy, WindowsHydrationPolicy, WindowsHydrationPolicyPrimary,
        WindowsInSyncPolicy, WindowsObservedNotification, WindowsPlaceholder,
        WindowsPlaceholderManagementPolicy, WindowsPlaceholderOptions, WindowsPopulationPolicy,
        WindowsProviderId, WindowsSyncRootConnectOptions, WindowsSyncRootIdentity,
        WindowsSyncRootPolicies, WindowsSyncRootRegistration, WindowsSyncRootRegistrationOptions,
        connect_sync_root, create_placeholders, register_sync_root, unregister_sync_root,
    };
    use async_trait::async_trait;
    use bytes::Bytes;

    pub type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

    #[derive(Clone)]
    struct MemoryFile {
        revision: ContentRevision,
        bytes: Bytes,
    }

    struct MemoryCloud {
        files: Mutex<HashMap<CloudItemKey, MemoryFile>>,
        read_delay: Duration,
    }

    impl MemoryCloud {
        fn new(read_delay: Duration) -> Self {
            Self {
                files: Mutex::new(HashMap::new()),
                read_delay,
            }
        }

        fn insert(&self, key: CloudItemKey, revision: ContentRevision, bytes: Bytes) {
            lock(&self.files).insert(key, MemoryFile { revision, bytes });
        }

        fn revision(&self, key: &CloudItemKey) -> Option<ContentRevision> {
            lock(&self.files).get(key).map(|file| file.revision.clone())
        }

        fn remove(&self, key: &CloudItemKey) -> bool {
            lock(&self.files).remove(key).is_some()
        }
    }

    #[async_trait]
    impl CloudContentBackend for MemoryCloud {
        async fn read_content(
            &self,
            request: &ContentReadRequest,
        ) -> BackendResult<ContentReadResponse> {
            if !self.read_delay.is_zero() {
                tokio::time::sleep(self.read_delay).await;
            }
            let files = lock(&self.files);
            let file = files
                .get(request.key())
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
            if file.revision != *request.revision()
                || u64::try_from(file.bytes.len()).ok() != Some(request.expected_size())
            {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::PreconditionFailed,
                ));
            }
            let (offset, bytes) = match request.read_range() {
                ContentReadRange::Whole => (0, file.bytes.clone()),
                ContentReadRange::Range(range) => {
                    let start = usize::try_from(range.offset()).map_err(|_| {
                        CloudBackendError::new(CloudBackendErrorKind::InvalidRequest)
                    })?;
                    let end = usize::try_from(range.end_exclusive())
                        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?
                        .min(file.bytes.len());
                    if start > end {
                        return Err(CloudBackendError::new(
                            CloudBackendErrorKind::InvalidRequest,
                        ));
                    }
                    (range.offset(), file.bytes.slice(start..end))
                }
            };
            ContentReadResponse::new(
                file.revision.clone(),
                offset,
                bytes,
                request.expected_size(),
            )
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
        }
    }

    pub fn run() -> ExampleResult<()> {
        let (sync_root, unregister_only) = sync_root_from_args()?;
        if unregister_only {
            unregister_sync_root(&sync_root)?;
            println!("unregistered {}", sync_root.display());
            return Ok(());
        }
        std::fs::create_dir_all(&sync_root)?;

        let scope = CloudScope::new(
            CloudNamespaceId::new("aster-forge-memory-example")?,
            CloudRootId::new("default")?,
        );
        let root_id = CloudItemId::new("root")?;
        let root_key = CloudItemKey::new(scope.clone(), root_id.clone());
        let read_delay = duration_from_env("ASTER_FORGE_CLOUD_FILES_READ_DELAY_MS", 1)?;
        let smoke_duration = duration_from_env("ASTER_FORGE_CLOUD_FILES_SMOKE_SECONDS", 1_000)?;
        let disable_watchdog_poll =
            std::env::var_os("ASTER_FORGE_CLOUD_FILES_DISABLE_WATCHDOG_POLL").is_some();
        let cloud = Arc::new(MemoryCloud::new(read_delay.unwrap_or_default()));
        let items = memory_items(&scope, &root_id, &cloud)?;
        let registration = WindowsSyncRootRegistration::new(
            "AsterForge Memory Cloud",
            env!("CARGO_PKG_VERSION"),
            WindowsProviderId::new(0x2b7c_1d94_f321_4dc1_8b45_714b_15bd_38d2)?,
            WindowsSyncRootIdentity::encode(&scope)?,
            WindowsFileIdentity::encode(&root_key)?,
            WindowsSyncRootPolicies {
                hydration: WindowsHydrationPolicy {
                    primary: WindowsHydrationPolicyPrimary::Progressive,
                    validation_required: false,
                    streaming_allowed: false,
                    auto_dehydration_allowed: true,
                    allow_full_restart_hydration: true,
                },
                population: WindowsPopulationPolicy::AlwaysFull,
                in_sync: WindowsInSyncPolicy::FILE_LAST_WRITE_TIME,
                hard_links: WindowsHardLinkPolicy::Forbidden,
                placeholder_management: WindowsPlaceholderManagementPolicy::default(),
            },
            WindowsSyncRootRegistrationOptions {
                update_existing: true,
                disable_on_demand_population_on_root: true,
                mark_in_sync_on_root: true,
            },
        )?;
        let platform = register_sync_root(&sync_root, &registration)?;
        println!(
            "registered {} (CFAPI integration {:#x})",
            sync_root.display(),
            platform.integration
        );

        let placeholders = items
            .iter()
            .map(|item| {
                WindowsPlaceholder::from_item(
                    item,
                    WindowsFileIdentity::encode(item.key())?,
                    WindowsFileTimes::default(),
                    WindowsPlaceholderOptions {
                        mark_in_sync: true,
                        disable_on_demand_population: false,
                        always_full: false,
                        supersede: true,
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let outcome = create_placeholders(&sync_root, &placeholders)?;
        println!(
            "created/updated {} of {} in-memory placeholders",
            outcome.entries_processed,
            outcome.entries.len()
        );

        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        let generation = SessionGeneration::new(1)?;
        let mut connection = connect_sync_root(
            &sync_root,
            generation,
            sender,
            WindowsSyncRootConnectOptions {
                require_process_info: true,
                require_full_file_path: true,
                block_self_implicit_hydration: true,
            },
        )?;
        let waiters = connection.fetch_data_waiters();
        let coordinator = HydrationCoordinator::new(cloud.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        println!(
            "memory cloud drive is active; open the folder in Explorer and press Ctrl+C to stop"
        );
        println!(
            "after this process stops, unhydrated files report \
             ERROR_CLOUD_FILE_PROVIDER_TERMINATED (0x80070194) until the provider reconnects"
        );
        let smoke_deadline = smoke_duration.map(|duration| std::time::Instant::now() + duration);
        loop {
            if smoke_deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                println!("smoke duration elapsed; disconnecting cleanly");
                break;
            }
            match receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(WindowsCallbackRequest::FetchData(request)) => {
                    let key = request.snapshot().info().file_identity().decode()?;
                    println!("fetch data: {:?}", key);
                    let Some(revision) = cloud.revision(&key) else {
                        request.fail(WindowsFetchDataFailure::NotInSync)?;
                        continue;
                    };
                    if request.snapshot().is_recovery() {
                        println!("recovery hydration: {:?}", key);
                    }
                    runtime.block_on(request.hydrate(
                        &coordinator,
                        revision,
                        Alignment::ONE,
                        &waiters,
                    ))?;
                }
                Ok(WindowsCallbackRequest::CancelFetchData(request)) => {
                    println!("cancel observation: {:?}", request.snapshot().range());
                }
                Ok(WindowsCallbackRequest::Preflight(request)) => {
                    println!("approving durable preflight: {:?}", request.snapshot());
                    request.approve()?;
                }
                Ok(WindowsCallbackRequest::Observation { notification, .. }) => {
                    if let WindowsObservedNotification::DeleteCompleted { info, .. } = &notification
                    {
                        let key = info.file_identity().decode()?;
                        println!("delete completed: removed={} {:?}", cloud.remove(&key), key);
                    } else {
                        println!("platform observation: {notification:?}");
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if !disable_watchdog_poll {
                        let _ = waiters.poll_watchdog(std::time::Instant::now());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        connection.disconnect()?;
        Ok(())
    }

    fn memory_items(
        scope: &CloudScope,
        root_id: &CloudItemId,
        cloud: &MemoryCloud,
    ) -> ExampleResult<Vec<CloudItem>> {
        let fixtures = [
            (
                "hello.txt",
                "hello",
                Bytes::from_static(b"Hello from the AsterForge in-memory cloud drive.\r\n"),
            ),
            (
                "readme.txt",
                "readme",
                Bytes::from_static(
                    b"This placeholder is hydrated from process memory through Windows CFAPI.\r\n",
                ),
            ),
            (
                "numbers.bin",
                "numbers",
                Bytes::from((0u8..=255).cycle().take(16 * 1024).collect::<Vec<_>>()),
            ),
        ];
        fixtures
            .into_iter()
            .enumerate()
            .map(|(index, (name, id, bytes))| {
                let key = CloudItemKey::new(scope.clone(), CloudItemId::new(id)?);
                let revision =
                    ContentRevision::from_slice(format!("content-v{}", index + 1).as_bytes())?;
                let item = CloudItem::file(
                    key.clone(),
                    root_id.clone(),
                    name,
                    MetadataRevision::from_slice(format!("metadata-v{}", index + 1).as_bytes())?,
                    CloudContentMetadata::new(revision.clone(), None, u64::try_from(bytes.len())?),
                )?;
                cloud.insert(key, revision, bytes);
                Ok(item)
            })
            .collect()
    }

    fn sync_root_from_args() -> ExampleResult<(PathBuf, bool)> {
        let mut arguments = std::env::args_os().skip(1);
        let first = arguments.next();
        let unregister_only = first.as_deref() == Some(std::ffi::OsStr::new("--unregister"));
        let path = if unregister_only {
            arguments.next().map(PathBuf::from)
        } else {
            first.map(PathBuf::from)
        }
        .unwrap_or_else(|| PathBuf::from(r"C:\AsterForgeMemoryCloud"));
        if !path.is_absolute() {
            return Err("sync-root path must be absolute".into());
        }
        Ok((path, unregister_only))
    }

    fn duration_from_env(name: &str, unit_millis: u64) -> ExampleResult<Option<Duration>> {
        let Some(raw) = std::env::var_os(name) else {
            return Ok(None);
        };
        let raw = raw
            .into_string()
            .map_err(|_| format!("{name} must contain UTF-8 decimal digits"))?;
        let value = raw
            .parse::<u64>()
            .map_err(|_| format!("{name} must be an unsigned integer"))?;
        let millis = value
            .checked_mul(unit_millis)
            .ok_or_else(|| format!("{name} exceeds the supported duration"))?;
        Ok(Some(Duration::from_millis(millis)))
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(windows)]
fn main() -> windows_example::ExampleResult<()> {
    windows_example::run()
}
