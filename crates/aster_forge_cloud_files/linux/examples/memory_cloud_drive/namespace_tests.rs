#[cfg(test)]
mod namespace_tests {
    use super::*;
    use aster_forge_cloud_files_linux::{
        LinuxInvalidation, LinuxRemoteChange, LinuxRemoteDelete, LinuxRemoteEntry,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    type MemoryEngine = LinuxWritableEngine<MemoryCloud, MemoryWritebackStore>;

    fn test_directory(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("aster-forge-cloud-namespace-{name}-{suffix}"))
    }

    async fn activate(
        state_directory: Option<&Path>,
        generation: SessionGeneration,
    ) -> (MemoryEngine, Arc<MemoryWritebackStore>, CloudScope) {
        let (backend, mut records) = MemoryCloud::fixture().unwrap();
        let scope = backend.scope.clone();
        let root = records.remove(0);
        let store = Arc::new(
            MemoryWritebackStore::new(scope.clone(), state_directory).unwrap(),
        );
        store.reserve_fixture_inodes_through(6);
        store.activate_session(&scope, generation).await.unwrap();
        let readonly = LinuxReadOnlyEngine::new(
            Arc::new(backend),
            Arc::new(LinuxInodeTable::new(root, records).unwrap()),
            LinuxAttributePolicy::new(1000, 1000, 0o644, 0o755, Duration::ZERO).unwrap(),
        );
        let engine = LinuxWritableEngine::activate_with_namespace(
            readonly,
            store.clone(),
            store.clone(),
            generation,
        )
        .await
        .unwrap();
        (engine, store, scope)
    }

    fn snapshot_names(engine: &MemoryEngine, directory: LinuxInode, handle: aster_forge_cloud_files_linux::LinuxDirectoryHandle) -> Vec<String> {
        engine
            .directory_snapshot(directory, handle)
            .unwrap()
            .entries()
            .iter()
            .map(|entry| entry.name().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn mkdir_nested_and_cross_parent_rename_preserve_inode_and_directory_snapshots() {
        let generation = SessionGeneration::new(1).unwrap();
        let (engine, _, _) = activate(None, generation).await;
        let before = engine.open_directory(LINUX_ROOT_INODE).await.unwrap();
        let alpha = engine
            .create_directory(LINUX_ROOT_INODE, "alpha", 0o755, 0)
            .await
            .unwrap();
        let alpha_inode = alpha.attributes().inode();
        let child = engine
            .create_directory(alpha_inode, "child", 0o755, 0)
            .await
            .unwrap();
        let child_inode = child.attributes().inode();
        let target = engine
            .create_directory(LINUX_ROOT_INODE, "target", 0o755, 0)
            .await
            .unwrap();

        engine
            .rename(LINUX_ROOT_INODE, "alpha", LINUX_ROOT_INODE, "beta", false)
            .await
            .unwrap();
        assert_eq!(
            engine
                .lookup(LINUX_ROOT_INODE, "beta")
                .await
                .unwrap()
                .attributes()
                .inode(),
            alpha_inode
        );
        assert_eq!(
            engine.lookup(LINUX_ROOT_INODE, "alpha").await.unwrap_err().error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotFound
        );
        assert_eq!(
            engine.lookup(alpha_inode, "child").await.unwrap().attributes().inode(),
            child_inode
        );
        engine
            .rename(
                alpha_inode,
                "child",
                target.attributes().inode(),
                "moved",
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            engine
                .lookup(target.attributes().inode(), "moved")
                .await
                .unwrap()
                .attributes()
                .inode(),
            child_inode
        );
        assert!(!snapshot_names(&engine, LINUX_ROOT_INODE, before)
            .iter()
            .any(|name| name == "alpha" || name == "beta" || name == "target"));
        let after = engine.open_directory(LINUX_ROOT_INODE).await.unwrap();
        let after_names = snapshot_names(&engine, LINUX_ROOT_INODE, after);
        assert!(after_names.iter().any(|name| name == "beta"));
        assert!(after_names.iter().any(|name| name == "target"));
    }

    #[tokio::test]
    async fn rename_replacement_no_replace_and_remove_kind_boundaries_are_exact() {
        let (engine, _, _) = activate(None, SessionGeneration::new(1).unwrap()).await;
        let hello = engine.lookup(LINUX_ROOT_INODE, "hello.txt").await.unwrap();
        let readme = engine.lookup(LINUX_ROOT_INODE, "readme.txt").await.unwrap();
        let docs = engine.lookup(LINUX_ROOT_INODE, "docs").await.unwrap();

        let error = engine
            .rename(
                LINUX_ROOT_INODE,
                "hello.txt",
                LINUX_ROOT_INODE,
                "readme.txt",
                true,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::AlreadyExists
        );
        engine
            .rename(
                LINUX_ROOT_INODE,
                "hello.txt",
                LINUX_ROOT_INODE,
                "readme.txt",
                false,
            )
            .await
            .unwrap();
        let replaced = engine.lookup(LINUX_ROOT_INODE, "readme.txt").await.unwrap();
        assert_eq!(replaced.key(), hello.key());
        assert_eq!(replaced.attributes().inode(), hello.attributes().inode());
        assert_ne!(replaced.key(), readme.key());

        let file_over_directory = engine
            .rename(
                LINUX_ROOT_INODE,
                "readme.txt",
                LINUX_ROOT_INODE,
                "docs",
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(
            file_over_directory.error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::IsDirectory
        );
        let directory_over_file = engine
            .rename(
                LINUX_ROOT_INODE,
                "docs",
                LINUX_ROOT_INODE,
                "readme.txt",
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(
            directory_over_file.error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotDirectory
        );
        assert_eq!(
            engine
                .remove(LINUX_ROOT_INODE, "docs", LinuxNodeKind::File)
                .await
                .unwrap_err()
                .error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::IsDirectory
        );
        assert_eq!(
            engine
                .remove(LINUX_ROOT_INODE, "readme.txt", LinuxNodeKind::Directory)
                .await
                .unwrap_err()
                .error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotDirectory
        );
        assert_eq!(
            engine
                .remove(LINUX_ROOT_INODE, "docs", LinuxNodeKind::Directory)
                .await
                .unwrap_err()
                .error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::DirectoryNotEmpty
        );
        assert_eq!(docs.attributes().kind(), LinuxNodeKind::Directory);
    }

    #[tokio::test]
    async fn unlink_and_empty_rmdir_hide_entries_without_mutating_old_snapshots() {
        let (engine, _, _) = activate(None, SessionGeneration::new(1).unwrap()).await;
        let directory = engine
            .create_directory(LINUX_ROOT_INODE, "empty", 0o755, 0)
            .await
            .unwrap();
        let (file, handle) = engine
            .create_file(
                LINUX_ROOT_INODE,
                "temporary.txt",
                0o644,
                0,
                aster_forge_cloud_files_linux::LinuxFileAccess::ReadWrite,
            )
            .await
            .unwrap();
        engine.release_file(handle).await.unwrap();
        let before = engine.open_directory(LINUX_ROOT_INODE).await.unwrap();
        engine
            .remove(LINUX_ROOT_INODE, "temporary.txt", LinuxNodeKind::File)
            .await
            .unwrap();
        engine
            .remove(LINUX_ROOT_INODE, "empty", LinuxNodeKind::Directory)
            .await
            .unwrap();
        let before_names = snapshot_names(&engine, LINUX_ROOT_INODE, before);
        assert!(before_names.iter().any(|name| name == "temporary.txt"));
        assert!(before_names.iter().any(|name| name == "empty"));
        assert_eq!(
            engine
                .lookup(LINUX_ROOT_INODE, "temporary.txt")
                .await
                .unwrap_err()
                .error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotFound
        );
        assert_eq!(
            engine.lookup(LINUX_ROOT_INODE, "empty").await.unwrap_err().error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotFound
        );
        assert_eq!(file.attributes().kind(), LinuxNodeKind::File);
        assert_eq!(directory.attributes().kind(), LinuxNodeKind::Directory);
    }

    #[tokio::test]
    async fn namespace_overlay_and_stable_inodes_survive_a_new_mount_generation() {
        let directory = test_directory("restart");
        fs::create_dir_all(&directory).unwrap();
        let generation_one = SessionGeneration::new(1).unwrap();
        let (engine, store, _) = activate(Some(&directory), generation_one).await;
        let parent = engine
            .create_directory(LINUX_ROOT_INODE, "persisted", 0o755, 0)
            .await
            .unwrap();
        let child = engine
            .create_directory(parent.attributes().inode(), "child", 0o755, 0)
            .await
            .unwrap();
        engine
            .rename(
                parent.attributes().inode(),
                "child",
                LINUX_ROOT_INODE,
                "restored-child",
                false,
            )
            .await
            .unwrap();
        let parent_inode = parent.attributes().inode();
        let child_inode = child.attributes().inode();
        drop(engine);
        drop(store);

        let (restarted, _, _) =
            activate(Some(&directory), SessionGeneration::new(2).unwrap()).await;
        assert_eq!(
            restarted
                .lookup(LINUX_ROOT_INODE, "persisted")
                .await
                .unwrap()
                .attributes()
                .inode(),
            parent_inode
        );
        assert_eq!(
            restarted
                .lookup(LINUX_ROOT_INODE, "restored-child")
                .await
                .unwrap()
                .attributes()
                .inode(),
            child_inode
        );
        assert_eq!(
            restarted
                .lookup(parent_inode, "child")
                .await
                .unwrap_err()
                .error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotFound
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn nested_create_upload_rename_and_reload_preserve_the_original_create_parent() {
        let directory = test_directory("nested-upload-reload");
        fs::create_dir_all(&directory).unwrap();
        let generation = SessionGeneration::new(1).unwrap();
        let (engine, store, scope) = activate(Some(&directory), generation).await;
        let parent = engine
            .create_directory(LINUX_ROOT_INODE, "parent", 0o755, 0)
            .await
            .unwrap();
        let (file, handle) = engine
            .create_file(
                parent.attributes().inode(),
                "nested.txt",
                0o644,
                0,
                aster_forge_cloud_files_linux::LinuxFileAccess::ReadWrite,
            )
            .await
            .unwrap();
        engine
            .write_file(
                file.attributes().inode(),
                handle,
                0,
                Bytes::from_static(b"nested-upload"),
            )
            .await
            .unwrap();
        engine.release_file(handle).await.unwrap();
        let backend = MemoryRemoteMutationBackend::new(store.clone(), Some(&directory)).unwrap();
        let create_operation = lock(&store.state)
            .created
            .get(file.key())
            .unwrap()
            .mutation_intent()
            .operation_id()
            .clone();
        MutationRunner
            .resume(&create_operation, generation, &*store, &backend)
            .await
            .unwrap();
        engine
            .rename(
                parent.attributes().inode(),
                "nested.txt",
                parent.attributes().inode(),
                "renamed.txt",
                false,
            )
            .await
            .unwrap();
        let upload_operation = lock(&store.state)
            .uploads
            .values()
            .find(|record| record.intent().snapshot().item_key() == file.key())
            .unwrap()
            .intent()
            .operation_id()
            .clone();
        ContentUploadRunner::new(8)
            .unwrap()
            .resume(&upload_operation, generation, &*store, &backend, &*store)
            .await
            .unwrap();
        drop(backend);
        drop(engine);
        drop(store);

        let reloaded = MemoryWritebackStore::new(scope, Some(&directory)).unwrap();
        {
            let state = lock(&reloaded.state);
            let created = state.created.get(file.key()).unwrap();
            assert_eq!(created.item().parent_id(), Some(parent.key().item_id()));
            assert!(matches!(
                created.mutation_intent().desired(),
                DesiredMutation::Create { parent_id, .. } if parent_id == parent.key().item_id()
            ));
            assert_eq!(
                state.namespace_items.get(file.key()).unwrap().item().name(),
                "renamed.txt"
            );
        }
        drop(reloaded);
        let (restarted, _, _) =
            activate(Some(&directory), SessionGeneration::new(2).unwrap()).await;
        let restored_parent = restarted
            .lookup(LINUX_ROOT_INODE, "parent")
            .await
            .unwrap();
        assert_eq!(restored_parent.attributes().inode(), parent.attributes().inode());
        assert_eq!(
            restarted
                .lookup(restored_parent.attributes().inode(), "renamed.txt")
                .await
                .unwrap()
                .attributes()
                .inode(),
            file.attributes().inode()
        );
        restarted
            .remove(
                restored_parent.attributes().inode(),
                "renamed.txt",
                LinuxNodeKind::File,
            )
            .await
            .unwrap();
        restarted
            .remove(LINUX_ROOT_INODE, "parent", LinuxNodeKind::Directory)
            .await
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn namespace_mutation_runner_reconciles_create_rename_and_delete_idempotently() {
        let directory = test_directory("runner");
        fs::create_dir_all(&directory).unwrap();
        let generation = SessionGeneration::new(1).unwrap();
        let (engine, store, _) = activate(Some(&directory), generation).await;
        let backend = MemoryRemoteMutationBackend::new(store.clone(), Some(&directory)).unwrap();
        let created = engine
            .create_directory(LINUX_ROOT_INODE, "remote-dir", 0o755, 0)
            .await
            .unwrap();
        let create_operation = lock(&store.state)
            .namespace_items
            .get(created.key())
            .unwrap()
            .mutation_intent()
            .operation_id()
            .clone();
        assert_eq!(
            MutationRunner
                .resume(&create_operation, generation, &*store, &backend)
                .await
                .unwrap(),
            MutationRunOutcome::Completed
        );
        assert_eq!(
            MutationRunner
                .resume(&create_operation, generation, &*store, &backend)
                .await
                .unwrap(),
            MutationRunOutcome::Completed
        );

        engine
            .rename(
                LINUX_ROOT_INODE,
                "remote-dir",
                LINUX_ROOT_INODE,
                "renamed-remote-dir",
                false,
            )
            .await
            .unwrap();
        let rename_operation = lock(&store.state)
            .namespace_items
            .get(created.key())
            .unwrap()
            .mutation_intent()
            .operation_id()
            .clone();
        assert_eq!(
            MutationRunner
                .resume(&rename_operation, generation, &*store, &backend)
                .await
                .unwrap(),
            MutationRunOutcome::Completed
        );
        assert_eq!(
            lock(&backend.state).items.get(created.key()).unwrap().name(),
            "renamed-remote-dir"
        );

        engine
            .remove(
                LINUX_ROOT_INODE,
                "renamed-remote-dir",
                LinuxNodeKind::Directory,
            )
            .await
            .unwrap();
        let delete_operation = lock(&store.state)
            .tombstones
            .get(&(CloudItemId::new("root").unwrap(), "renamed-remote-dir".to_owned()))
            .unwrap()
            .mutation_intent()
            .operation_id()
            .clone();
        assert_eq!(
            MutationRunner
                .resume(&delete_operation, generation, &*store, &backend)
                .await
                .unwrap(),
            MutationRunOutcome::Completed
        );
        assert!(!lock(&backend.state).items.contains_key(created.key()));
        drop(backend);
        let restarted_backend =
            MemoryRemoteMutationBackend::new(store, Some(&directory)).unwrap();
        assert!(!lock(&restarted_backend.state).items.contains_key(created.key()));
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn fenced_or_failed_namespace_transactions_publish_no_overlay() {
        let directory = test_directory("failure");
        fs::create_dir_all(&directory).unwrap();
        let generation = SessionGeneration::new(2).unwrap();
        let (engine, store, scope) = activate(Some(&directory), generation).await;
        let stale = LinuxCreateDirectoryRequest::new(
            key(&scope, "root").unwrap(),
            "stale",
            0o755,
            0,
            SessionGeneration::new(1).unwrap(),
        )
        .unwrap();
        assert_eq!(
            store.create_directory(&stale).await.unwrap_err().kind(),
            LinuxNamespaceMutationStoreErrorKind::Fenced
        );
        fs::remove_file(directory.join("writeback.json")).unwrap();
        fs::remove_dir(&directory).unwrap();
        fs::write(&directory, b"not-a-directory").unwrap();
        let error = engine
            .create_directory(LINUX_ROOT_INODE, "failed", 0o755, 0)
            .await
            .unwrap_err();
        assert_eq!(
            error.error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::Io
        );
        assert_eq!(
            engine.lookup(LINUX_ROOT_INODE, "failed").await.unwrap_err().error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotFound
        );
        assert!(lock(&store.state).namespace_items.is_empty());
        fs::remove_file(directory).unwrap();
    }

    #[tokio::test]
    async fn durable_remote_overlay_updates_namespace_and_emits_exact_invalidations() {
        let (engine, _, scope) = activate(None, SessionGeneration::new(1).unwrap()).await;
        let before = engine.open_directory(LINUX_ROOT_INODE).await.unwrap();
        let remote_key = key(&scope, "remote-new").unwrap();
        let remote_record = LinuxInodeRecord::new(
            remote_key.clone(),
            LinuxInode::new(100).unwrap(),
            LinuxInodeGeneration::new(7).unwrap(),
        );
        let remote_item = CloudItem::directory(
            remote_key.clone(),
            Some(CloudItemId::new("root").unwrap()),
            "remote-new",
            metadata_revision("remote-new-m1").unwrap(),
        )
        .unwrap();
        let create_plan = engine
            .apply_remote_change(LinuxRemoteChange::Upsert(
                aster_forge_cloud_files_linux::LinuxRemoteUpsert::new(
                    LinuxRemoteEntry::new(remote_item, remote_record.clone()),
                    None,
                    None,
                ),
            ))
            .unwrap();
        assert_eq!(
            create_plan,
            vec![
                LinuxInvalidation::Entry {
                    parent: LINUX_ROOT_INODE,
                    name: "remote-new".to_owned(),
                },
                LinuxInvalidation::Inode {
                    inode: remote_record.inode(),
                },
            ]
        );
        assert_eq!(
            engine
                .lookup(LINUX_ROOT_INODE, "remote-new")
                .await
                .unwrap()
                .attributes()
                .inode(),
            remote_record.inode()
        );
        assert!(!snapshot_names(&engine, LINUX_ROOT_INODE, before)
            .iter()
            .any(|name| name == "remote-new"));

        let docs = engine.lookup(LINUX_ROOT_INODE, "docs").await.unwrap();
        let moved_item = CloudItem::directory(
            remote_key.clone(),
            Some(docs.key().item_id().clone()),
            "moved-remote",
            metadata_revision("remote-new-m2").unwrap(),
        )
        .unwrap();
        let move_plan = engine
            .apply_remote_change(LinuxRemoteChange::Upsert(
                aster_forge_cloud_files_linux::LinuxRemoteUpsert::new(
                    LinuxRemoteEntry::new(moved_item, remote_record.clone()),
                    Some(
                        aster_forge_cloud_files_linux::LinuxRemoteLocation::new(
                            key(&scope, "root").unwrap(),
                            "remote-new",
                        )
                        .unwrap(),
                    ),
                    None,
                ),
            ))
            .unwrap();
        assert_eq!(
            move_plan,
            vec![
                LinuxInvalidation::Entry {
                    parent: LINUX_ROOT_INODE,
                    name: "remote-new".to_owned(),
                },
                LinuxInvalidation::Entry {
                    parent: docs.attributes().inode(),
                    name: "moved-remote".to_owned(),
                },
                LinuxInvalidation::Inode {
                    inode: remote_record.inode(),
                },
            ]
        );
        assert_eq!(
            engine
                .lookup(LINUX_ROOT_INODE, "remote-new")
                .await
                .unwrap_err()
                .error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotFound
        );
        assert_eq!(
            engine
                .lookup(docs.attributes().inode(), "moved-remote")
                .await
                .unwrap()
                .attributes()
                .inode(),
            remote_record.inode()
        );

        let invalid_record = LinuxInodeRecord::new(
            remote_key.clone(),
            LinuxInode::new(3).unwrap(),
            LinuxInodeGeneration::new(7).unwrap(),
        );
        let invalid_item = CloudItem::directory(
            remote_key.clone(),
            Some(docs.key().item_id().clone()),
            "invalid",
            metadata_revision("invalid-m").unwrap(),
        )
        .unwrap();
        assert!(
            engine
                .apply_remote_change(LinuxRemoteChange::Upsert(
                    aster_forge_cloud_files_linux::LinuxRemoteUpsert::new(
                        LinuxRemoteEntry::new(invalid_item, invalid_record),
                        None,
                        None,
                    ),
                ))
                .is_err()
        );
        assert_eq!(
            engine
                .lookup(docs.attributes().inode(), "moved-remote")
                .await
                .unwrap()
                .attributes()
                .inode(),
            remote_record.inode()
        );

        let delete = LinuxRemoteDelete::new(
            remote_key,
            remote_record.clone(),
            docs.key().clone(),
            "moved-remote",
            LinuxNodeKind::Directory,
        )
        .unwrap();
        assert_eq!(
            engine
                .apply_remote_change(LinuxRemoteChange::Delete(delete))
                .unwrap(),
            vec![LinuxInvalidation::Delete {
                parent: docs.attributes().inode(),
                child: remote_record.inode(),
                name: "moved-remote".to_owned(),
            }]
        );
        assert_eq!(
            engine
                .lookup(docs.attributes().inode(), "moved-remote")
                .await
                .unwrap_err()
                .error_code(),
            aster_forge_cloud_files_linux::LinuxErrorCode::NotFound
        );
    }
}
