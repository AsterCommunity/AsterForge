//! Cross-process suite fixture metadata.
//!
//! Containers and per-test databases are short-lived mechanics. A migrated template database or
//! schema snapshot is different: it is a suite-scoped product fixture that can be consumed by
//! many nextest processes after its producer exits. This module owns only the locked, atomic
//! metadata publication protocol; products own fixture names, contents, validation, and cleanup.

use crate::suite::TestContainerSuite;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const FIXTURE_STATE_FORMAT_VERSION: u32 = 1;
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
    _file: File,
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
        assert_valid_fixture_name(fixture);
        let state_path = suite
            .state_dir()
            .join(format!("{}-fixture-{fixture}.json", suite.name()));
        let lock_path = suite
            .state_dir()
            .join(format!("{}-fixture-{fixture}.lock", suite.name()));
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
        file.lock_exclusive().unwrap_or_else(|error| {
            panic!(
                "failed to lock suite fixture state {}: {error}",
                lock_path.display()
            )
        });
        Self {
            _file: file,
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
    use super::{SuiteFixtureLock, SuiteFixtureState};
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
}
