//! General-purpose utilities.

pub mod crypto;
pub mod email;
pub mod hash;
pub mod id;
pub mod numbers;
pub mod paths;
pub mod raii;

pub fn truncate_utf8_to_max_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

pub async fn cleanup_temp_dir(path: &str) {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(path, error = %error, "failed to cleanup temp dir"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "asterforge-utils-{label}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn truncate_utf8_to_max_bytes_keeps_short_ascii_unchanged() {
        assert_eq!(
            super::truncate_utf8_to_max_bytes("AsterForge", 32),
            "AsterForge"
        );
    }

    #[test]
    fn truncate_utf8_to_max_bytes_truncates_ascii_by_bytes() {
        assert_eq!(super::truncate_utf8_to_max_bytes("AsterForge", 5), "Aster");
    }

    #[test]
    fn truncate_utf8_to_max_bytes_preserves_char_boundaries() {
        assert_eq!(super::truncate_utf8_to_max_bytes("你好世界", 7), "你好");
        assert_eq!(super::truncate_utf8_to_max_bytes("éclair", 1), "");
        assert_eq!(super::truncate_utf8_to_max_bytes("éclair", 2), "é");
    }

    #[test]
    fn truncate_utf8_to_max_bytes_handles_zero_limit() {
        assert_eq!(super::truncate_utf8_to_max_bytes("AsterForge", 0), "");
        assert_eq!(super::truncate_utf8_to_max_bytes("你好", 0), "");
    }

    #[tokio::test]
    async fn cleanup_temp_dir_removes_directory_tree() {
        let path = unique_temp_path("cleanup");
        let nested = path.join("nested");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        tokio::fs::write(nested.join("payload.txt"), b"temporary")
            .await
            .unwrap();

        super::cleanup_temp_dir(path.to_str().unwrap()).await;

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_temp_dir_tolerates_missing_directory() {
        let path = unique_temp_path("missing-cleanup");

        super::cleanup_temp_dir(path.to_str().unwrap()).await;

        assert!(!path.exists());
    }
}
