//! Cross-process suite fixture metadata.
//!
//! Containers and per-test databases are short-lived mechanics. A migrated template database or
//! schema snapshot is different: it is a suite-scoped product fixture that can be consumed by
//! many nextest processes after its producer exits. This module owns only the locked, atomic
//! metadata publication protocol; products own fixture names, contents, validation, and cleanup.
//!
// TODO(AsterFS): use the shared AsterFS private-state/atomic-publication primitives for both
// Unix and Windows; retain the cross-process fixture lock and fingerprint protocol here.

use crate::suite::TestContainerSuite;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::fs::{self, File};
#[cfg(not(unix))]
use std::io::{Read, Write};
use std::path::PathBuf;
#[cfg(not(unix))]
use std::sync::atomic::{AtomicU64, Ordering};

const FIXTURE_STATE_FORMAT_VERSION: u32 = 1;
#[cfg(not(unix))]
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomically published identity of a suite-scoped product fixture.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SuiteFixtureState {
    format_version: u32,
    fixture: String,
    container_identity: String,
    fingerprint: String,
    resource: String,
    producer_version: String,
}

impl SuiteFixtureState {
    /// Creates state for a fully initialized product fixture.
    #[must_use]
    pub fn new(
        fixture: impl Into<String>,
        container_identity: impl Into<String>,
        fingerprint: impl Into<String>,
        resource: impl Into<String>,
        producer_version: impl Into<String>,
    ) -> Self {
        let state = Self {
            format_version: FIXTURE_STATE_FORMAT_VERSION,
            fixture: fixture.into(),
            container_identity: container_identity.into(),
            fingerprint: fingerprint.into(),
            resource: resource.into(),
            producer_version: producer_version.into(),
        };
        state.assert_valid();
        state
    }

    /// Returns the product-defined fixture kind, such as `postgres-template`.
    #[must_use]
    pub fn fixture(&self) -> &str {
        &self.fixture
    }

    /// Returns the shared container identity this fixture was built in.
    #[must_use]
    pub fn container_identity(&self) -> &str {
        &self.container_identity
    }

    /// Returns the product-defined migration or schema fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the product-defined backing resource name.
    #[must_use]
    pub fn resource(&self) -> &str {
        &self.resource
    }

    /// Returns the producer implementation version.
    #[must_use]
    pub fn producer_version(&self) -> &str {
        &self.producer_version
    }

    /// Returns whether state belongs to the requested fixture contract.
    #[must_use]
    pub fn matches(
        &self,
        fixture: &str,
        container_identity: &str,
        fingerprint: &str,
        producer_version: &str,
    ) -> bool {
        self.format_version == FIXTURE_STATE_FORMAT_VERSION
            && self.fixture == fixture
            && self.container_identity == container_identity
            && self.fingerprint == fingerprint
            && self.producer_version == producer_version
    }

    fn assert_valid(&self) {
        assert_eq!(
            self.format_version, FIXTURE_STATE_FORMAT_VERSION,
            "unsupported suite fixture state format version {}",
            self.format_version
        );
        for (field, value) in [
            ("fixture", self.fixture.as_str()),
            ("container_identity", self.container_identity.as_str()),
            ("fingerprint", self.fingerprint.as_str()),
            ("resource", self.resource.as_str()),
            ("producer_version", self.producer_version.as_str()),
        ] {
            assert!(
                !value.is_empty() && !value.contains(['\r', '\n']),
                "suite fixture state {field} must be a non-empty single-line value"
            );
        }
    }
}

/// Exclusive lock and atomic state file for one suite fixture.
///
/// Keep this guard for the complete validate-or-rebuild transaction. A producer that exits before
/// [`Self::publish`] leaves no visible fixture state, so the next process can safely clean the
/// product's deterministic candidate resource and rebuild it.
pub struct SuiteFixtureLock {
    file: File,
    #[cfg(unix)]
    directory: std::sync::Arc<aster_forge_utils::state_directory::StateDirectory>,
    state_path: PathBuf,
}

impl SuiteFixtureLock {
    /// Acquires the cross-process lock for one suite fixture.
    ///
    /// # Panics
    ///
    /// Panics when the fixture name is invalid or the lock file cannot be opened or locked.
    #[must_use]
    pub fn acquire(suite: &TestContainerSuite, fixture: &str) -> Self {
        let lock = Self::open(suite, fixture);
        lock.file
            .lock_exclusive()
            .unwrap_or_else(|error| panic!("failed to lock suite fixture state: {error}"));
        lock
    }

    async fn acquire_async(suite: &TestContainerSuite, fixture: &str) -> Self {
        loop {
            let suite = suite.clone();
            let fixture = fixture.to_owned();
            let acquired = tokio::task::spawn_blocking(move || {
                let lock = Self::open(&suite, &fixture);
                match lock.file.try_lock_exclusive() {
                    Ok(()) => Some(lock),
                    Err(error)
                        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                    {
                        None
                    }
                    Err(error) => panic!("failed to try suite fixture lock: {error}"),
                }
            })
            .await
            .expect("fixture lock task should complete");
            if let Some(lock) = acquired {
                return lock;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    fn open(suite: &TestContainerSuite, fixture: &str) -> Self {
        assert_valid_fixture_name(fixture);
        let state_path = suite.fixture_state_path(fixture);
        let lock_path = suite.fixture_lock_path(fixture);
        #[cfg(unix)]
        let file = suite
            .directory
            .open_lock(
                lock_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap(),
            )
            .unwrap_or_else(|error| panic!("unsafe test lock {}: {error}", lock_path.display()));
        #[cfg(not(unix))]
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to open suite fixture lock {}: {error}",
                    lock_path.display()
                )
            });
        Self {
            file,
            #[cfg(unix)]
            directory: suite.directory.clone(),
            state_path,
        }
    }

    /// Loads the last fully published state, or `None` when no fixture has been published.
    ///
    /// # Panics
    ///
    /// Panics when state cannot be read, decoded, or validated.
    #[must_use]
    pub fn load(&self) -> Option<SuiteFixtureState> {
        #[cfg(unix)]
        let raw = {
            let directory = &self.directory;
            match directory.read(
                self.state_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap(),
                16 * 1024 * 1024,
            ) {
                Ok(bytes) => String::from_utf8(bytes).unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
                Err(error) => panic!("unsafe fixture state: {error}"),
            }
        };
        #[cfg(not(unix))]
        let raw = {
            if !self.state_path.exists() {
                return None;
            }

            let mut raw = String::new();
            File::open(&self.state_path)
                .and_then(|mut file| file.read_to_string(&mut raw))
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to read suite fixture state {}: {error}",
                        self.state_path.display()
                    )
                });
            raw
        };
        if raw.trim().is_empty() {
            return None;
        }

        let state: SuiteFixtureState = serde_json::from_str(&raw).unwrap_or_else(|error| {
            panic!(
                "failed to parse suite fixture state {}: {error}",
                self.state_path.display()
            )
        });
        state.assert_valid();
        Some(state)
    }

    /// Atomically publishes a completed fixture state while this guard is held.
    ///
    /// # Panics
    ///
    /// Panics when state is invalid or its temporary file cannot be written or published.
    pub fn publish(&self, state: &SuiteFixtureState) {
        state.assert_valid();
        let payload = serde_json::to_vec(state)
            .unwrap_or_else(|error| panic!("failed to serialize suite fixture state: {error}"));
        #[cfg(unix)]
        self.directory
            .atomic_write(
                self.state_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap(),
                &payload,
            )
            .unwrap_or_else(|error| panic!("failed to publish fixture state: {error}"));
        #[cfg(not(unix))]
        {
            let temporary_path = self.state_path.with_extension(format!(
                "json.tmp-{}-{}",
                std::process::id(),
                TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));

            let mut temporary = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to create suite fixture temporary state {}: {error}",
                        temporary_path.display()
                    )
                });
            temporary
                .write_all(&payload)
                .and_then(|()| temporary.write_all(b"\n"))
                .and_then(|()| temporary.sync_all())
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to write suite fixture temporary state {}: {error}",
                        temporary_path.display()
                    )
                });
            drop(temporary);

            if let Err(error) = fs::rename(&temporary_path, &self.state_path) {
                // Windows does not replace a destination through rename. Readers also hold this
                // lock, so a brief missing state is safe: it deterministically triggers rebuild.
                if self.state_path.exists() {
                    fs::remove_file(&self.state_path).unwrap_or_else(|remove_error| {
                    panic!(
                        "failed to replace suite fixture state {} after rename error {error}: {remove_error}",
                        self.state_path.display()
                    )
                });
                    fs::rename(&temporary_path, &self.state_path).unwrap_or_else(|retry_error| {
                    panic!(
                        "failed to replace suite fixture state {} after rename error {error}: {retry_error}",
                        self.state_path.display()
                    )
                });
                } else {
                    panic!(
                        "failed to publish suite fixture state {}: {error}",
                        self.state_path.display()
                    );
                }
            }
        }
    }

    /// Clears published state after product cleanup of a superseded fixture.
    ///
    /// # Panics
    ///
    /// Panics when an existing state file cannot be removed.
    pub fn clear(&self) {
        match fs::remove_file(&self.state_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!(
                "failed to clear suite fixture state {}: {error}",
                self.state_path.display()
            ),
        }
    }
}

/// Validates or rebuilds one product-owned schema/template fixture under the suite lock.
///
/// Forge owns locking, fingerprint comparison, cleanup ordering, and atomic state publication.
/// Products provide the resource probe, migration/seed builder, and cleanup operation.
pub struct TemplateFixtureSpec<'a> {
    /// Product-defined fixture name used for the cross-process lock.
    pub fixture: &'a str,
    /// Deterministic resource name used to recover interrupted builds.
    pub resource: &'a str,
    /// Identity of the shared database/container holding the fixture.
    pub container_identity: &'a str,
    /// Product-defined schema or migration fingerprint.
    pub fingerprint: &'a str,
    /// Version of the product code that created the fixture.
    pub producer_version: &'a str,
}

/// Product-defined identity of a reusable migrated database.
#[derive(Clone, Copy)]
pub struct SchemaFixtureSpec<'a> {
    /// Lock/state key scoped to the test suite.
    pub fixture: &'a str,
    /// Deterministic database name; interrupted builds are removed before rebuilding.
    pub resource: &'a str,
    /// Product migration/schema fingerprint.
    pub fingerprint: &'a str,
    /// Product fixture implementation version.
    pub producer_version: &'a str,
}

/// Product-owned operations required to validate, build, and remove a reusable fixture.
pub struct TemplateFixtureActions<Check, Build, Cleanup> {
    /// Probes whether the named resource can be used. The name is owned so the async operation
    /// may outlive the fixture lock's borrow.
    pub is_usable: Check,
    /// Creates and initializes the resource, returning its name.
    pub build: Build,
    /// Removes an obsolete resource.
    pub cleanup: Cleanup,
}

/// Reuses a matching fixture or rebuilds and publishes it while holding its cross-process lock.
///
/// # Panics
/// Panics when fixture state is corrupt or lock/state filesystem operations fail.
pub async fn ensure_template_locked<Check, CheckFut, Build, BuildFut, Cleanup, CleanupFut>(
    suite: &TestContainerSuite,
    spec: TemplateFixtureSpec<'_>,
    actions: TemplateFixtureActions<Check, Build, Cleanup>,
) -> String
where
    Check: Fn(String) -> CheckFut,
    CheckFut: std::future::Future<Output = bool>,
    Build: FnOnce() -> BuildFut,
    BuildFut: std::future::Future<Output = String>,
    Cleanup: Fn(String) -> CleanupFut,
    CleanupFut: std::future::Future<Output = ()>,
{
    assert_valid_fixture_name(spec.fixture);
    let expected = SuiteFixtureState::new(
        spec.fixture,
        spec.container_identity,
        spec.fingerprint,
        spec.resource,
        spec.producer_version,
    );
    let lock = SuiteFixtureLock::acquire_async(suite, spec.fixture).await;
    if let Some(state) = lock.load() {
        if state.matches(
            spec.fixture,
            spec.container_identity,
            spec.fingerprint,
            spec.producer_version,
        ) && state.resource() == spec.resource
            && (actions.is_usable)(state.resource().to_owned()).await
        {
            return state.resource().to_owned();
        }
        if state.container_identity() == spec.container_identity
            && state.resource() != spec.resource
        {
            (actions.cleanup)(state.resource().to_owned()).await;
        }
        lock.clear();
    }
    (actions.cleanup)(spec.resource.to_owned()).await;
    let resource = (actions.build)().await;
    assert_eq!(
        resource, spec.resource,
        "fixture builder must return its declared resource"
    );
    assert!(
        (actions.is_usable)(resource.clone()).await,
        "new fixture failed its usability probe"
    );
    lock.publish(&expected);
    resource
}

fn assert_valid_fixture_name(name: &str) {
    assert!(
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "suite fixture name must be non-empty ASCII alphanumeric, '-' or '_': {name:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::{SuiteFixtureLock, SuiteFixtureState, TemplateFixtureActions, TemplateFixtureSpec};
    use crate::suite::TestContainerSuite;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn fixture_state_round_trips_and_matches_contract() {
        let suite = TestContainerSuite::new("forge-fixture-state-test");
        let lock = SuiteFixtureLock::acquire(&suite, "postgres-template");
        lock.clear();

        let state = SuiteFixtureState::new(
            "postgres-template",
            "aster-test-forge-fixture-state-test-postgres",
            "migration-sha",
            "fixture_database",
            "asterdrive-0.4.0",
        );
        lock.publish(&state);

        let loaded = lock.load().expect("fixture state should load");
        assert_eq!(loaded, state);
        assert!(loaded.matches(
            "postgres-template",
            "aster-test-forge-fixture-state-test-postgres",
            "migration-sha",
            "asterdrive-0.4.0",
        ));
        assert!(!loaded.matches(
            "postgres-template",
            "aster-test-forge-fixture-state-test-postgres",
            "different-sha",
            "asterdrive-0.4.0",
        ));
        lock.clear();
    }

    #[test]
    fn fixture_lock_serializes_concurrent_publishers() {
        let suite = TestContainerSuite::new("forge-fixture-lock-test");
        let lock = SuiteFixtureLock::acquire(&suite, "schema-template");
        lock.clear();

        let (sender, receiver) = mpsc::channel();
        let suite_for_thread = suite.clone();
        let handle = std::thread::spawn(move || {
            let other = SuiteFixtureLock::acquire(&suite_for_thread, "schema-template");
            sender.send(()).expect("lock test receiver should exist");
            other.clear();
        });

        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
        drop(lock);
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("second publisher should acquire after first lock releases");
        handle
            .join()
            .expect("fixture lock test thread should finish");
    }

    #[test]
    fn fixture_lock_rejects_unsafe_names() {
        let suite = TestContainerSuite::new("forge-fixture-name-test");
        for name in ["", "has space", "../escape", "unicode-测试"] {
            assert!(
                std::panic::catch_unwind(|| SuiteFixtureLock::acquire(&suite, name)).is_err(),
                "fixture name {name:?} should be rejected"
            );
        }
    }

    #[tokio::test]
    async fn template_helper_reuses_matching_state_and_rebuilds_changed_fingerprint() {
        use super::ensure_template_locked;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let suite = TestContainerSuite::new("forge-template-helper-test");
        let fixture = format!("schema-template-{}", std::process::id());
        let lock = SuiteFixtureLock::acquire(&suite, &fixture);
        lock.clear();
        drop(lock);
        let builds = Arc::new(AtomicUsize::new(0));
        let cleanups = Arc::new(AtomicUsize::new(0));
        let first_builds = builds.clone();
        let first = ensure_template_locked(
            &suite,
            TemplateFixtureSpec {
                fixture: &fixture,
                resource: "template_a",
                container_identity: "container-a",
                fingerprint: "schema-v1",
                producer_version: "producer-v1",
            },
            TemplateFixtureActions {
                is_usable: |_| async { true },
                build: move || async move {
                    first_builds.fetch_add(1, Ordering::SeqCst);
                    "template_a".to_owned()
                },
                cleanup: |_| async {},
            },
        )
        .await;
        let second_builds = builds.clone();
        let reused = ensure_template_locked(
            &suite,
            TemplateFixtureSpec {
                fixture: &fixture,
                resource: "template_a",
                container_identity: "container-a",
                fingerprint: "schema-v1",
                producer_version: "producer-v1",
            },
            TemplateFixtureActions {
                is_usable: |_| async { true },
                build: move || async move {
                    second_builds.fetch_add(1, Ordering::SeqCst);
                    "unexpected".to_owned()
                },
                cleanup: |_| async {},
            },
        )
        .await;
        assert_eq!(first, "template_a");
        assert_eq!(reused, first);
        assert_eq!(builds.load(Ordering::SeqCst), 1);

        let rebuild_count = builds.clone();
        let cleanup_count = cleanups.clone();
        let rebuilt = ensure_template_locked(
            &suite,
            TemplateFixtureSpec {
                fixture: &fixture,
                resource: "template_b",
                container_identity: "container-a",
                fingerprint: "schema-v2",
                producer_version: "producer-v1",
            },
            TemplateFixtureActions {
                is_usable: |_| async { true },
                build: move || async move {
                    rebuild_count.fetch_add(1, Ordering::SeqCst);
                    "template_b".to_owned()
                },
                cleanup: move |resource: String| {
                    assert!(["template_a", "template_b"].contains(&resource.as_str()));
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                    async {}
                },
            },
        )
        .await;
        assert_eq!(rebuilt, "template_b");
        assert_eq!(cleanups.load(Ordering::SeqCst), 2);
        assert_eq!(builds.load(Ordering::SeqCst), 2);
        SuiteFixtureLock::acquire(&suite, &fixture).clear();
    }
    #[tokio::test]
    async fn cancelled_template_build_never_publishes_partial_state() {
        use super::ensure_template_locked;
        let suite = TestContainerSuite::new("forge-template-cancel-test");
        let fixture = format!("cancelled-template-{}", std::process::id());
        SuiteFixtureLock::acquire(&suite, &fixture).clear();
        let spec = || TemplateFixtureSpec {
            fixture: &fixture,
            resource: "candidate",
            container_identity: "container",
            fingerprint: "v1",
            producer_version: "v1",
        };
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        {
            let attempt = ensure_template_locked(
                &suite,
                spec(),
                TemplateFixtureActions {
                    is_usable: |_| async { true },
                    cleanup: |_| async {},
                    build: || async {
                        signal.notify_one();
                        std::future::pending::<String>().await
                    },
                },
            );
            tokio::pin!(attempt);
            tokio::select! {
                () = entered.notified() => {},
                _ = &mut attempt => panic!("pending builder unexpectedly completed"),
            }
            // Dropping the future releases the fixture lock without publishing state.
        }
        assert!(SuiteFixtureLock::acquire(&suite, &fixture).load().is_none());
        let rebuilt = ensure_template_locked(
            &suite,
            spec(),
            TemplateFixtureActions {
                is_usable: |_| async { true },
                cleanup: |resource: String| async move {
                    assert_eq!(resource, "candidate");
                },
                build: || async { "candidate".to_owned() },
            },
        )
        .await;
        assert_eq!(rebuilt, "candidate");
        SuiteFixtureLock::acquire(&suite, &fixture).clear();
    }
    #[test]
    fn async_lock_wait_leaves_the_only_blocking_thread_available() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let suite = TestContainerSuite::new("forge-lock-pool-test");
            let fixture = format!("one-thread-{}", std::process::id());
            let lock = SuiteFixtureLock::acquire(&suite, &fixture);
            let waiter_suite = suite.clone();
            let waiter_fixture = fixture.clone();
            let waiter = tokio::spawn(async move {
                SuiteFixtureLock::acquire_async(&waiter_suite, &waiter_fixture).await
            });
            // Let the waiter attempt the held OS lock before submitting build-like file work.
            tokio::time::sleep(Duration::from_millis(30)).await;
            let directory = crate::temp::TestTempDir::new("fixture-blocking-pool");
            tokio::time::timeout(
                Duration::from_secs(2),
                tokio::fs::write(directory.join("build"), b"data"),
            )
            .await
            .unwrap()
            .unwrap();
            drop(lock);
            let acquired = tokio::time::timeout(Duration::from_secs(2), waiter)
                .await
                .unwrap()
                .unwrap();
            drop(acquired);
        });
    }
}
