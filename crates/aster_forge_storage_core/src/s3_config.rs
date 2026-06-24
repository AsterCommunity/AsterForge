//! S3-compatible endpoint and bucket normalization helpers.
//!
//! S3-compatible providers often encode the bucket either in configuration or in the endpoint URL.
//! This module extracts a consistent bucket and endpoint pair while rejecting ambiguous or malformed
//! values before a storage driver attempts to connect.

use http::Uri;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Normalized S3-compatible endpoint and bucket values.
pub struct NormalizedS3Config {
    /// Endpoint URL, or an empty string when the provider default endpoint should be used.
    pub endpoint: String,
    /// Bucket name with surrounding whitespace removed.
    pub bucket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Errors returned while normalizing S3-compatible configuration.
pub enum S3ConfigError {
    /// The bucket field is required but was empty.
    MissingBucket,
    /// The endpoint URL was malformed or used an unsupported scheme.
    InvalidEndpoint(String),
}

/// Normalizes and validates an S3-compatible endpoint and bucket pair.
pub fn normalize_s3_endpoint_and_bucket(
    endpoint: &str,
    bucket: &str,
) -> std::result::Result<NormalizedS3Config, S3ConfigError> {
    let endpoint = endpoint.trim();
    let bucket = bucket.trim().to_string();

    if endpoint.is_empty() {
        if bucket.is_empty() {
            return Err(S3ConfigError::MissingBucket);
        }

        return Ok(NormalizedS3Config {
            endpoint: String::new(),
            bucket,
        });
    }

    let uri: Uri = endpoint.parse().map_err(|_| {
        S3ConfigError::InvalidEndpoint(format!("invalid S3 endpoint URL: '{endpoint}'"))
    })?;

    let scheme = uri.scheme_str().ok_or_else(|| {
        S3ConfigError::InvalidEndpoint(format!(
            "S3 endpoint must include http:// or https://: '{endpoint}'"
        ))
    })?;
    if scheme != "http" && scheme != "https" {
        return Err(S3ConfigError::InvalidEndpoint(format!(
            "S3 endpoint must use http:// or https://: '{endpoint}'"
        )));
    }

    uri.authority().ok_or_else(|| {
        S3ConfigError::InvalidEndpoint(format!("S3 endpoint must include a hostname: '{endpoint}'"))
    })?;

    if bucket.is_empty() {
        return Err(S3ConfigError::MissingBucket);
    }

    Ok(NormalizedS3Config {
        endpoint: endpoint.to_string(),
        bucket,
    })
}

#[cfg(test)]
mod tests {
    use super::{S3ConfigError, normalize_s3_endpoint_and_bucket};

    #[test]
    fn allows_standard_s3_endpoint_without_rewriting() {
        let normalized =
            normalize_s3_endpoint_and_bucket("https://s3.example.com/custom/path", "archive")
                .expect("normalized S3 config");

        assert_eq!(normalized.endpoint, "https://s3.example.com/custom/path");
        assert_eq!(normalized.bucket, "archive");
    }

    #[test]
    fn rejects_missing_bucket_for_any_s3_compatible_endpoint() {
        assert_eq!(
            normalize_s3_endpoint_and_bucket("https://s3.example.com", "")
                .expect_err("missing bucket should fail"),
            S3ConfigError::MissingBucket
        );
    }

    #[test]
    fn allows_empty_endpoint_when_bucket_is_present() {
        let normalized =
            normalize_s3_endpoint_and_bucket("   ", " archive ").expect("bucket-only config");

        assert_eq!(normalized.endpoint, "");
        assert_eq!(normalized.bucket, "archive");
    }

    #[test]
    fn rejects_endpoint_without_http_scheme_or_host() {
        assert!(matches!(
            normalize_s3_endpoint_and_bucket("s3.example.com", "archive"),
            Err(S3ConfigError::InvalidEndpoint(_))
        ));
        assert!(matches!(
            normalize_s3_endpoint_and_bucket("ftp://s3.example.com", "archive"),
            Err(S3ConfigError::InvalidEndpoint(_))
        ));
        assert!(matches!(
            normalize_s3_endpoint_and_bucket("https:///missing-host", "archive"),
            Err(S3ConfigError::InvalidEndpoint(_))
        ));
    }
}
