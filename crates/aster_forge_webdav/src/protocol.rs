//! WebDAV header parsing and protocol precondition rules.

use std::time::SystemTime;

use http::header::{self, HeaderMap, HeaderValue};
use http::{StatusCode, Uri};

use crate::DavPath;
use aster_forge_utils::http_validators;

/// WebDAV `Depth` header value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The request target only.
    Zero,
    /// The target and its immediate children.
    One,
    /// The complete descendant tree.
    Infinity,
}

impl Depth {
    /// Returns whether this depth traverses all descendants.
    #[must_use]
    pub fn is_infinity(self) -> bool {
        matches!(self, Self::Infinity)
    }
}

/// A parsed `Destination` header restricted to the current origin and mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// Canonical path relative to the WebDAV mount.
    pub path: DavPath,
    /// Decoded relative path retained for product adapters.
    pub relative: String,
}

/// A parsed WebDAV `If` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfHeader {
    /// Resource-tagged or untagged condition groups.
    pub groups: Vec<IfResourceGroup>,
}

/// Conditions associated with one tagged resource or the request target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfResourceGroup {
    /// Tagged resource URI, or `None` for the request target.
    pub tagged_path: Option<String>,
    /// OR-connected state lists for this resource.
    pub lists: Vec<IfStateList>,
}

/// AND-connected conditions inside one parenthesized state list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfStateList {
    /// State token and entity-tag conditions.
    pub conditions: Vec<IfStateCondition>,
}

/// One WebDAV `If` condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IfStateCondition {
    /// A lock state token condition.
    Token { value: String, negated: bool },
    /// An entity-tag condition.
    Etag { value: String, negated: bool },
}

/// Outcome of HTTP entity-tag and modification-date precondition evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavPrecondition {
    /// Continue processing the request.
    Proceed,
    /// Return `304 Not Modified` for a safe method.
    NotModified,
}

/// Stable protocol error classification for transport adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavProtocolErrorKind {
    BadRequest,
    PreconditionFailed,
}

/// A product-neutral WebDAV protocol error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DavProtocolError {
    kind: DavProtocolErrorKind,
    status: StatusCode,
    message: &'static str,
}

impl DavProtocolError {
    /// Returns the stable error category.
    #[must_use]
    pub fn kind(&self) -> DavProtocolErrorKind {
        self.kind
    }

    /// Returns the HTTP status required by the protocol boundary.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns a protocol-level response message.
    #[must_use]
    pub fn message(&self) -> &'static str {
        self.message
    }

    pub(crate) fn bad_request(message: &'static str) -> Self {
        Self {
            kind: DavProtocolErrorKind::BadRequest,
            status: StatusCode::BAD_REQUEST,
            message,
        }
    }

    fn precondition_failed() -> Self {
        Self {
            kind: DavProtocolErrorKind::PreconditionFailed,
            status: StatusCode::PRECONDITION_FAILED,
            message: "Precondition failed",
        }
    }
}

/// Parses the `Depth` semantics used by `PROPFIND`.
pub fn parse_propfind_depth(headers: &HeaderMap) -> Result<Depth, DavProtocolError> {
    match parse_depth_header(headers)? {
        Some(Depth::Zero) => Ok(Depth::Zero),
        Some(Depth::One) => Ok(Depth::One),
        Some(Depth::Infinity) | None => Ok(Depth::Infinity),
    }
}

/// Parses the `Depth` semantics used by `COPY`.
pub fn parse_copy_depth(headers: &HeaderMap) -> Result<Depth, DavProtocolError> {
    match parse_depth_header(headers)? {
        Some(Depth::Zero) => Ok(Depth::Zero),
        Some(Depth::Infinity) | None => Ok(Depth::Infinity),
        Some(Depth::One) => Err(DavProtocolError::bad_request("Invalid Depth header")),
    }
}

/// Parses the `Depth` semantics used by `MOVE`.
pub fn parse_move_depth(headers: &HeaderMap) -> Result<Depth, DavProtocolError> {
    Ok(parse_depth_header(headers)?.unwrap_or(Depth::Infinity))
}

/// Parses the `Depth` semantics used by `DELETE`.
pub fn parse_delete_depth(headers: &HeaderMap) -> Result<Depth, DavProtocolError> {
    Ok(parse_depth_header(headers)?.unwrap_or(Depth::Infinity))
}

/// Parses the `Depth` semantics used by `LOCK`.
pub fn parse_lock_depth(headers: &HeaderMap) -> Result<Depth, DavProtocolError> {
    match parse_depth_header(headers)? {
        None | Some(Depth::Infinity) => Ok(Depth::Infinity),
        Some(Depth::Zero) => Ok(Depth::Zero),
        Some(Depth::One) => Err(DavProtocolError::bad_request("Invalid Depth header")),
    }
}

fn parse_depth_header(headers: &HeaderMap) -> Result<Option<Depth>, DavProtocolError> {
    let Some(value) = headers.get("Depth") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| DavProtocolError::bad_request("Invalid Depth header"))?;

    match value {
        value if value.eq_ignore_ascii_case("0") => Ok(Some(Depth::Zero)),
        value if value.eq_ignore_ascii_case("1") => Ok(Some(Depth::One)),
        value if value.eq_ignore_ascii_case("infinity") => Ok(Some(Depth::Infinity)),
        _ => Err(DavProtocolError::bad_request("Invalid Depth header")),
    }
}

/// Parses `Overwrite`, defaulting to `true` when the header is absent.
pub fn parse_overwrite(headers: &HeaderMap) -> Result<bool, DavProtocolError> {
    let Some(value) = headers.get("Overwrite") else {
        return Ok(true);
    };
    let value = value
        .to_str()
        .map_err(|_| DavProtocolError::bad_request("Invalid Overwrite header"))?
        .trim();
    if value.eq_ignore_ascii_case("T") {
        Ok(true)
    } else if value.eq_ignore_ascii_case("F") {
        Ok(false)
    } else {
        Err(DavProtocolError::bad_request("Invalid Overwrite header"))
    }
}

/// Parses and constrains `Destination` to the current origin and WebDAV mount.
pub fn destination_relative_path(
    headers: &HeaderMap,
    prefix: &str,
    request_scheme: &str,
    request_host: &str,
) -> Result<Destination, DavProtocolError> {
    let raw = headers
        .get("Destination")
        .ok_or_else(|| DavProtocolError::bad_request("Missing Destination header"))?
        .to_str()
        .map_err(|_| DavProtocolError::bad_request("Invalid Destination header"))?
        .trim();
    let uri: Uri = raw
        .parse()
        .map_err(|_| DavProtocolError::bad_request("Invalid Destination header"))?;
    match (uri.scheme_str(), uri.authority()) {
        (Some(scheme), Some(authority)) => {
            if !scheme.eq_ignore_ascii_case(request_scheme)
                || !authority.as_str().eq_ignore_ascii_case(request_host)
            {
                return Err(DavProtocolError::bad_request(
                    "Destination must stay on this WebDAV server",
                ));
            }
        }
        (None, None) => {
            if !raw.starts_with('/') {
                return Err(DavProtocolError::bad_request("Invalid Destination header"));
            }
        }
        _ => return Err(DavProtocolError::bad_request("Invalid Destination header")),
    }

    let path = uri.path();
    let relative = strip_mount_prefix(path, prefix).ok_or_else(|| {
        DavProtocolError::bad_request("Destination must stay under WebDAV prefix")
    })?;
    let path = DavPath::new(relative)
        .map_err(|_| DavProtocolError::bad_request("Invalid Destination header"))?;
    let relative = path.as_str().to_string();
    Ok(Destination { path, relative })
}

/// Parses a WebDAV `If` header.
pub fn parse_if_header(headers: &HeaderMap) -> Result<Option<IfHeader>, DavProtocolError> {
    let Some(value) = headers.get("If") else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .map_err(|_| DavProtocolError::bad_request("Invalid If header"))?;
    IfHeaderParser::new(raw).parse().map(Some)
}

/// Extracts submitted lock tokens that apply to one request path.
pub fn submitted_lock_tokens_for_path(
    headers: &HeaderMap,
    request_path: &str,
    request_scheme: &str,
    request_host: &str,
) -> Vec<String> {
    let Some(if_header) = parse_if_header(headers).ok().flatten() else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    for group in &if_header.groups {
        match group.tagged_path.as_deref() {
            None => {}
            Some(tagged_path)
                if if_tag_matches_path(tagged_path, request_path, request_scheme, request_host) => {
            }
            Some(_) => continue,
        }
        for list in &group.lists {
            for condition in &list.conditions {
                if let IfStateCondition::Token { value, .. } = condition {
                    tokens.push(value.clone());
                }
            }
        }
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

/// Evaluates `If-Match` and `If-None-Match` for a resource operation.
pub fn evaluate_http_etag_preconditions(
    headers: &HeaderMap,
    resource_exists: bool,
    current_etag: Option<&str>,
    safe_method: bool,
) -> Result<DavPrecondition, DavProtocolError> {
    if let Some(value) = headers.get(header::IF_MATCH) {
        let raw = value
            .to_str()
            .map_err(|_| DavProtocolError::bad_request("Invalid If-Match header"))?;
        if !http_validators::if_match_header_matches(raw, resource_exists, current_etag)
            .map_err(|_| DavProtocolError::bad_request("Invalid If-Match header"))?
        {
            return Err(DavProtocolError::precondition_failed());
        }
    }

    if let Some(value) = headers.get(header::IF_NONE_MATCH) {
        let raw = value
            .to_str()
            .map_err(|_| DavProtocolError::bad_request("Invalid If-None-Match header"))?;
        if http_validators::if_none_match_header_matches(raw, resource_exists, current_etag)
            .map_err(|_| DavProtocolError::bad_request("Invalid If-None-Match header"))?
        {
            return if safe_method {
                Ok(DavPrecondition::NotModified)
            } else {
                Err(DavProtocolError::precondition_failed())
            };
        }
    }

    Ok(DavPrecondition::Proceed)
}

/// Evaluates RFC 7232 download preconditions in their required precedence order.
pub fn evaluate_http_download_preconditions(
    headers: &HeaderMap,
    current_etag: Option<&str>,
    last_modified: Option<SystemTime>,
) -> Result<DavPrecondition, DavProtocolError> {
    let has_if_match = headers.contains_key(header::IF_MATCH);
    if let Some(value) = headers.get(header::IF_MATCH) {
        let raw = value
            .to_str()
            .map_err(|_| DavProtocolError::bad_request("Invalid If-Match header"))?;
        if !http_validators::if_match_header_matches(raw, true, current_etag)
            .map_err(|_| DavProtocolError::bad_request("Invalid If-Match header"))?
        {
            return Err(DavProtocolError::precondition_failed());
        }
    }

    if !has_if_match
        && let (Some(value), Some(last_modified)) =
            (headers.get(header::IF_UNMODIFIED_SINCE), last_modified)
    {
        let since = parse_http_date_header(value, "Invalid If-Unmodified-Since header")?;
        if http_validators::http_date_epoch_seconds(last_modified)
            > http_validators::http_date_epoch_seconds(since)
        {
            return Err(DavProtocolError::precondition_failed());
        }
    }

    let has_if_none_match = headers.contains_key(header::IF_NONE_MATCH);
    if let Some(value) = headers.get(header::IF_NONE_MATCH) {
        let raw = value
            .to_str()
            .map_err(|_| DavProtocolError::bad_request("Invalid If-None-Match header"))?;
        if http_validators::if_none_match_header_matches(raw, true, current_etag)
            .map_err(|_| DavProtocolError::bad_request("Invalid If-None-Match header"))?
        {
            return Ok(DavPrecondition::NotModified);
        }
    }

    if !has_if_none_match
        && let (Some(value), Some(last_modified)) =
            (headers.get(header::IF_MODIFIED_SINCE), last_modified)
    {
        let since = parse_http_date_header(value, "Invalid If-Modified-Since header")?;
        if http_validators::http_date_epoch_seconds(last_modified)
            <= http_validators::http_date_epoch_seconds(since)
        {
            return Ok(DavPrecondition::NotModified);
        }
    }

    Ok(DavPrecondition::Proceed)
}

fn parse_http_date_header(
    value: &HeaderValue,
    invalid_message: &'static str,
) -> Result<SystemTime, DavProtocolError> {
    let raw = value
        .to_str()
        .map_err(|_| DavProtocolError::bad_request(invalid_message))?;
    http_validators::parse_http_date(raw)
        .map_err(|_| DavProtocolError::bad_request(invalid_message))
}

fn strip_mount_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix).filter(|_| {
        prefix == "/"
            || path == prefix
            || path
                .as_bytes()
                .get(prefix.len())
                .is_some_and(|byte| *byte == b'/')
    })
}

fn normalize_lock_token(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character| character == '<' || character == '>')
        .to_string()
}

struct IfHeaderParser<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> IfHeaderParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn parse(&mut self) -> Result<IfHeader, DavProtocolError> {
        self.skip_linear_whitespace();
        if self.is_eof() {
            return Err(DavProtocolError::bad_request("Invalid If header"));
        }

        let tagged = self.peek_char() == Some('<');
        let mut groups = Vec::new();
        if tagged {
            while !self.is_eof() {
                let tagged_path = self.parse_angle_value()?;
                let mut lists = Vec::new();
                loop {
                    self.skip_linear_whitespace();
                    if self.peek_char() != Some('(') {
                        break;
                    }
                    lists.push(self.parse_state_list()?);
                }
                if lists.is_empty() {
                    return Err(DavProtocolError::bad_request("Invalid If header"));
                }
                groups.push(IfResourceGroup {
                    tagged_path: Some(tagged_path),
                    lists,
                });
                self.skip_linear_whitespace();
                if self.is_eof() {
                    break;
                }
                if self.peek_char() != Some('<') {
                    return Err(DavProtocolError::bad_request("Invalid If header"));
                }
            }
        } else {
            let mut lists = Vec::new();
            while !self.is_eof() {
                lists.push(self.parse_state_list()?);
                self.skip_linear_whitespace();
                if self.peek_char() == Some('<') {
                    return Err(DavProtocolError::bad_request("Invalid If header"));
                }
            }
            groups.push(IfResourceGroup {
                tagged_path: None,
                lists,
            });
        }
        Ok(IfHeader { groups })
    }

    fn parse_state_list(&mut self) -> Result<IfStateList, DavProtocolError> {
        self.expect_char('(')?;
        let mut conditions = Vec::new();
        loop {
            self.skip_linear_whitespace();
            if self.peek_char() == Some(')') {
                self.position += 1;
                break;
            }
            if self.is_eof() {
                return Err(DavProtocolError::bad_request("Invalid If header"));
            }

            let negated = self.consume_not();
            self.skip_linear_whitespace();
            let condition = match self.peek_char() {
                Some('<') => IfStateCondition::Token {
                    value: normalize_lock_token(&self.parse_angle_value()?),
                    negated,
                },
                Some('[') => IfStateCondition::Etag {
                    value: self.parse_bracket_value()?,
                    negated,
                },
                _ => return Err(DavProtocolError::bad_request("Invalid If header")),
            };
            conditions.push(condition);
        }

        if conditions.is_empty() {
            return Err(DavProtocolError::bad_request("Invalid If header"));
        }
        Ok(IfStateList { conditions })
    }

    fn parse_angle_value(&mut self) -> Result<String, DavProtocolError> {
        self.parse_delimited('<', '>')
    }

    fn parse_bracket_value(&mut self) -> Result<String, DavProtocolError> {
        self.parse_delimited('[', ']')
    }

    fn parse_delimited(
        &mut self,
        opening: char,
        closing: char,
    ) -> Result<String, DavProtocolError> {
        self.expect_char(opening)?;
        let start = self.position;
        while let Some(character) = self.peek_char() {
            if character == closing {
                let value = self.input[start..self.position].trim();
                self.position += closing.len_utf8();
                if value.is_empty() {
                    return Err(DavProtocolError::bad_request("Invalid If header"));
                }
                return Ok(value.to_string());
            }
            self.position += character.len_utf8();
        }
        Err(DavProtocolError::bad_request("Invalid If header"))
    }

    fn consume_not(&mut self) -> bool {
        let rest = &self.input[self.position..];
        let Some(candidate) = rest.get(..3) else {
            return false;
        };
        if !candidate.eq_ignore_ascii_case("not") {
            return false;
        }
        let after_not = &rest[3..];
        if after_not.chars().next().is_some_and(|character| {
            !character.is_ascii_whitespace() && character != '<' && character != '['
        }) {
            return false;
        }
        self.position += 3;
        true
    }

    fn expect_char(&mut self, expected: char) -> Result<(), DavProtocolError> {
        if self.peek_char() == Some(expected) {
            self.position += expected.len_utf8();
            Ok(())
        } else {
            Err(DavProtocolError::bad_request("Invalid If header"))
        }
    }

    fn skip_linear_whitespace(&mut self) {
        while self
            .peek_char()
            .is_some_and(|character| matches!(character, ' ' | '\t' | '\r' | '\n'))
        {
            self.position += 1;
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.position..].chars().next()
    }

    fn is_eof(&self) -> bool {
        self.position >= self.input.len()
    }
}

fn if_tag_matches_path(
    tagged_path: &str,
    request_path: &str,
    request_scheme: &str,
    request_host: &str,
) -> bool {
    if path_equivalent(tagged_path, request_path) {
        return true;
    }
    let Ok(uri) = tagged_path.parse::<Uri>() else {
        return false;
    };
    match (uri.scheme_str(), uri.authority()) {
        (Some(scheme), Some(authority)) => {
            scheme.eq_ignore_ascii_case(request_scheme)
                && authority.as_str().eq_ignore_ascii_case(request_host)
                && path_equivalent(uri.path(), request_path)
        }
        (None, None) => path_equivalent(uri.path(), request_path),
        _ => false,
    }
}

fn path_equivalent(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left_decoded = urlencoding::decode(left).ok();
    let right_decoded = urlencoding::decode(right).ok();
    match (left_decoded.as_deref(), right_decoded.as_deref()) {
        (Some(left), Some(right)) => left == right,
        (Some(left), None) => left == right,
        (None, Some(right)) => left == right,
        (None, None) => false,
    }
}
