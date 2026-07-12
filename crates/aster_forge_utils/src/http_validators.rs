//! Transport-neutral HTTP conditional request helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Errors produced while parsing HTTP validators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpValidatorError {
    /// A comma-separated ETag list did not contain an entity tag.
    #[error("ETag list must contain at least one entity tag")]
    EmptyEtagList,
    /// A value was not a valid HTTP date.
    #[error("invalid HTTP date")]
    InvalidHttpDate,
}

/// Formats a system time as an IMF-fixdate HTTP date.
pub fn format_http_date(time: SystemTime) -> String {
    httpdate::fmt_http_date(time)
}

/// Parses an HTTP date into system time.
pub fn parse_http_date(value: &str) -> Result<SystemTime, HttpValidatorError> {
    httpdate::parse_http_date(value).map_err(|_| HttpValidatorError::InvalidHttpDate)
}

/// Returns whole seconds relative to the Unix epoch, preserving pre-epoch ordering.
pub fn http_date_epoch_seconds(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::from(duration.as_secs()),
        Err(error) => -i128::from(error.duration().as_secs()),
    }
}

/// Applies the strong comparison required by `If-Match`.
pub fn if_match_header_matches(
    raw: &str,
    resource_exists: bool,
    current_etag: Option<&str>,
) -> Result<bool, HttpValidatorError> {
    let raw = raw.trim();
    if raw == "*" {
        return Ok(resource_exists);
    }
    let Some(current_etag) = current_etag else {
        return Ok(false);
    };
    let mut saw_tag = false;
    for candidate in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        saw_tag = true;
        if is_weak_etag(candidate) {
            continue;
        }
        if strong_etag_matches(candidate, current_etag) {
            return Ok(true);
        }
    }
    if saw_tag {
        Ok(false)
    } else {
        Err(HttpValidatorError::EmptyEtagList)
    }
}

/// Applies the weak comparison required by `If-None-Match`.
pub fn if_none_match_header_matches(
    raw: &str,
    resource_exists: bool,
    current_etag: Option<&str>,
) -> Result<bool, HttpValidatorError> {
    let raw = raw.trim();
    if raw == "*" {
        return Ok(resource_exists);
    }
    let Some(current_etag) = current_etag else {
        return Ok(false);
    };
    let mut saw_tag = false;
    for candidate in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        saw_tag = true;
        if etag_matches(candidate, current_etag) {
            return Ok(true);
        }
    }
    if saw_tag {
        Ok(false)
    } else {
        Err(HttpValidatorError::EmptyEtagList)
    }
}

fn etag_matches(header_value: &str, current_etag: &str) -> bool {
    let header_value = strip_weak_etag_prefix(header_value.trim());
    let current = strip_weak_etag_prefix(current_etag.trim());
    strip_etag_quotes(header_value) == strip_etag_quotes(current)
}

fn strong_etag_matches(candidate: &str, current_etag: &str) -> bool {
    if is_weak_etag(current_etag) {
        return false;
    }
    strip_etag_quotes(candidate.trim()) == strip_etag_quotes(current_etag.trim())
}

fn is_weak_etag(value: &str) -> bool {
    value
        .trim()
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("W/"))
}

fn strip_weak_etag_prefix(value: &str) -> &str {
    value
        .strip_prefix("W/")
        .or_else(|| value.strip_prefix("w/"))
        .unwrap_or(value)
}

fn strip_etag_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{
        HttpValidatorError, format_http_date, http_date_epoch_seconds, if_match_header_matches,
        if_none_match_header_matches, parse_http_date,
    };

    #[test]
    fn if_none_match_uses_weak_comparison() {
        assert_eq!(
            if_none_match_header_matches(r#"W/"etag-1", "etag-2""#, true, Some(r#""etag-1""#)),
            Ok(true)
        );
    }

    #[test]
    fn if_match_requires_strong_comparison() {
        assert_eq!(
            if_match_header_matches(r#"W/"etag-1""#, true, Some(r#""etag-1""#)),
            Ok(false)
        );
        assert_eq!(
            if_match_header_matches(r#""etag-1""#, true, Some(r#""etag-1""#)),
            Ok(true)
        );
        assert_eq!(
            if_match_header_matches(r#""etag-1""#, true, Some(r#"W/"etag-1""#)),
            Ok(false)
        );
    }

    #[test]
    fn wildcard_respects_resource_existence() {
        assert_eq!(if_match_header_matches("*", true, None), Ok(true));
        assert_eq!(if_match_header_matches("*", false, None), Ok(false));
        assert_eq!(if_none_match_header_matches("*", true, None), Ok(true));
        assert_eq!(if_none_match_header_matches("*", false, None), Ok(false));
    }

    #[test]
    fn empty_etag_lists_are_invalid() {
        assert_eq!(
            if_none_match_header_matches(" , ", true, Some("etag")),
            Err(HttpValidatorError::EmptyEtagList)
        );
        assert_eq!(
            if_match_header_matches(" , ", true, Some("etag")),
            Err(HttpValidatorError::EmptyEtagList)
        );
    }

    #[test]
    fn http_dates_round_trip_and_reject_invalid_values() {
        let time = UNIX_EPOCH + Duration::from_secs(784_111_777);
        let formatted = format_http_date(time);

        assert_eq!(formatted, "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(parse_http_date(&formatted), Ok(time));
        assert_eq!(
            parse_http_date("not a date"),
            Err(HttpValidatorError::InvalidHttpDate)
        );
    }

    #[test]
    fn epoch_seconds_preserve_pre_epoch_ordering() {
        assert_eq!(http_date_epoch_seconds(UNIX_EPOCH), 0);
        assert_eq!(
            http_date_epoch_seconds(UNIX_EPOCH + Duration::from_secs(2)),
            2
        );
        assert_eq!(
            http_date_epoch_seconds(UNIX_EPOCH - Duration::from_secs(2)),
            -2
        );
    }
}
