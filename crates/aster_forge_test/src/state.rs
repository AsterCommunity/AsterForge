//! Shared container state: lock files, per-process resource registry, and stale-process pruning.
//!
//! Test binaries from several processes may share one reusable container. The state file records
//! which process created which resources (for example per-test databases), so a later run can
//! clean up resources whose owner process already exited.

use crate::suite::TestContainerSuite;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

/// Serializable registry of live test processes and the resources they created.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SharedContainerState {
    #[serde(default)]
    pids: Vec<u32>,
    #[serde(default)]
    resources_by_pid: HashMap<u32, Vec<String>>,
    #[serde(default)]
    shared_resources: Vec<String>,
    #[serde(default)]
    endpoint: Option<SharedContainerEndpoint>,
}

/// Last verified endpoint of one reusable suite container.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SharedContainerEndpoint {
    identity: String,
    port: u16,
}

impl SharedContainerEndpoint {
    /// Creates endpoint state for a concrete image/container contract.
    ///
    /// # Panics
    ///
    /// Panics when identity is empty or multiline, or when port is zero.
    #[must_use]
    pub fn new(identity: impl Into<String>, port: u16) -> Self {
        let identity = identity.into();
        assert!(
            !identity.is_empty() && !identity.contains(['\r', '\n']),
            "shared container endpoint identity must be a non-empty single-line value"
        );
        assert_ne!(port, 0, "shared container endpoint port must be non-zero");
        Self { identity, port }
    }

    /// Returns whether this endpoint belongs to the requested container contract.
    #[must_use]
    pub fn matches(&self, identity: &str) -> bool {
        self.identity == identity
    }

    /// Returns the published host port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl SharedContainerState {
    /// Registers a live process id.
    pub fn register_pid(&mut self, pid: u32) {
        if !self.pids.contains(&pid) {
            self.pids.push(pid);
        }
        self.normalize();
    }

    /// Records a resource (for example a per-test database name) owned by `pid`.
    pub fn remember_resource(&mut self, pid: u32, resource: &str) {
        self.register_pid(pid);
        let resources = self.resources_by_pid.entry(pid).or_default();
        if !resources.iter().any(|name| name == resource) {
            resources.push(resource.to_string());
        }
        resources.sort_unstable();
    }

    /// Removes one resource from `pid` after the owning test cleaned it up successfully.
    pub fn forget_resource(&mut self, pid: u32, resource: &str) {
        let remove_owner = if let Some(resources) = self.resources_by_pid.get_mut(&pid) {
            resources.retain(|name| name != resource);
            resources.is_empty()
        } else {
            false
        };
        if remove_owner {
            self.resources_by_pid.remove(&pid);
        }
        self.normalize();
    }

    /// Records a suite-scoped resource that must survive the producer process.
    ///
    /// Products use this for durable test fixtures such as a migrated template database. Unlike
    /// per-process resources, these entries are never returned by [`Self::prune_stale`]; the
    /// product owns fingerprint-based invalidation and explicit cleanup.
    pub fn remember_shared_resource(&mut self, resource: &str) {
        if !self.shared_resources.iter().any(|name| name == resource) {
            self.shared_resources.push(resource.to_string());
        }
        self.normalize();
    }

    /// Removes a suite-scoped resource after explicit product cleanup.
    pub fn forget_shared_resource(&mut self, resource: &str) {
        self.shared_resources.retain(|name| name != resource);
        self.normalize();
    }

    /// Returns suite-scoped resources retained independently of a producer PID.
    #[must_use]
    pub fn shared_resources(&self) -> &[String] {
        &self.shared_resources
    }

    /// Returns the last verified reusable-container endpoint.
    #[must_use]
    pub fn endpoint(&self) -> Option<&SharedContainerEndpoint> {
        self.endpoint.as_ref()
    }

    /// Publishes the last verified reusable-container endpoint.
    pub fn set_endpoint(&mut self, endpoint: SharedContainerEndpoint) {
        self.endpoint = Some(endpoint);
    }

    /// Clears an endpoint that no longer accepts a readiness probe.
    pub fn clear_endpoint(&mut self) {
        self.endpoint = None;
    }

    /// Returns the resources currently attributed to live processes.
    pub fn live_resources(&self) -> Vec<&str> {
        self.resources_by_pid
            .values()
            .flatten()
            .map(String::as_str)
            .collect()
    }

    /// Removes entries whose process no longer exists and returns the orphaned resources.
    pub fn prune_stale(&mut self) -> Vec<String> {
        let stale_pids = self
            .pids
            .iter()
            .copied()
            .filter(|pid| !process_is_running(*pid))
            .collect::<Vec<_>>();
        let orphaned = stale_pids
            .iter()
            .flat_map(|pid| self.resources_by_pid.remove(pid).unwrap_or_default())
            .collect::<Vec<_>>();

        self.pids.retain(|pid| !stale_pids.contains(pid));
        self.normalize();
        orphaned
    }

    fn normalize(&mut self) {
        self.pids.sort_unstable();
        self.pids.dedup();
        self.resources_by_pid
            .retain(|pid, _| self.pids.binary_search(pid).is_ok());
        self.shared_resources.sort_unstable();
        self.shared_resources.dedup();
    }
}

/// Exclusive filesystem lock guarding one service's state file.
///
/// Hold the lock for the whole read-modify-write cycle. The lock is released when the guard
/// drops.
pub struct ContainerStateLock {
    _file: File,
    state_path: std::path::PathBuf,
}

impl ContainerStateLock {
    /// Acquires the exclusive lock for `service` within `suite`, blocking until available.
    ///
    /// # Panics
    ///
    /// Panics when the lock file cannot be opened or exclusively locked.
    #[must_use]
    pub fn acquire(suite: &TestContainerSuite, service: &str) -> Self {
        let lock_path = suite.lock_path(service);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to open test container lock {}: {error}",
                    lock_path.display()
                )
            });
        file.lock_exclusive().unwrap_or_else(|error| {
            panic!(
                "failed to lock test container state {}: {error}",
                lock_path.display()
            )
        });
        Self {
            _file: file,
            state_path: suite.state_path(service),
        }
    }

    /// Loads the state file, tolerating a missing or empty file.
    ///
    /// # Panics
    ///
    /// Panics when the state file cannot be read or contains invalid JSON.
    #[must_use]
    pub fn load(&self) -> SharedContainerState {
        if !self.state_path.exists() {
            return SharedContainerState::default();
        }

        let mut raw = String::new();
        File::open(&self.state_path)
            .and_then(|mut file| file.read_to_string(&mut raw))
            .unwrap_or_else(|error| {
                panic!(
                    "failed to read test container state {}: {error}",
                    self.state_path.display()
                )
            });

        let mut state = if raw.trim().is_empty() {
            SharedContainerState::default()
        } else {
            serde_json::from_str(&raw).unwrap_or_else(|error| {
                panic!(
                    "failed to parse test container state {}: {error}",
                    self.state_path.display()
                )
            })
        };
        state.normalize();
        state
    }

    /// Persists the state file atomically enough for test purposes (write + flush).
    ///
    /// # Panics
    ///
    /// Panics when state serialization, file creation, writing, or flushing fails.
    pub fn save(&self, state: &SharedContainerState) {
        let json = serde_json::to_vec(state)
            .unwrap_or_else(|error| panic!("failed to serialize test container state: {error}"));
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.state_path)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to open test container state {}: {error}",
                    self.state_path.display()
                )
            });
        file.write_all(&json)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.flush())
            .unwrap_or_else(|error| {
                panic!(
                    "failed to write test container state {}: {error}",
                    self.state_path.display()
                )
            });
    }
}

/// Lease that prunes dead-process entries from a service's state file on drop.
///
/// Test containers hold the lease so abnormal test binary exits still let the next run reclaim
/// orphaned resources.
pub struct ContainerLease {
    suite: TestContainerSuite,
    service: String,
}

impl ContainerLease {
    /// Creates a lease for `service` within `suite`.
    pub fn new(suite: TestContainerSuite, service: impl Into<String>) -> Self {
        Self {
            suite,
            service: service.into(),
        }
    }
}

impl Drop for ContainerLease {
    fn drop(&mut self) {
        let lock = ContainerStateLock::acquire(&self.suite, &self.service);
        let mut state = lock.load();
        let _ = state.prune_stale();
        lock.save(&state);
    }
}

fn process_is_running(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }

    // `kill` reserves zero and negative values for process-group or broadcast semantics. Some
    // implementations parse values above `i32::MAX` into a signed `pid_t` (for example,
    // `u32::MAX` becomes `-1`), which can make an invalid state-file entry look alive.
    if pid == 0 || i32::try_from(pid).is_err() {
        return false;
    }

    platform_process_is_running(pid)
}

#[cfg(unix)]
fn platform_process_is_running(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(not(unix))]
fn platform_process_is_running(pid: u32) -> bool {
    // Without a portable liveness probe, assume processes are alive so entries are kept.
    let _ = pid;
    true
}

#[cfg(test)]
mod tests {
    use super::{
        ContainerLease, ContainerStateLock, SharedContainerEndpoint, SharedContainerState,
    };
    use crate::suite::TestContainerSuite;

    #[test]
    fn state_registry_tracks_resources_per_process() {
        let mut state = SharedContainerState::default();
        state.remember_resource(42, "db_a");
        state.remember_resource(42, "db_b");
        state.remember_resource(42, "db_a");
        state.remember_resource(7, "db_c");

        let mut live = state.live_resources();
        live.sort_unstable();
        assert_eq!(live, vec!["db_a", "db_b", "db_c"]);
    }

    #[test]
    fn state_registry_forgets_cleaned_resources() {
        let mut state = SharedContainerState::default();
        state.remember_resource(42, "db_a");
        state.remember_resource(42, "db_b");

        state.forget_resource(42, "db_a");
        assert_eq!(state.live_resources(), vec!["db_b"]);
        state.forget_resource(42, "db_b");
        assert!(state.live_resources().is_empty());
    }

    #[test]
    fn state_registry_keeps_shared_resources_after_producer_exit() {
        let mut state = SharedContainerState::default();
        state.remember_resource(u32::MAX, "db_owned_by_dead_process");
        state.remember_shared_resource("schema_template");

        assert_eq!(
            state.prune_stale(),
            vec!["db_owned_by_dead_process".to_string()]
        );
        assert_eq!(state.shared_resources(), ["schema_template"]);

        state.forget_shared_resource("schema_template");
        assert!(state.shared_resources().is_empty());
    }

    #[test]
    fn state_registry_round_trips_a_verified_endpoint() {
        let mut state = SharedContainerState::default();
        state.set_endpoint(SharedContainerEndpoint::new("mysql:8.4/container-a", 33061));

        let endpoint = state.endpoint().expect("endpoint should be present");
        assert!(endpoint.matches("mysql:8.4/container-a"));
        assert_eq!(endpoint.port(), 33061);

        state.clear_endpoint();
        assert!(state.endpoint().is_none());
    }

    #[test]
    fn prune_removes_dead_processes_and_returns_orphans() {
        let mut state = SharedContainerState::default();
        state.remember_resource(std::process::id(), "db_live");
        state.remember_resource(u32::MAX, "db_dead");

        let orphaned = state.prune_stale();

        assert_eq!(orphaned, vec!["db_dead".to_string()]);
        assert_eq!(state.live_resources(), vec!["db_live"]);
    }

    #[test]
    fn lock_round_trips_state_through_json_file() {
        let suite = TestContainerSuite::new("forge-state-test");
        let lock = ContainerStateLock::acquire(&suite, "roundtrip");

        // The state file survives across test runs, so drop entries left by previous
        // (already exited) processes before asserting on what this run sees.
        let mut state = lock.load();
        let _ = state.prune_stale();
        state.remember_resource(std::process::id(), "db_persisted");
        lock.save(&state);

        let loaded = lock.load();
        assert_eq!(loaded.live_resources(), vec!["db_persisted"]);
    }

    #[test]
    fn lease_drop_preserves_live_entries() {
        let suite = TestContainerSuite::new("forge-lease-test");
        {
            let lock = ContainerStateLock::acquire(&suite, "leased");
            let mut state = lock.load();
            state.remember_resource(std::process::id(), "db_live");
            lock.save(&state);
        }

        drop(ContainerLease::new(suite.clone(), "leased"));

        let lock = ContainerStateLock::acquire(&suite, "leased");
        assert_eq!(lock.load().live_resources(), vec!["db_live"]);
    }
}
