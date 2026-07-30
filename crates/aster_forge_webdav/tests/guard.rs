use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavConditionalEvaluationError, DavConditionalOutcome,
    DavConditionalResource, DavFileSystem, DavIfEvaluationError, DavLock, DavLockError,
    DavLockPreflightError, DavLockSystem, DavMetaData, DavMethod, DavPath, DavResponseBody,
    DavWriteHandle, DavWriteOptions, DavWriteSystem, FsError, FsFuture, LsFuture,
    enforce_if_header_with_backends, enforce_parent_collection, enforce_parent_unlocked,
    enforce_unlocked, ensure_lock_target_exists, parse_if_header, plan_conditionals_with_backends,
    unsubmitted_lock_conflicts,
};
use bytes::Bytes;
use http::StatusCode;
use http::header::{HeaderMap, HeaderValue};

#[derive(Clone)]
struct TestMeta {
    etag: Option<String>,
    is_dir: bool,
}

impl DavMetaData for TestMeta {
    fn len(&self) -> u64 {
        0
    }

    fn modified(&self) -> Result<SystemTime, FsError> {
        Ok(SystemTime::UNIX_EPOCH)
    }

    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn etag(&self) -> Option<String> {
        self.etag.clone()
    }

    fn created(&self) -> Result<SystemTime, FsError> {
        Ok(SystemTime::UNIX_EPOCH)
    }
}

#[derive(Default)]
struct TestFileSystem {
    etags: HashMap<String, String>,
    directories: HashSet<String>,
    files: Mutex<HashMap<String, Bytes>>,
    concurrent_creations: Mutex<HashMap<String, Bytes>>,
    failures: HashMap<String, FsError>,
    open_failure: Option<FsError>,
    finish_failure: Option<FsError>,
    open_calls: Mutex<Vec<(String, DavWriteOptions)>>,
    finish_calls: Arc<AtomicUsize>,
}

struct TestWriteHandle {
    finish_failure: Option<FsError>,
    finish_calls: Arc<AtomicUsize>,
}

impl DavWriteHandle for TestWriteHandle {
    async fn write_bytes(&mut self, _buf: Bytes) -> Result<(), DavBackendError> {
        Err(DavBackendError::from(FsError::GeneralFailure))
    }

    async fn finish(self) -> Result<(), DavBackendError> {
        self.finish_calls.fetch_add(1, Ordering::SeqCst);
        self.finish_failure
            .map_or(Ok(()), |error| Err(DavBackendError::from(error)))
    }

    async fn abort(self) -> Result<(), DavBackendError> {
        Ok(())
    }
}

impl DavWriteSystem for TestFileSystem {
    type Handle = TestWriteHandle;

    async fn open_write<'a>(
        &'a self,
        path: &'a DavPath,
        options: DavWriteOptions,
    ) -> Result<Self::Handle, DavBackendError> {
        let create_new = options.create_new;
        let truncate = options.truncate;
        let create = options.create;
        self.open_calls
            .lock()
            .expect("open call log should not be poisoned")
            .push((path.as_str().to_owned(), options));
        if let Some(error) = self.open_failure {
            return Err(DavBackendError::from(error));
        }
        let concurrent_creation = self
            .concurrent_creations
            .lock()
            .expect("concurrent creation state should not be poisoned")
            .remove(path.as_str());
        let mut files = self
            .files
            .lock()
            .expect("file state should not be poisoned");
        if let Some(contents) = concurrent_creation {
            files.insert(path.as_str().to_owned(), contents);
        }
        if create_new
            && (self.etags.contains_key(path.as_str()) || files.contains_key(path.as_str()))
        {
            return Err(DavBackendError::from(FsError::Exists));
        }
        if truncate {
            files.insert(path.as_str().to_owned(), Bytes::new());
        } else if create {
            files.entry(path.as_str().to_owned()).or_default();
        }
        let file = TestWriteHandle {
            finish_failure: self.finish_failure,
            finish_calls: Arc::clone(&self.finish_calls),
        };
        Ok(file)
    }
}

impl DavFileSystem for TestFileSystem {
    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        Box::pin(async move {
            if let Some(error) = self.failures.get(path.as_str()) {
                return Err(*error);
            }
            let is_dir = self.directories.contains(path.as_str());
            let etag = self.etags.get(path.as_str()).cloned();
            let exists = etag.is_some()
                || self
                    .files
                    .lock()
                    .expect("file state should not be poisoned")
                    .contains_key(path.as_str());
            if !is_dir && !exists {
                return Err(FsError::NotFound);
            }
            Ok(Box::new(TestMeta { etag, is_dir }) as Box<dyn DavMetaData>)
        })
    }

    fn create_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::GeneralFailure) })
    }

    fn remove_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::GeneralFailure) })
    }

    fn remove_file<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::GeneralFailure) })
    }

    fn rename<'a>(&'a self, _from: &'a DavPath, _to: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::GeneralFailure) })
    }

    fn copy<'a>(&'a self, _from: &'a DavPath, _to: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::GeneralFailure) })
    }
}

#[derive(Default)]
struct TestLockSystem {
    discovered: Vec<DavLock>,
    conflicts: Vec<DavLock>,
    conflict_calls: Mutex<Vec<(String, bool)>>,
}

impl DavLockSystem for TestLockSystem {
    fn prepare_lock(&self, _path: &DavPath) -> LsFuture<'_, Result<(), DavLockPreflightError>> {
        Box::pin(async { Ok(()) })
    }

    fn lock(
        &self,
        _path: &DavPath,
        _principal: Option<&str>,
        _owner: Option<&aster_forge_webdav::DavXmlElement>,
        _timeout: Option<Duration>,
        _shared: bool,
        _deep: bool,
    ) -> LsFuture<'_, Result<DavLock, DavLockError>> {
        Box::pin(async { Err(DavLockError::Backend) })
    }

    fn unlock(&self, _path: &DavPath, _token: &str) -> LsFuture<'_, Result<(), DavLockError>> {
        Box::pin(async { Err(DavLockError::TokenMismatch) })
    }

    fn refresh(
        &self,
        _path: &DavPath,
        _token: &str,
        _timeout: Option<Duration>,
    ) -> LsFuture<'_, Result<DavLock, DavLockError>> {
        Box::pin(async { Err(DavLockError::TokenMismatch) })
    }

    fn check(
        &self,
        _path: &DavPath,
        _principal: Option<&str>,
        _ignore_principal: bool,
        _deep: bool,
        _submitted_tokens: &[String],
    ) -> LsFuture<'_, Result<(), DavLock>> {
        Box::pin(async { Ok(()) })
    }

    fn discover(&self, path: &DavPath) -> LsFuture<'_, Vec<DavLock>> {
        let discovered = self
            .discovered
            .iter()
            .filter(|lock| lock.path.as_str() == path.as_str())
            .cloned()
            .collect();
        Box::pin(async move { discovered })
    }

    fn conflicting_locks(&self, path: &DavPath, deep: bool) -> LsFuture<'_, Vec<DavLock>> {
        self.conflict_calls
            .lock()
            .expect("conflict call log should not be poisoned")
            .push((path.as_str().to_owned(), deep));
        let conflicts = self.conflicts.clone();
        Box::pin(async move { conflicts })
    }

    fn delete(&self, _path: &DavPath) -> LsFuture<'_, Result<(), DavLockError>> {
        Box::pin(async { Ok(()) })
    }
}

fn lock(path: &str, token: &str) -> DavLock {
    DavLock {
        token: token.to_owned(),
        path: Box::new(DavPath::new(path).expect("test lock path should be valid")),
        principal: None,
        owner: None,
        timeout_at: None,
        timeout: Some(Duration::from_mins(1)),
        shared: false,
        deep: true,
    }
}

#[test]
fn backend_conditional_entrypoint_covers_success_backend_and_representation_failures() {
    let path = DavPath::new("/file.txt").expect("path");
    let mut if_headers = HeaderMap::new();
    if_headers.insert("If", HeaderValue::from_static("([\"v1\"])"));
    let if_header = parse_if_header(&if_headers)
        .expect("If grammar")
        .expect("If header");
    let mut filesystem = TestFileSystem::default();
    filesystem
        .etags
        .insert(path.as_str().to_owned(), "v1".to_owned());
    let locks = TestLockSystem::default();
    let resource = DavConditionalResource {
        exists: true,
        etag: Some("v1"),
        last_modified: Some(SystemTime::UNIX_EPOCH),
    };

    let plan = futures::executor::block_on(plan_conditionals_with_backends(
        Some(&if_header),
        &filesystem,
        &locks,
        &path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Put,
        &HeaderMap::new(),
        resource,
    ))
    .expect("backend conditional plan");
    assert_eq!(plan.outcome, DavConditionalOutcome::Proceed);

    filesystem
        .failures
        .insert(path.as_str().to_owned(), FsError::GeneralFailure);
    let error = futures::executor::block_on(plan_conditionals_with_backends(
        Some(&if_header),
        &filesystem,
        &locks,
        &path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Put,
        &HeaderMap::new(),
        resource,
    ))
    .expect_err("filesystem error should cross the backend boundary");
    assert!(matches!(
        error,
        DavConditionalEvaluationError::Backend(error)
            if error.kind == DavBackendErrorKind::Internal
    ));

    let plan = futures::executor::block_on(plan_conditionals_with_backends(
        None,
        &filesystem,
        &locks,
        &path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Get,
        &HeaderMap::new(),
        DavConditionalResource {
            exists: true,
            etag: None,
            last_modified: Some(SystemTime::UNIX_EPOCH - Duration::from_secs(1)),
        },
    ))
    .expect("unconditional planning should skip unrepresentable metadata");
    let mut response_headers = HeaderMap::new();
    plan.apply_response_headers(http::StatusCode::OK, &mut response_headers);
    assert!(response_headers.is_empty());

    let mut conditional_headers = HeaderMap::new();
    conditional_headers.insert(
        "If-Modified-Since",
        HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
    );
    let error = futures::executor::block_on(plan_conditionals_with_backends(
        None,
        &filesystem,
        &locks,
        &path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Get,
        &conditional_headers,
        DavConditionalResource {
            exists: true,
            etag: None,
            last_modified: Some(SystemTime::UNIX_EPOCH - Duration::from_secs(1)),
        },
    ))
    .expect_err("an active date condition requires representable metadata");
    assert!(matches!(
        error,
        DavConditionalEvaluationError::InvalidRepresentation
    ));
}

fn if_header(value: &'static str) -> aster_forge_webdav::IfHeader {
    let mut headers = HeaderMap::new();
    headers.insert("If", HeaderValue::from_static(value));
    parse_if_header(&headers)
        .expect("test If header should parse")
        .expect("test If header should exist")
}

#[test]
fn backend_if_resolver_combines_filesystem_etag_and_lock_tokens() {
    futures::executor::block_on(async {
        let path = DavPath::new("/a.txt").expect("test path");
        let filesystem = TestFileSystem {
            etags: HashMap::from([("/a.txt".to_owned(), "etag-a".to_owned())]),
            ..TestFileSystem::default()
        };
        let lock_system = TestLockSystem {
            discovered: vec![lock("/a.txt", "urn:uuid:lock-a")],
            ..TestLockSystem::default()
        };
        let header = if_header("(<urn:uuid:lock-a> [\"etag-a\"])");

        enforce_if_header_with_backends(
            Some(&header),
            &filesystem,
            &lock_system,
            &path,
            "/webdav",
            "https",
            "dav.example",
        )
        .await
        .expect("matching filesystem and lock state should satisfy If");
    });
}

#[test]
fn backend_if_resolver_preserves_failures_and_treats_missing_as_empty_state() {
    futures::executor::block_on(async {
        let path = DavPath::new("/a.txt").expect("test path");
        let failing = TestFileSystem {
            failures: HashMap::from([("/a.txt".to_owned(), FsError::Forbidden)]),
            ..TestFileSystem::default()
        };
        let error = enforce_if_header_with_backends(
            Some(&if_header("(Not [\"etag-a\"])")),
            &failing,
            &TestLockSystem::default(),
            &path,
            "/webdav",
            "https",
            "dav.example",
        )
        .await
        .expect_err("filesystem failure should remain a backend failure");
        assert!(matches!(
            error,
            DavIfEvaluationError::Backend(error) if error.kind == DavBackendErrorKind::Forbidden
        ));

        enforce_if_header_with_backends(
            Some(&if_header("(Not [\"etag-a\"])")),
            &TestFileSystem::default(),
            &TestLockSystem::default(),
            &path,
            "/webdav",
            "https",
            "dav.example",
        )
        .await
        .expect("a missing resource should contribute empty state");
    });
}

#[test]
fn lock_guard_requires_a_token_scoped_to_the_conflicting_lock_root() {
    futures::executor::block_on(async {
        let target = DavPath::new("/locked/child.txt").expect("test path");
        let lock_system = TestLockSystem {
            conflicts: vec![lock("/locked/", "urn:uuid:root-lock")],
            ..TestLockSystem::default()
        };

        let response = enforce_unlocked(
            &lock_system,
            &target,
            true,
            "/webdav",
            Some(&if_header("</webdav/other> (<urn:uuid:root-lock>)")),
            "https",
            "dav.example",
        )
        .await
        .expect_err("a token tagged for another resource must not unlock this target");
        assert_eq!(response.status, StatusCode::LOCKED);
        assert!(matches!(response.body, DavResponseBody::Bytes(_)));

        let accepted = enforce_unlocked(
            &lock_system,
            &target,
            true,
            "/webdav",
            Some(&if_header("</webdav/locked/> (<urn:uuid:root-lock>)")),
            "https",
            "dav.example",
        )
        .await;
        assert!(
            accepted.is_ok(),
            "a token tagged for the lock root should unlock the target"
        );

        assert_eq!(
            lock_system
                .conflict_calls
                .lock()
                .expect("conflict call log should not be poisoned")
                .as_slice(),
            [
                ("/locked/child.txt".to_owned(), true),
                ("/locked/child.txt".to_owned(), true),
            ]
        );
    });
}

#[test]
fn parent_lock_guard_checks_the_canonical_parent_and_skips_mount_root() {
    futures::executor::block_on(async {
        let target = DavPath::new("/folder/new.txt").expect("test path");
        let lock_system = TestLockSystem {
            conflicts: vec![lock("/folder/", "urn:uuid:parent-lock")],
            ..TestLockSystem::default()
        };

        let accepted = enforce_parent_unlocked(
            &lock_system,
            &target,
            "/webdav",
            Some(&if_header("</webdav/folder/> (<urn:uuid:parent-lock>)")),
            "https",
            "dav.example",
        )
        .await;
        assert!(
            accepted.is_ok(),
            "the parent-scoped token should allow the mutation"
        );
        assert_eq!(
            lock_system
                .conflict_calls
                .lock()
                .expect("conflict call log should not be poisoned")
                .as_slice(),
            [("/folder/".to_owned(), false)]
        );

        let root_locks = TestLockSystem::default();
        let root_result = enforce_parent_unlocked(
            &root_locks,
            &DavPath::new("/").expect("mount root"),
            "/webdav",
            None,
            "https",
            "dav.example",
        )
        .await;
        assert!(root_result.is_ok(), "the mount root has no parent to check");
        assert!(
            root_locks
                .conflict_calls
                .lock()
                .expect("conflict call log should not be poisoned")
                .is_empty()
        );
    });
}

#[test]
fn conflict_filter_removes_only_tokens_submitted_for_each_lock_root() {
    futures::executor::block_on(async {
        let target = DavPath::new("/tree/").expect("test path");
        let lock_system = TestLockSystem {
            conflicts: vec![
                lock("/tree/a.txt", "urn:uuid:a"),
                lock("/tree/b.txt", "urn:uuid:b"),
                lock("/tree/c.txt", "urn:uuid:c"),
            ],
            ..TestLockSystem::default()
        };
        let header = if_header(
            "</webdav/tree/a.txt> (<urn:uuid:a>) </webdav/tree/b.txt> (<urn:uuid:wrong>) </webdav/other> (<urn:uuid:c>)",
        );

        let conflicts = unsubmitted_lock_conflicts(
            &lock_system,
            &target,
            true,
            "/webdav",
            Some(&header),
            "https",
            "dav.example",
        )
        .await;
        assert_eq!(
            conflicts
                .iter()
                .map(|lock| lock.path.as_str())
                .collect::<Vec<_>>(),
            ["/tree/b.txt", "/tree/c.txt"]
        );
    });
}

#[test]
fn parent_collection_guard_covers_root_collection_missing_and_backend_boundaries() {
    futures::executor::block_on(async {
        let filesystem = TestFileSystem {
            directories: HashSet::from(["/folder/".to_owned()]),
            etags: HashMap::from([("/file/".to_owned(), "etag-file".to_owned())]),
            failures: HashMap::from([("/private/".to_owned(), FsError::Forbidden)]),
            ..TestFileSystem::default()
        };

        let root_error =
            enforce_parent_collection(&filesystem, &DavPath::new("/").expect("mount root"))
                .await
                .expect_err("the mount root cannot be created below itself");
        assert_eq!(root_error.status, StatusCode::METHOD_NOT_ALLOWED);

        assert!(
            enforce_parent_collection(&filesystem, &DavPath::new("/new.txt").expect("root child"))
                .await
                .is_ok(),
            "the implicit mount root is a valid parent"
        );
        assert!(
            enforce_parent_collection(
                &filesystem,
                &DavPath::new("/folder/new.txt").expect("nested child"),
            )
            .await
            .is_ok(),
            "an existing collection is a valid parent"
        );

        for target in ["/file/new.txt", "/missing/new.txt"] {
            let response = enforce_parent_collection(
                &filesystem,
                &DavPath::new(target).expect("mutation target"),
            )
            .await
            .expect_err("a file or missing resource cannot be a collection parent");
            assert_eq!(response.status, StatusCode::CONFLICT);
        }

        let response = enforce_parent_collection(
            &filesystem,
            &DavPath::new("/private/new.txt").expect("forbidden parent"),
        )
        .await
        .expect_err("backend errors must retain their protocol classification");
        assert_eq!(response.status, StatusCode::FORBIDDEN);
    });
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "The test keeps the existing, missing, collection, and backend-failure lock targets in one matrix."
)]
fn lock_target_guard_creates_only_missing_files_and_preserves_failures() {
    futures::executor::block_on(async {
        let existing = TestFileSystem {
            etags: HashMap::from([("/existing.txt".to_owned(), "etag".to_owned())]),
            ..TestFileSystem::default()
        };
        assert!(
            ensure_lock_target_exists(
                &existing,
                &existing,
                &DavPath::new("/existing.txt").expect("existing file"),
            )
            .await
            .expect("existing target lookup")
        );
        assert!(
            existing
                .open_calls
                .lock()
                .expect("open call log should not be poisoned")
                .is_empty()
        );

        let missing = TestFileSystem::default();
        assert!(
            !ensure_lock_target_exists(
                &missing,
                &missing,
                &DavPath::new("/new.txt").expect("missing file"),
            )
            .await
            .expect("a missing file should become a lock-null resource")
        );
        assert_eq!(missing.finish_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            missing
                .open_calls
                .lock()
                .expect("open call log should not be poisoned")
                .as_slice(),
            [(
                "/new.txt".to_owned(),
                DavWriteOptions {
                    create: true,
                    create_new: true,
                    truncate: true,
                    expected_length: Some(0),
                    ..DavWriteOptions::default()
                }
            )]
        );

        let missing_collection = TestFileSystem::default();
        assert_eq!(
            ensure_lock_target_exists(
                &missing_collection,
                &missing_collection,
                &DavPath::new("/new/").expect("missing collection"),
            )
            .await,
            Err(DavBackendError::from(FsError::NotFound))
        );
        assert!(
            missing_collection
                .open_calls
                .lock()
                .expect("open call log should not be poisoned")
                .is_empty()
        );

        for (filesystem, expected) in [
            (
                TestFileSystem {
                    failures: HashMap::from([("/failed.txt".to_owned(), FsError::Forbidden)]),
                    ..TestFileSystem::default()
                },
                FsError::Forbidden,
            ),
            (
                TestFileSystem {
                    open_failure: Some(FsError::InsufficientStorage),
                    ..TestFileSystem::default()
                },
                FsError::InsufficientStorage,
            ),
            (
                TestFileSystem {
                    finish_failure: Some(FsError::GeneralFailure),
                    ..TestFileSystem::default()
                },
                FsError::GeneralFailure,
            ),
        ] {
            let result = ensure_lock_target_exists(
                &filesystem,
                &filesystem,
                &DavPath::new("/failed.txt").expect("failed target"),
            )
            .await;
            assert_eq!(
                result.expect_err("lock target failure"),
                DavBackendError::from(expected)
            );
        }
    });
}

#[test]
fn lock_target_guard_does_not_truncate_a_concurrent_put_creation() {
    futures::executor::block_on(async {
        let filesystem = TestFileSystem::default();
        let path = DavPath::new("/raced.txt").expect("race target path");
        filesystem
            .concurrent_creations
            .lock()
            .expect("concurrent creation state should not be poisoned")
            .insert(
                path.as_str().to_owned(),
                Bytes::from_static(b"concurrent PUT"),
            );

        assert!(matches!(
            filesystem.metadata(&path).await,
            Err(FsError::NotFound)
        ));
        let error = ensure_lock_target_exists(&filesystem, &filesystem, &path)
            .await
            .expect_err("create_new must reject a target created after metadata");
        assert_eq!(error.kind, DavBackendErrorKind::AlreadyExists);
        assert_eq!(filesystem.finish_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            filesystem
                .files
                .lock()
                .expect("file state should not be poisoned")
                .get(path.as_str()),
            Some(&Bytes::from_static(b"concurrent PUT"))
        );
    });
}
