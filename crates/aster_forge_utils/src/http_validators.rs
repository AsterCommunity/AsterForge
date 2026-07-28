//! Transport-neutral HTTP conditional request helpers.

use std::time::{SystemTime, UNIX_EPOCH};

use headers::{ETag, Header, IfMatch, IfNoneMatch};
use http::header::{IF_MATCH, IF_NONE_MATCH};
use http::{HeaderMap, HeaderValue};

const MAX_ETAG_LIST_ELEMENTS: usize = 128;
const MAX_HTTP_DATE_EPOCH_SECONDS: u64 = 253_402_300_799;

/// Errors produced while parsing HTTP validators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpValidatorError {
    /// An entity-tag list contained a malformed tag or mixed `*` with tags.
    #[error("invalid ETag list")]
    InvalidEtagList,
    /// A value was not a valid HTTP date.
    #[error("invalid HTTP date")]
    InvalidHttpDate,
}

/// Formats a system time as an IMF-fixdate HTTP date.
pub fn format_http_date(time: SystemTime) -> String {
    httpdate::fmt_http_date(time)
}

/// Formats a system time when it is representable by the HTTP-date implementation.
pub fn try_format_http_date(time: SystemTime) -> Result<String, HttpValidatorError> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| HttpValidatorError::InvalidHttpDate)?;
    if duration.as_secs() > MAX_HTTP_DATE_EPOCH_SECONDS {
        return Err(HttpValidatorError::InvalidHttpDate);
    }
    Ok(httpdate::fmt_http_date(time))
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
    match parse_etag_list(raw.as_bytes())? {
        ParsedEtagList::Any => Ok(resource_exists),
        ParsedEtagList::Tags(candidates) => strong_candidates_match(&candidates, current_etag),
    }
}

/// Applies `If-Match` to all field lines in an HTTP header map.
pub fn if_match_headers_match(
    headers: &HeaderMap,
    resource_exists: bool,
    current_etag: Option<&str>,
) -> Result<Option<bool>, HttpValidatorError> {
    let Some(raw) = combined_header_bytes(headers, IF_MATCH) else {
        return Ok(None);
    };
    match parse_etag_list(&raw)? {
        ParsedEtagList::Any => Ok(Some(resource_exists)),
        ParsedEtagList::Tags(candidates) => {
            strong_candidates_match(&candidates, current_etag).map(Some)
        }
    }
}

fn strong_candidates_match(
    candidates: &[ETag],
    current_etag: Option<&str>,
) -> Result<bool, HttpValidatorError> {
    let Some(current_etag) = current_etag else {
        return Ok(false);
    };
    let current = parse_entity_tag(current_etag)?;
    Ok(candidates
        .iter()
        .any(|candidate| IfMatch::from(candidate.clone()).precondition_passes(&current)))
}

/// Applies the weak comparison required by `If-None-Match`.
pub fn if_none_match_header_matches(
    raw: &str,
    resource_exists: bool,
    current_etag: Option<&str>,
) -> Result<bool, HttpValidatorError> {
    match parse_etag_list(raw.as_bytes())? {
        ParsedEtagList::Any => Ok(resource_exists),
        ParsedEtagList::Tags(candidates) => weak_candidates_match(&candidates, current_etag),
    }
}

/// Applies `If-None-Match` to all field lines in an HTTP header map.
pub fn if_none_match_headers_match(
    headers: &HeaderMap,
    resource_exists: bool,
    current_etag: Option<&str>,
) -> Result<Option<bool>, HttpValidatorError> {
    let Some(raw) = combined_header_bytes(headers, IF_NONE_MATCH) else {
        return Ok(None);
    };
    match parse_etag_list(&raw)? {
        ParsedEtagList::Any => Ok(Some(resource_exists)),
        ParsedEtagList::Tags(candidates) => {
            weak_candidates_match(&candidates, current_etag).map(Some)
        }
    }
}

fn weak_candidates_match(
    candidates: &[ETag],
    current_etag: Option<&str>,
) -> Result<bool, HttpValidatorError> {
    let Some(current_etag) = current_etag else {
        return Ok(false);
    };
    let current = parse_entity_tag(current_etag)?;
    Ok(candidates
        .iter()
        .any(|candidate| !IfNoneMatch::from(candidate.clone()).precondition_passes(&current)))
}

enum ParsedEtagList {
    Any,
    Tags(Vec<ETag>),
}

fn parse_etag_list(raw: &[u8]) -> Result<ParsedEtagList, HttpValidatorError> {
    let trimmed = trim_ows(raw);
    if trimmed == b"*" {
        return Ok(ParsedEtagList::Any);
    }
    let mut tags = Vec::new();
    let mut remaining = raw;
    let mut elements = 0_usize;
    loop {
        remaining = trim_start_ows(remaining);
        while let Some(rest) = remaining.strip_prefix(b",") {
            elements = elements.saturating_add(1);
            if elements > MAX_ETAG_LIST_ELEMENTS {
                return Err(HttpValidatorError::InvalidEtagList);
            }
            remaining = trim_start_ows(rest);
        }
        if remaining.is_empty() {
            break;
        }
        elements = elements.saturating_add(1);
        if elements > MAX_ETAG_LIST_ELEMENTS {
            return Err(HttpValidatorError::InvalidEtagList);
        }
        let weak = remaining.starts_with(b"W/");
        let quoted = if weak {
            remaining.get(2..)
        } else {
            Some(remaining)
        }
        .and_then(|value| value.strip_prefix(b"\""))
        .ok_or(HttpValidatorError::InvalidEtagList)?;
        let closing = quoted
            .iter()
            .position(|byte| *byte == b'"')
            .ok_or(HttpValidatorError::InvalidEtagList)?;
        let consumed = closing + 2 + usize::from(weak) * 2;
        let candidate = remaining
            .get(..consumed)
            .ok_or(HttpValidatorError::InvalidEtagList)?;
        tags.push(parse_entity_tag_bytes(candidate)?);
        remaining = remaining
            .get(consumed..)
            .ok_or(HttpValidatorError::InvalidEtagList)?;
        let trimmed = trim_start_ows(remaining);
        if trimmed.is_empty() {
            break;
        }
        remaining = trimmed
            .strip_prefix(b",")
            .ok_or(HttpValidatorError::InvalidEtagList)?;
    }
    Ok(ParsedEtagList::Tags(tags))
}

fn parse_entity_tag(value: &str) -> Result<ETag, HttpValidatorError> {
    let value = value.trim();
    let rendered = if value.starts_with('"') || value.starts_with("W/\"") {
        value.to_owned()
    } else {
        format!("\"{value}\"")
    };
    rendered
        .parse()
        .map_err(|_| HttpValidatorError::InvalidEtagList)
}

fn parse_entity_tag_bytes(value: &[u8]) -> Result<ETag, HttpValidatorError> {
    let value = HeaderValue::from_bytes(value).map_err(|_| HttpValidatorError::InvalidEtagList)?;
    ETag::decode(&mut std::iter::once(&value)).map_err(|_| HttpValidatorError::InvalidEtagList)
}

fn combined_header_bytes(headers: &HeaderMap, name: http::header::HeaderName) -> Option<Vec<u8>> {
    let mut combined = Vec::new();
    let mut present = false;
    for value in headers.get_all(name).iter() {
        if present {
            combined.push(b',');
        }
        present = true;
        combined.extend_from_slice(value.as_bytes());
    }
    present.then_some(combined)
}

fn trim_ows(value: &[u8]) -> &[u8] {
    let value = trim_start_ows(value);
    let end = value
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(0, |index| index + 1);
    &value[..end]
}

fn trim_start_ows(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(value.len());
    &value[start..]
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{
        HttpValidatorError, MAX_ETAG_LIST_ELEMENTS, MAX_HTTP_DATE_EPOCH_SECONDS, format_http_date,
        http_date_epoch_seconds, if_match_header_matches, if_match_headers_match,
        if_none_match_header_matches, if_none_match_headers_match, parse_http_date,
        try_format_http_date,
    };
    use http::header::{IF_MATCH, IF_NONE_MATCH};
    use http::{HeaderMap, HeaderValue};

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
    fn opaque_backend_etags_that_start_with_weak_marker_text_are_quoted() {
        assert_eq!(
            if_match_header_matches(r#""W/backend-value""#, true, Some("W/backend-value")),
            Ok(true)
        );
        assert_eq!(
            if_none_match_header_matches(r#""W/backend-value""#, true, Some("W/backend-value")),
            Ok(true)
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
    fn empty_etag_lists_use_zero_member_rfc_semantics() {
        assert_eq!(
            if_none_match_header_matches(" , ", true, Some("etag")),
            Ok(false)
        );
        assert_eq!(
            if_match_header_matches(" , ", true, Some("etag")),
            Ok(false)
        );

        let mut headers = HeaderMap::new();
        headers.insert(IF_MATCH, HeaderValue::from_static(""));
        assert_eq!(
            if_match_headers_match(&headers, true, Some("etag")),
            Ok(Some(false))
        );

        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static(""));
        assert_eq!(
            if_none_match_headers_match(&headers, true, Some("etag")),
            Ok(Some(false))
        );
    }

    #[test]
    fn malformed_etag_lists_are_invalid() {
        for raw in [r#"etag-1"#, r#"*, "etag-1""#, r#""unterminated"#] {
            assert_eq!(
                if_none_match_header_matches(raw, true, Some(r#""etag-1""#)),
                Err(HttpValidatorError::InvalidEtagList),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn recipient_list_parsing_ignores_empty_members_and_preserves_opaque_commas() {
        for raw in [
            r#", "etag-1""#,
            r#""etag-1","#,
            r#", , "etag-1", ,"#,
            r#""opaque,comma", "etag-1""#,
        ] {
            assert_eq!(
                if_none_match_header_matches(raw, true, Some(r#""etag-1""#)),
                Ok(true),
                "{raw:?}"
            );
        }
        assert_eq!(
            if_match_header_matches(
                r#""opaque,comma", "other""#,
                true,
                Some(r#""opaque,comma""#),
            ),
            Ok(true)
        );
    }

    #[test]
    fn repeated_field_lines_are_combined_as_one_rfc_list() {
        let mut headers = HeaderMap::new();
        headers.append(IF_MATCH, HeaderValue::from_static("\"other\""));
        headers.append(IF_MATCH, HeaderValue::from_static("\"etag-1\""));
        assert_eq!(
            if_match_headers_match(&headers, true, Some("etag-1")),
            Ok(Some(true))
        );

        let mut headers = HeaderMap::new();
        headers.append(IF_NONE_MATCH, HeaderValue::from_static("\"other\""));
        headers.append(IF_NONE_MATCH, HeaderValue::from_static("W/\"etag-1\""));
        assert_eq!(
            if_none_match_headers_match(&headers, true, Some("etag-1")),
            Ok(Some(true))
        );
    }

    #[test]
    fn obs_text_is_valid_inside_an_opaque_tag() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IF_MATCH,
            HeaderValue::from_bytes(&[b'"', 0xff, b'"']).expect("obs-text header"),
        );
        assert_eq!(
            if_match_headers_match(&headers, true, Some("etag-1")),
            Ok(Some(false))
        );
    }

    #[test]
    fn reasonable_empty_members_are_bounded() {
        let accepted = ",".repeat(MAX_ETAG_LIST_ELEMENTS - 1) + "\"etag-1\"";
        assert_eq!(
            if_match_header_matches(&accepted, true, Some("etag-1")),
            Ok(true)
        );

        let rejected = ",".repeat(MAX_ETAG_LIST_ELEMENTS) + "\"etag-1\"";
        assert_eq!(
            if_match_header_matches(&rejected, true, Some("etag-1")),
            Err(HttpValidatorError::InvalidEtagList)
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
        assert_eq!(try_format_http_date(time), Ok(formatted));
        assert_eq!(
            try_format_http_date(UNIX_EPOCH - Duration::from_secs(1)),
            Err(HttpValidatorError::InvalidHttpDate)
        );
        assert_eq!(
            try_format_http_date(UNIX_EPOCH + Duration::from_secs(MAX_HTTP_DATE_EPOCH_SECONDS + 1),),
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
