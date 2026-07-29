async fn run_mutation_worker(
    store: Arc<MemoryWritebackStore>,
    backend: Arc<MemoryRemoteMutationBackend>,
    scope: CloudScope,
    generation: SessionGeneration,
    mut shutdown: watch::Receiver<bool>,
) -> ExampleResult<()> {
    let runner = MutationRunner;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let records = store
            .recoverable_mutations_page(&scope, None, RECOVERY_BATCH_LIMIT)
            .await?
            .into_items();
        if records.is_empty() {
            tokio::select! {
                _ = store.mutation_notify.notified() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
            }
            continue;
        }

        let mut retry_later = false;
        for record in records {
            match runner
                .resume(
                    record.intent().operation_id(),
                    generation,
                    &*store,
                    &*backend,
                )
                .await
            {
                Ok(MutationRunOutcome::Completed) => {}
                Ok(MutationRunOutcome::RemoteOutcomePending) => retry_later = true,
                Ok(MutationRunOutcome::Fenced) => return Ok(()),
                Err(error) => {
                    eprintln!("synthetic mutation worker retry: {error}");
                    retry_later = true;
                }
            }
        }
        if retry_later {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn run_upload_worker(
    store: Arc<MemoryWritebackStore>,
    backend: Arc<MemoryRemoteMutationBackend>,
    scope: CloudScope,
    generation: SessionGeneration,
    mut shutdown: watch::Receiver<bool>,
) -> ExampleResult<()> {
    let runner = ContentUploadRunner::new(8)?;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let records = store
            .recoverable_content_uploads_page(&scope, None, RECOVERY_BATCH_LIMIT)
            .await?
            .into_items();
        if records.is_empty() {
            tokio::select! {
                _ = store.upload_notify.notified() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
            }
            continue;
        }

        let mut retry_later = false;
        for record in records {
            match runner
                .resume(
                    record.intent().operation_id(),
                    generation,
                    &*store,
                    &*backend,
                    &*store,
                )
                .await
            {
                Ok(ContentUploadRunOutcome::Completed) => {}
                Ok(ContentUploadRunOutcome::RemoteOutcomePending) => retry_later = true,
                Ok(ContentUploadRunOutcome::Fenced) => return Ok(()),
                Err(error) => {
                    eprintln!("synthetic upload worker retry: {error}");
                    retry_later = true;
                }
            }
        }
        if retry_later {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

pub fn run() -> ExampleResult<()> {
    let mut arguments = std::env::args_os().skip(1);
    let mountpoint = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/aster-forge-memory-cloud"));
    let state_directory = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        return Err("expected at most MOUNTPOINT and optional STATE_DIRECTORY".into());
    }
    std::fs::create_dir_all(&mountpoint)?;
    let metadata = std::fs::metadata(&mountpoint)?;
    let (backend, mut records) = MemoryCloud::fixture()?;
    let scope = backend.scope.clone();
    let root = records.remove(0);
    if root.inode() != LINUX_ROOT_INODE {
        return Err("memory fixture root did not use inode one".into());
    }
    let inode_table = Arc::new(LinuxInodeTable::new(root, records)?);
    let attributes = LinuxAttributePolicy::new(
        metadata.uid(),
        metadata.gid(),
        0o644,
        0o755,
        Duration::from_secs(1),
    )?;
    let store = Arc::new(MemoryWritebackStore::new(
        scope.clone(),
        state_directory.as_deref(),
    )?);
    store.reserve_fixture_inodes_through(6);
    let session_generation = store.next_mount_generation()?;
    let metadata_backend = Arc::new(backend);
    let mutation_backend = Arc::new(MemoryRemoteMutationBackend::new(
        store.clone(),
        state_directory.as_deref(),
    )?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(store.activate_session(&scope, session_generation))?;
    let readonly = LinuxReadOnlyEngine::new(metadata_backend, inode_table, attributes);
    let engine = runtime.block_on(LinuxWritableEngine::activate_with_namespace(
        readonly,
        store.clone(),
        store.clone(),
        session_generation,
    ))?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mutation_worker = runtime.spawn(run_mutation_worker(
        store.clone(),
        mutation_backend.clone(),
        scope.clone(),
        session_generation,
        shutdown_rx.clone(),
    ));
    let upload_worker = runtime.spawn(run_upload_worker(
        store.clone(),
        mutation_backend,
        scope.clone(),
        session_generation,
        shutdown_rx,
    ));
    let filesystem = LinuxWritableFilesystem::new(engine, runtime.handle().clone(), 64)?;
    let config = LinuxMountConfig::new("aster-forge-memory-cloud")?
        .with_subtype("aster_forge_cloud_files")?;

    println!("mounting writable memory cloud at {}", mountpoint.display());
    println!(
        "unmount from another terminal with: fusermount3 -u {}",
        mountpoint.display()
    );
    match state_directory.as_ref() {
        Some(path) => println!(
            "synthetic dirty generations persist across restarts in {}",
            path.display()
        ),
        None => println!("dirty generations exist only while this example process is running"),
    }
    println!(
        "active synthetic mount generation: {}",
        session_generation.get()
    );
    println!("existing and newly created files support write, truncate, flush, fsync, and reopen");
    println!("create, mkdir, rename, unlink, and rmdir use the durable synthetic mutation ledger");
    println!(
        "external remote changes are integrated by product code through the Linux remote overlay and notifier APIs"
    );
    if let Some(path) = state_directory.as_ref() {
        println!(
            "synthetic remote mutation outcomes persist in {}",
            path.join("remote.json").display()
        );
    }
    let mount_result = mount_writable(filesystem, &mountpoint, &config);
    runtime.block_on(store.transition_session(
        &scope,
        session_generation,
        SessionState::Closing,
    ))?;
    let _ = shutdown_tx.send(true);
    runtime.block_on(store.transition_session(
        &scope,
        session_generation,
        SessionState::Draining,
    ))?;
    runtime.block_on(mutation_worker)??;
    runtime.block_on(upload_worker)??;
    runtime.block_on(store.transition_session(&scope, session_generation, SessionState::Closed))?;
    mount_result?;
    Ok(())
}
