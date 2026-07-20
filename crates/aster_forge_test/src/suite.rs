//! Test-suite identity shared by container helpers.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Identity of a test suite that owns shared containers.
///
/// The suite name keeps containers from different products apart, while a hash of the current
/// working directory isolates parallel checkouts of the same product on one machine. Cargo runs
/// test binaries with the package directory as working directory, so each checkout gets its own
/// instance id without any compile-time env tricks.
#[derive(Debug, Clone)]
pub struct TestContainerSuite {
    name: String,
    state_dir: PathBuf,
    instance: String,
}

impl TestContainerSuite {
    /// Creates a suite rooted at `<temp dir>/aster-testcontainers-<name>`.
    ///
    /// The name becomes part of container names and lock file paths, so it must be non-empty
    /// ASCII alphanumeric or `-`.
    pub fn new(name: &str) -> Self {
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "test container suite name must be non-empty ascii alphanumeric or '-': {name:?}"
        );
        let state_dir = std::env::temp_dir().join(format!("aster-testcontainers-{name}"));
        std::fs::create_dir_all(&state_dir).unwrap_or_else(|error| {
            panic!(
                "failed to create test container state dir {}: {error}",
                state_dir.display()
            )
        });
        Self {
            name: name.to_string(),
            state_dir,
            instance: instance_id().to_string(),
        }
    }

    /// Returns the suite name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the directory holding lock files and state JSON for this suite.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Returns the shared container name for a service.
    pub fn container_name(&self, service: &str) -> String {
        format!("aster-test-{}-{}-{service}", self.name, self.instance)
    }

    pub(crate) fn lock_path(&self, service: &str) -> PathBuf {
        self.state_dir
            .join(format!("{}-{}-{service}.lock", self.name, self.instance))
    }

    pub(crate) fn state_path(&self, service: &str) -> PathBuf {
        self.state_dir
            .join(format!("{}-{}-{service}.json", self.name, self.instance))
    }
}

fn instance_id() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    })
}

#[cfg(test)]
mod tests {
    use super::TestContainerSuite;

    #[test]
    fn suite_rejects_invalid_names() {
        for name in ["", "has space", "has/slash", "中文"] {
            let result = std::panic::catch_unwind(|| TestContainerSuite::new(name));
            assert!(result.is_err(), "suite name {name:?} should be rejected");
        }
    }

    #[test]
    fn suite_builds_scoped_paths_and_container_names() {
        let suite = TestContainerSuite::new("forge-test");
        assert_eq!(suite.name(), "forge-test");
        assert!(
            suite
                .state_dir()
                .ends_with("aster-testcontainers-forge-test")
        );

        let container = suite.container_name("redis");
        assert!(container.starts_with("aster-test-forge-test-"));
        assert!(container.ends_with("-redis"));
        assert_ne!(suite.lock_path("redis"), suite.state_path("redis"));
    }
}
