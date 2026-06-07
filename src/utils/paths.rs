//! 工具子模块：`paths`。

use crate::errors::{AsterError, Result};
use std::path::{Component, Path, PathBuf};

pub const DEFAULT_DATA_DIR: &str = "data";
pub const DEFAULT_CONFIG_PATH: &str = "data/config.toml";
pub const DEFAULT_SQLITE_DATABASE_PATH: &str = "data/asterforge.db";
pub const DEFAULT_CONFIG_SQLITE_DATABASE_URL: &str = "sqlite://asterforge.db?mode=rwc";
pub const DEFAULT_SQLITE_DATABASE_URL: &str = "sqlite://data/asterforge.db?mode=rwc";
pub const DEFAULT_CONFIG_TEMP_DIR: &str = ".tmp";
pub const DEFAULT_TEMP_DIR: &str = "data/.tmp";

fn join_path(root: &str, leaf: &str) -> String {
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

fn normalize_path(path: &Path) -> PathBuf {
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

fn render_runtime_relative_path(base_dir: &Path, resolved: &Path) -> Result<String> {
    let normalized_base_dir = normalize_path(base_dir);
    let normalized_resolved = normalize_path(resolved);

    match normalized_resolved.strip_prefix(&normalized_base_dir) {
        Ok(stripped) if stripped.as_os_str().is_empty() => Ok(".".to_string()),
        Ok(stripped) => Ok(stripped.to_string_lossy().to_string()),
        Err(_) => Err(AsterError::config_error(format!(
            "configured relative path resolves outside data base_dir: base_dir='{}', resolved='{}'",
            normalized_base_dir.display(),
            normalized_resolved.display()
        ))),
    }
}

fn is_data_prefixed_relative_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Normal(component)) if component == DEFAULT_DATA_DIR
    )
}

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

pub fn temp_file_path(temp_dir: &str, name: &str) -> String {
    join_path(temp_dir, name)
}

pub fn runtime_temp_dir(temp_root: &str) -> String {
    join_path(temp_root, "_runtime")
}

pub fn runtime_temp_file_path(temp_root: &str, name: &str) -> String {
    join_path(&runtime_temp_dir(temp_root), name)
}

pub fn task_temp_dir(temp_root: &str, task_id: i64) -> String {
    join_path(&join_path(temp_root, "tasks"), &task_id.to_string())
}

pub fn task_token_temp_dir(temp_root: &str, task_id: i64, processing_token: i64) -> String {
    join_path(
        &task_temp_dir(temp_root, task_id),
        &processing_token.to_string(),
    )
}

pub async fn ensure_runtime_dirs(temp_dir: &str) -> Result<()> {
    tokio::fs::create_dir_all(runtime_temp_dir(temp_dir))
        .await
        .map_err(|error| {
            AsterError::config_error(format!("failed to create runtime temp dir: {error}"))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_CONFIG_SQLITE_DATABASE_URL, DEFAULT_TEMP_DIR, resolve_config_relative_path,
        resolve_config_relative_sqlite_url, runtime_temp_dir, runtime_temp_file_path,
        task_temp_dir, task_token_temp_dir, temp_file_path,
    };
    use std::path::Path;

    fn assert_no_double_slash(path: &str) {
        assert!(
            !path.contains("//"),
            "path should not contain double slashes: {path}"
        );
    }

    #[test]
    fn temp_file_path_joins_normal_inputs() {
        let path = temp_file_path(DEFAULT_TEMP_DIR, "abc123");
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
        let path = temp_file_path("/tmp///", "///runtime.bin");
        assert_eq!(path, "/tmp/runtime.bin");
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
    fn empty_root_and_leaf_do_not_generate_slashes() {
        let path = temp_file_path("", "");
        assert_eq!(path, "");
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

    #[tokio::test]
    async fn ensure_runtime_dirs_creates_runtime_subdir() {
        let root =
            std::env::temp_dir().join(format!("asterforge-runtime-dirs-{}", uuid::Uuid::new_v4()));
        let root = root.to_string_lossy().to_string();

        super::ensure_runtime_dirs(&root).await.unwrap();

        assert!(Path::new(&runtime_temp_dir(&root)).is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_config_relative_path_accepts_plain_and_data_prefixed_relative_values() {
        let base_dir = Path::new("/srv/asterforge");
        let config_dir = Path::new("/srv/asterforge/data");

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
    fn resolve_config_relative_sqlite_url_accepts_plain_and_data_prefixed_relative_values() {
        let base_dir = Path::new("/srv/asterforge");
        let config_dir = Path::new("/srv/asterforge/data");

        assert_eq!(
            resolve_config_relative_sqlite_url(
                base_dir,
                config_dir,
                DEFAULT_CONFIG_SQLITE_DATABASE_URL
            )
            .unwrap(),
            "sqlite://data/asterforge.db?mode=rwc"
        );
        assert_eq!(
            resolve_config_relative_sqlite_url(
                base_dir,
                config_dir,
                "sqlite://data/asterforge.db?mode=rwc"
            )
            .unwrap(),
            "sqlite://data/asterforge.db?mode=rwc"
        );
        assert_eq!(
            resolve_config_relative_sqlite_url(
                base_dir,
                config_dir,
                "sqlite:///var/lib/asterforge/custom.db?mode=rwc"
            )
            .unwrap(),
            "sqlite:///var/lib/asterforge/custom.db?mode=rwc"
        );
    }

    #[test]
    fn resolve_config_relative_path_rejects_values_outside_base_dir() {
        let base_dir = Path::new("/srv/asterforge");
        let config_dir = Path::new("/srv/asterforge/data");

        let error = resolve_config_relative_path(base_dir, config_dir, "../../shared")
            .expect_err("path outside base_dir should be rejected");
        assert!(error.to_string().contains("outside data base_dir"));
    }

    #[test]
    fn resolve_config_relative_sqlite_url_rejects_values_outside_base_dir() {
        let base_dir = Path::new("/srv/asterforge");
        let config_dir = Path::new("/srv/asterforge/data");

        let error = resolve_config_relative_sqlite_url(
            base_dir,
            config_dir,
            "sqlite://../../shared/asterforge.db?mode=rwc",
        )
        .expect_err("sqlite path outside base_dir should be rejected");
        assert!(error.to_string().contains("outside data base_dir"));
    }
}
