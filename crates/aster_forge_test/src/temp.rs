//! Isolated temporary filesystem fixtures for tests.

use aster_forge_utils::raii::TempDirGuard;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// A uniquely named temporary directory backed by [`TempDirGuard`].
///
/// The directory name includes the process ID, a process-local counter, and the current timestamp
/// so parallel test binaries and repeated runs do not share filesystem state. The utils-layer
/// guard owns recursive cleanup, including early-return and panic paths.
#[must_use = "keep the fixture alive for as long as its temporary files are in use"]
pub struct TestTempDir {
    guard: TempDirGuard,
}

impl TestTempDir {
    /// Creates an isolated directory under the platform temporary directory.
    pub fn new(scope: &str) -> Self {
        Self::new_in(std::env::temp_dir(), scope)
    }

    /// Creates an isolated directory below `root`.
    ///
    /// This is useful when a test intentionally needs a path below the package directory, such as
    /// configuration tests that verify runtime-relative path rendering.
    pub fn new_in(root: impl AsRef<Path>, scope: &str) -> Self {
        assert_valid_scope(scope);
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let path = root.as_ref().join(format!(
            "aster-test-{scope}-{}-{id}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!(
                "failed to create isolated test directory {}: {error}",
                path.display()
            )
        });
        Self {
            guard: TempDirGuard::new(path, "isolated test directory"),
        }
    }

    /// Returns the owned temporary directory path.
    pub fn path(&self) -> &Path {
        self.guard.path()
    }

    /// Joins a test-owned relative path below the temporary directory.
    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        assert!(
            path.components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
            "test fixture path must stay relative to its temporary directory: {path:?}"
        );
        self.path().join(path)
    }
}

impl std::fmt::Debug for TestTempDir {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TestTempDir")
            .field("path", &self.path())
            .finish()
    }
}

/// A file-backed SQLite test database inside an isolated temporary directory.
///
/// The fixture owns the directory rather than only the main database file, so SQLite journal,
/// WAL, and shared-memory sidecars are cleaned together. Database handles must be closed before
/// this value is dropped on platforms that lock open files.
#[derive(Debug)]
#[must_use = "keep the fixture alive until all SQLite connections have been closed"]
pub struct SqliteTestDatabase {
    directory: TestTempDir,
    path: PathBuf,
    url: String,
}

impl SqliteTestDatabase {
    /// Creates a uniquely named file-backed SQLite fixture.
    pub fn new(scope: &str) -> Self {
        let directory = TestTempDir::new(&format!("sqlite-{scope}"));
        let path = directory.join("database.sqlite3");
        let url = sqlite_database_url(&path);
        Self {
            directory,
            path,
            url,
        }
    }

    /// Returns the database file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns a `mode=rwc` SQLite URL with the native path percent-encoded.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the directory that owns the database and any SQLite sidecars.
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
}

/// Builds a file-backed SQLite URL from a native filesystem path.
///
/// The opaque `sqlite:` form lets drive letters, backslashes, spaces, `?`, and `#` remain part of
/// the database filename after SQLx percent-decodes it, while the URL still passes generic URL
/// validation performed by SeaORM.
pub fn sqlite_database_url(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let path = path.to_str().unwrap_or_else(|| {
        panic!(
            "SQLite test database path must be valid UTF-8: {}",
            path.display()
        )
    });
    let encoded = percent_encode_sqlite_path(path);
    format!("sqlite:{encoded}?mode=rwc")
}

fn percent_encode_sqlite_path(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    encoded
}

fn assert_valid_scope(scope: &str) {
    assert!(
        !scope.is_empty()
            && scope
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "test temp scope must be non-empty ASCII alphanumeric, '-' or '_': {scope:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::{SqliteTestDatabase, TestTempDir, sqlite_database_url};
    use std::path::Path;

    #[test]
    fn test_temp_dir_creates_and_removes_an_isolated_tree() {
        let path;
        {
            let directory = TestTempDir::new("temp-dir-cleanup");
            path = directory.path().to_path_buf();
            std::fs::write(directory.join("payload.txt"), b"fixture")
                .expect("fixture file should be written");
            assert!(path.is_dir());
        }
        assert!(!path.exists());
    }

    #[test]
    fn test_temp_dir_rejects_path_components_in_scope() {
        for scope in ["", "has space", "../escape", "nested/path", "nested\\path"] {
            let result = std::panic::catch_unwind(|| TestTempDir::new(scope));
            assert!(result.is_err(), "scope {scope:?} should be rejected");
        }
    }

    #[test]
    fn test_temp_dir_join_rejects_paths_outside_fixture() {
        let directory = TestTempDir::new("join-boundary");
        for path in [Path::new("../escape"), Path::new("nested/../../escape")] {
            let result = std::panic::catch_unwind(|| directory.join(path));
            assert!(result.is_err(), "path {path:?} should be rejected");
        }
    }

    #[test]
    fn sqlite_url_percent_encodes_windows_and_reserved_path_characters() {
        assert_eq!(
            sqlite_database_url(Path::new(r"C:\Temp Folder\db?#.sqlite3")),
            "sqlite:C%3A%5CTemp%20Folder%5Cdb%3F%23.sqlite3?mode=rwc"
        );
    }

    #[test]
    fn sqlite_fixture_owns_database_parent_and_parseable_url() {
        let database = SqliteTestDatabase::new("database-fixture");
        assert_eq!(database.path().parent(), Some(database.directory()));
        assert!(database.url().ends_with("?mode=rwc"));
        url::Url::parse(database.url()).expect("SQLite fixture URL should pass URL validation");
    }
}
