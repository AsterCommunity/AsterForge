//! Path rendering and configuration-relative path helpers.
//!
//! Aster services commonly accept paths from static configuration while running with a fixed data
//! base directory. This module centralizes the string-level path joining and normalization rules
//! used by those services: redundant slashes are trimmed, `.` and safe `..` components are folded,
//! paths configured relative to the config file are rendered back as runtime-relative paths under
//! the data base directory, and sqlite URLs keep their query string while resolving the embedded
//! filesystem path. The helpers intentionally avoid filesystem canonicalization so they work before
//! directories or database files exist.

use std::path::{Component, Path, PathBuf};

use crate::{Result, UtilsError};

const DEFAULT_DATA_DIR_NAME: &str = "data";

/// Joins two slash-separated path fragments without emitting duplicate separators.
///
/// The helper is designed for runtime paths stored in configuration or database records. It keeps a
/// leading slash from `root`, trims trailing slashes from `root`, and trims leading/trailing slashes
/// from `leaf`.
pub fn join_path(root: &str, leaf: &str) -> String {
    let root_had_leading_slash = root.starts_with('/');
    let root = root.trim_end_matches('/');
    let leaf = leaf.trim_matches('/');

    if root.is_empty() {
        return if leaf.is_empty() {
            if root_had_leading_slash {
                "/".to_string()
            } else {
                String::new()
            }
        } else if root_had_leading_slash {
            format!("/{leaf}")
        } else {
            leaf.to_string()
        };
    }

    if leaf.is_empty() {
        return root.to_string();
    }

    format!("{root}/{leaf}")
}

/// Normalizes a path lexically without touching the filesystem.
///
/// `.` components are dropped. `..` removes the previous normal component when possible, but it is
/// preserved when removing it would cross an unknown relative root. Absolute roots and platform
/// prefixes are retained.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => normalized.push(component.as_os_str()),
            },
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

/// Renders a resolved path as a runtime-relative path under `base_dir`.
///
/// The returned path is relative to the normalized `base_dir`. If `resolved` points outside
/// `base_dir`, the function rejects it instead of returning a path with leading `..` segments.
pub fn render_runtime_relative_path(base_dir: &Path, resolved: &Path) -> Result<String> {
    let normalized_base_dir = normalize_path(base_dir);
    let normalized_resolved = normalize_path(resolved);

    match normalized_resolved.strip_prefix(&normalized_base_dir) {
        Ok(stripped) if stripped.as_os_str().is_empty() => Ok(".".to_string()),
        Ok(stripped) => Ok(stripped.to_string_lossy().to_string()),
        Err(_) => Err(UtilsError::invalid_value(format!(
            "configured relative path resolves outside data base_dir: base_dir='{}', resolved='{}'",
            normalized_base_dir.display(),
            normalized_resolved.display()
        ))),
    }
}

fn is_data_prefixed_relative_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Normal(component)) if component == DEFAULT_DATA_DIR_NAME
    )
}

/// Resolves a config value into the runtime path form used under `base_dir`.
///
/// Empty values are preserved. Absolute paths are normalized and returned as absolute paths.
/// Relative values starting with `data` are anchored at `base_dir`; all other relative values are
/// anchored at `config_dir`, then rendered relative to `base_dir`. Values resolving outside
/// `base_dir` are rejected.
pub fn resolve_config_relative_path(
    base_dir: &Path,
    config_dir: &Path,
    value: &str,
) -> Result<String> {
    if value.is_empty() {
        return Ok(value.to_string());
    }

    let configured_path = Path::new(value);
    if configured_path.is_absolute() {
        return Ok(normalize_path(configured_path)
            .to_string_lossy()
            .to_string());
    }

    let anchor_dir = if is_data_prefixed_relative_path(configured_path) {
        base_dir
    } else {
        config_dir
    };
    let resolved = normalize_path(&anchor_dir.join(configured_path));

    render_runtime_relative_path(base_dir, &resolved)
}

/// Resolves the filesystem path inside a sqlite URL while preserving sqlite-specific values.
///
/// Non-sqlite URLs, `sqlite::memory:`, `sqlite://`, and `sqlite://:memory:` are returned unchanged.
/// For file-backed sqlite URLs, the embedded path is resolved with
/// [`resolve_config_relative_path`] and the original query string is retained.
pub fn resolve_config_relative_sqlite_url(
    base_dir: &Path,
    config_dir: &Path,
    value: &str,
) -> Result<String> {
    if value == "sqlite::memory:" {
        return Ok(value.to_string());
    }

    let Some(path_and_query) = value.strip_prefix("sqlite://") else {
        return Ok(value.to_string());
    };
    let (raw_path, raw_query) = match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    };

    if raw_path.is_empty() || raw_path == ":memory:" {
        return Ok(value.to_string());
    }

    let configured_path = Path::new(raw_path);
    let resolved_path = if configured_path.is_absolute() {
        normalize_path(configured_path)
            .to_string_lossy()
            .to_string()
    } else {
        resolve_config_relative_path(base_dir, config_dir, raw_path)?
    };

    match raw_query {
        Some(query) => Ok(format!("sqlite://{resolved_path}?{query}")),
        None => Ok(format!("sqlite://{resolved_path}")),
    }
}

/// Returns the path to a temporary file under `temp_dir`.
pub fn temp_file_path(temp_dir: &str, name: &str) -> String {
    join_path(temp_dir, name)
}

/// Returns the namespaced runtime temporary directory under `temp_root`.
pub fn runtime_temp_dir(temp_root: &str) -> String {
    join_path(temp_root, "_runtime")
}

/// Returns a runtime temporary file path under the `_runtime` namespace.
pub fn runtime_temp_file_path(temp_root: &str, name: &str) -> String {
    join_path(&runtime_temp_dir(temp_root), name)
}

/// Returns the temporary directory for a multipart upload session.
pub fn upload_temp_dir(upload_temp_root: &str, upload_id: &str) -> String {
    join_path(upload_temp_root, upload_id)
}

/// Returns the temporary path for one uploaded chunk.
pub fn upload_chunk_path(upload_temp_root: &str, upload_id: &str, chunk_number: i32) -> String {
    join_path(
        &upload_temp_dir(upload_temp_root, upload_id),
        &format!("chunk_{chunk_number}"),
    )
}

/// Returns the assembled-file temporary path for a multipart upload session.
pub fn upload_assembled_path(upload_temp_root: &str, upload_id: &str) -> String {
    join_path(&upload_temp_dir(upload_temp_root, upload_id), "_assembled")
}

/// Returns the temporary directory for a background task.
pub fn task_temp_dir(temp_root: &str, task_id: i64) -> String {
    join_path(temp_root, &format!("tasks/{task_id}"))
}

/// Returns the temporary directory for a specific task processing token.
///
/// The processing token keeps artifacts from separate leases isolated when an old worker wakes up
/// after a newer lease has already started.
pub fn task_token_temp_dir(temp_root: &str, task_id: i64, processing_token: i64) -> String {
    join_path(
        &task_temp_dir(temp_root, task_id),
        &processing_token.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        join_path, normalize_path, render_runtime_relative_path, resolve_config_relative_path,
        resolve_config_relative_sqlite_url, runtime_temp_dir, runtime_temp_file_path,
        task_temp_dir, task_token_temp_dir, temp_file_path, upload_assembled_path,
        upload_chunk_path, upload_temp_dir,
    };
    use crate::UtilsError;
    use std::path::{Path, PathBuf};

    fn assert_no_double_slash(path: &str) {
        assert!(
            !path.contains("//"),
            "path should not contain double slashes: {path}"
        );
    }

    #[test]
    fn join_path_handles_empty_and_absolute_roots() {
        assert_eq!(join_path("", ""), "");
        assert_eq!(join_path("", "leaf"), "leaf");
        assert_eq!(join_path("/", ""), "/");
        assert_eq!(join_path("/", "/leaf/"), "/leaf");
        assert_eq!(join_path("/tmp///", "///runtime.bin"), "/tmp/runtime.bin");
    }

    #[test]
    fn normalize_path_folds_current_and_parent_components() {
        assert_eq!(
            normalize_path(Path::new("/srv/app/data/../data/./.tmp")),
            PathBuf::from("/srv/app/data/.tmp")
        );
        assert_eq!(
            normalize_path(Path::new("./data/./.tmp")),
            PathBuf::from("data/.tmp")
        );
        assert_eq!(
            normalize_path(Path::new("../shared")),
            PathBuf::from("../shared")
        );
        assert_eq!(normalize_path(Path::new(".")), PathBuf::from("."));
    }

    #[test]
    fn render_runtime_relative_path_rejects_paths_outside_base_dir() {
        let base_dir = Path::new("/srv/app");
        let resolved = Path::new("/srv/shared");

        let error = render_runtime_relative_path(base_dir, resolved).unwrap_err();
        assert!(matches!(error, UtilsError::InvalidValue(_)));
        assert!(error.to_string().contains("outside data base_dir"));
    }

    #[test]
    fn temp_file_path_joins_normal_inputs() {
        let path = temp_file_path("data/.tmp", "abc123");
        assert_eq!(path, "data/.tmp/abc123");
        assert_no_double_slash(&path);
    }

    #[test]
    fn temp_file_path_trims_user_supplied_slashes() {
        let path = temp_file_path("data/.tmp///", "/nested/file.tmp/");
        assert_eq!(path, "data/.tmp/nested/file.tmp");
        assert_no_double_slash(&path);
    }

    #[test]
    fn temp_file_path_preserves_absolute_root_without_double_slash() {
        let path = temp_file_path("/tmp///", "///upload.bin");
        assert_eq!(path, "/tmp/upload.bin");
        assert_no_double_slash(&path);
    }

    #[test]
    fn runtime_temp_file_path_nests_under_runtime_subdir() {
        let path = runtime_temp_file_path("data/.tmp///", "/abc123/");
        assert_eq!(path, "data/.tmp/_runtime/abc123");
        assert_no_double_slash(&path);
    }

    #[test]
    fn runtime_temp_dir_uses_namespaced_subdir() {
        let path = runtime_temp_dir("/tmp///");
        assert_eq!(path, "/tmp/_runtime");
        assert_no_double_slash(&path);
    }

    #[test]
    fn upload_paths_trim_edge_case_inputs() {
        let dir = upload_temp_dir("data/.uploads///", "/session-123/");
        assert_eq!(dir, "data/.uploads/session-123");
        assert_no_double_slash(&dir);

        let chunk = upload_chunk_path("data/.uploads///", "///session-123///", 7);
        assert_eq!(chunk, "data/.uploads/session-123/chunk_7");
        assert_no_double_slash(&chunk);

        let assembled = upload_assembled_path("/var/tmp/uploads///", "///session-123///");
        assert_eq!(assembled, "/var/tmp/uploads/session-123/_assembled");
        assert_no_double_slash(&assembled);
    }

    #[test]
    fn empty_upload_id_returns_normalized_upload_root() {
        let path = upload_temp_dir("data/.uploads///", "");
        assert_eq!(path, "data/.uploads");
        assert_no_double_slash(&path);
    }

    #[test]
    fn task_paths_do_not_emit_double_slashes() {
        let dir = task_temp_dir("data/.tmp///", 42);
        assert_eq!(dir, "data/.tmp/tasks/42");
        assert_no_double_slash(&dir);
    }

    #[test]
    fn task_token_temp_dir_nests_under_task_root() {
        let path = task_token_temp_dir("data/.tmp///", 42, 7);
        assert_eq!(path, "data/.tmp/tasks/42/7");
        assert_no_double_slash(&path);
    }

    #[test]
    fn resolve_config_relative_path_accepts_plain_and_data_prefixed_relative_values() {
        let base_dir = Path::new("/srv/asterapp");
        let config_dir = Path::new("/srv/asterapp/data");

        assert_eq!(
            resolve_config_relative_path(base_dir, config_dir, ".tmp").unwrap(),
            "data/.tmp"
        );
        assert_eq!(
            resolve_config_relative_path(base_dir, config_dir, "data/.tmp").unwrap(),
            "data/.tmp"
        );
        assert_eq!(
            resolve_config_relative_path(base_dir, config_dir, "../shared").unwrap(),
            "shared"
        );
    }

    #[test]
    fn resolve_config_relative_path_preserves_empty_and_absolute_values() {
        let base_dir = Path::new("/srv/asterapp");
        let config_dir = Path::new("/srv/asterapp/data");

        assert_eq!(
            resolve_config_relative_path(base_dir, config_dir, "").unwrap(),
            ""
        );
        assert_eq!(
            resolve_config_relative_path(base_dir, config_dir, "/var/lib/asterapp/../app/data")
                .unwrap(),
            "/var/lib/app/data"
        );
    }

    #[test]
    fn resolve_config_relative_path_rejects_values_outside_base_dir() {
        let base_dir = Path::new("/srv/asterapp");
        let config_dir = Path::new("/srv/asterapp/data");

        let error = resolve_config_relative_path(base_dir, config_dir, "../../shared")
            .expect_err("path outside base_dir should be rejected");
        assert!(error.to_string().contains("outside data base_dir"));
    }

    #[test]
    fn resolve_config_relative_sqlite_url_accepts_plain_and_data_prefixed_relative_values() {
        let base_dir = Path::new("/srv/asterapp");
        let config_dir = Path::new("/srv/asterapp/data");

        assert_eq!(
            resolve_config_relative_sqlite_url(
                base_dir,
                config_dir,
                "sqlite://asterapp.db?mode=rwc"
            )
            .unwrap(),
            "sqlite://data/asterapp.db?mode=rwc"
        );
        assert_eq!(
            resolve_config_relative_sqlite_url(
                base_dir,
                config_dir,
                "sqlite://data/asterapp.db?mode=rwc"
            )
            .unwrap(),
            "sqlite://data/asterapp.db?mode=rwc"
        );
        assert_eq!(
            resolve_config_relative_sqlite_url(
                base_dir,
                config_dir,
                "sqlite:///var/lib/asterapp/custom.db?mode=rwc"
            )
            .unwrap(),
            "sqlite:///var/lib/asterapp/custom.db?mode=rwc"
        );
    }

    #[test]
    fn resolve_config_relative_sqlite_url_preserves_non_file_backed_values() {
        let base_dir = Path::new("/srv/asterapp");
        let config_dir = Path::new("/srv/asterapp/data");

        assert_eq!(
            resolve_config_relative_sqlite_url(base_dir, config_dir, "sqlite::memory:").unwrap(),
            "sqlite::memory:"
        );
        assert_eq!(
            resolve_config_relative_sqlite_url(base_dir, config_dir, "sqlite://:memory:").unwrap(),
            "sqlite://:memory:"
        );
        assert_eq!(
            resolve_config_relative_sqlite_url(base_dir, config_dir, "postgres://localhost/db")
                .unwrap(),
            "postgres://localhost/db"
        );
    }

    #[test]
    fn resolve_config_relative_sqlite_url_rejects_values_outside_base_dir() {
        let base_dir = Path::new("/srv/asterapp");
        let config_dir = Path::new("/srv/asterapp/data");

        let error = resolve_config_relative_sqlite_url(
            base_dir,
            config_dir,
            "sqlite://../../shared/asterapp.db?mode=rwc",
        )
        .expect_err("sqlite path outside base_dir should be rejected");
        assert!(error.to_string().contains("outside data base_dir"));
    }
}
