//! Method-aware HTTP and WebDAV conditional request planning.

use std::time::SystemTime;

use aster_forge_utils::http_validators;
use http::header::{ETAG, HeaderName, IF_MODIFIED_SINCE, IF_UNMODIFIED_SINCE, LAST_MODIFIED};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::{
    DavBackendError, DavFileSystem, DavIfEvaluationError, DavIfStateResolver, DavLockSystem,
    DavMethod, DavPath, DavProtocolError, IfHeader, enforce_if_header,
    enforce_if_header_with_backends,
};

/// Protocol-visible metadata for the request target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavConditionalResource<'a> {
    /// Whether the request target currently maps to a representation.
    pub exists: bool,
    /// Current entity tag, either as an opaque backend value or a complete entity-tag.
    pub etag: Option<&'a str>,
    /// Current modification time when the product can provide one authoritatively.
    pub last_modified: Option<SystemTime>,
}

impl DavConditionalResource<'_> {
    /// Metadata for an unmapped request target.
    #[must_use]
    pub const fn missing() -> Self {
        Self {
            exists: false,
            etag: None,
            last_modified: None,
        }
    }
}

/// Result selected by RFC 9110 conditional request precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavConditionalOutcome {
    /// Continue processing the method.
    Proceed,
    /// Return `304 Not Modified` for GET or HEAD.
    NotModified,
    /// Return `412 Precondition Failed`.
    PreconditionFailed,
}

/// Whether GET byte-range processing can run after preconditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavRangeEvaluation {
    /// Evaluate `Range` and then `If-Range`.
    Evaluate,
    /// Do not evaluate range headers for this method or outcome.
    Skip,
}

/// Complete transport-neutral result of HTTP conditional request planning.
#[derive(Debug, Clone)]
pub struct DavConditionalPlan {
    /// Selected request outcome.
    pub outcome: DavConditionalOutcome,
    /// Whether the range planner is the next RFC 9110 step.
    pub range: DavRangeEvaluation,
    validator_headers: HeaderMap,
}

impl DavConditionalPlan {
    /// Copies representation validators onto statuses whose response contract retains them.
    pub fn apply_response_headers(&self, status: StatusCode, headers: &mut HeaderMap) {
        if !matches!(
            status,
            StatusCode::OK
                | StatusCode::PARTIAL_CONTENT
                | StatusCode::NOT_MODIFIED
                | StatusCode::PRECONDITION_FAILED
                | StatusCode::RANGE_NOT_SATISFIABLE
        ) {
            return;
        }
        for (name, value) in &self.validator_headers {
            headers.insert(name.clone(), value.clone());
        }
    }
}

/// Failure while parsing request conditions or representing product metadata.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavConditionalPlanError {
    /// A request conditional header was malformed.
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    /// Product metadata could not be represented as HTTP validator headers.
    #[error("invalid WebDAV conditional representation metadata")]
    InvalidRepresentation,
}

/// Failure while executing WebDAV `If` before HTTP conditional planning.
#[derive(Debug, thiserror::Error)]
pub enum DavConditionalEvaluationError {
    /// A request header was malformed or a WebDAV `If` condition failed.
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    /// Resolving a WebDAV `If` resource failed in the product backend.
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    /// Product metadata could not be represented as HTTP validator headers.
    #[error("invalid WebDAV conditional representation metadata")]
    InvalidRepresentation,
}

/// Plans RFC 9110 conditional headers for one HTTP/WebDAV method.
///
/// The order is `If-Match`, `If-Unmodified-Since`, `If-None-Match`, GET/HEAD
/// `If-Modified-Since`, and finally GET range eligibility. `If-Match` suppresses
/// `If-Unmodified-Since`; `If-None-Match` suppresses `If-Modified-Since`.
pub fn plan_http_conditionals(
    method: DavMethod,
    headers: &HeaderMap,
    resource: DavConditionalResource<'_>,
) -> Result<DavConditionalPlan, DavConditionalPlanError> {
    let validator_headers = validator_headers(resource)?;

    let if_match = http_validators::if_match_headers_match(headers, resource.exists, resource.etag)
        .map_err(|_| DavProtocolError::bad_request("Invalid If-Match header"))?;
    if let Some(false) = if_match {
        return Ok(plan(
            DavConditionalOutcome::PreconditionFailed,
            method,
            validator_headers,
        ));
    }

    if if_match.is_none()
        && let Some(since) = optional_http_date(headers, IF_UNMODIFIED_SINCE)
        && resource.exists
        && resource.last_modified.is_some_and(|last_modified| {
            http_validators::http_date_epoch_seconds(last_modified)
                > http_validators::http_date_epoch_seconds(since)
        })
    {
        return Ok(plan(
            DavConditionalOutcome::PreconditionFailed,
            method,
            validator_headers,
        ));
    }

    let if_none_match =
        http_validators::if_none_match_headers_match(headers, resource.exists, resource.etag)
            .map_err(|_| DavProtocolError::bad_request("Invalid If-None-Match header"))?;
    if let Some(true) = if_none_match {
        let outcome = if matches!(method, DavMethod::Get | DavMethod::Head) {
            DavConditionalOutcome::NotModified
        } else {
            DavConditionalOutcome::PreconditionFailed
        };
        return Ok(plan(outcome, method, validator_headers));
    }

    if if_none_match.is_none()
        && matches!(method, DavMethod::Get | DavMethod::Head)
        && let Some(since) = optional_http_date(headers, IF_MODIFIED_SINCE)
        && resource.exists
        && resource.last_modified.is_some_and(|last_modified| {
            http_validators::http_date_epoch_seconds(last_modified)
                <= http_validators::http_date_epoch_seconds(since)
        })
    {
        return Ok(plan(
            DavConditionalOutcome::NotModified,
            method,
            validator_headers,
        ));
    }

    Ok(plan(
        DavConditionalOutcome::Proceed,
        method,
        validator_headers,
    ))
}

/// Enforces WebDAV `If` first, then applies the RFC 9110 HTTP conditional planner.
///
/// Tagged destination/resource conditions therefore fail with WebDAV `412` before the
/// request-target HTTP conditions are considered. Product code still chooses the metadata
/// snapshot passed to the HTTP planner.
#[allow(clippy::too_many_arguments)]
pub async fn plan_conditionals(
    if_header: Option<&IfHeader>,
    resolver: &dyn DavIfStateResolver,
    request_path: &DavPath,
    prefix: &str,
    request_scheme: &str,
    request_host: &str,
    method: DavMethod,
    headers: &HeaderMap,
    resource: DavConditionalResource<'_>,
) -> Result<DavConditionalPlan, DavConditionalEvaluationError> {
    enforce_if_header(
        if_header,
        resolver,
        request_path,
        prefix,
        request_scheme,
        request_host,
    )
    .await
    .map_err(map_if_error)?;
    plan_http_conditionals(method, headers, resource).map_err(map_plan_error)
}

/// Enforces WebDAV `If` through the canonical backend ports, then applies HTTP conditions.
///
/// This has the same WebDAV-first ordering as [`plan_conditionals`].
#[allow(clippy::too_many_arguments)]
pub async fn plan_conditionals_with_backends(
    if_header: Option<&IfHeader>,
    filesystem: &dyn DavFileSystem,
    lock_system: &dyn DavLockSystem,
    request_path: &DavPath,
    prefix: &str,
    request_scheme: &str,
    request_host: &str,
    method: DavMethod,
    headers: &HeaderMap,
    resource: DavConditionalResource<'_>,
) -> Result<DavConditionalPlan, DavConditionalEvaluationError> {
    enforce_if_header_with_backends(
        if_header,
        filesystem,
        lock_system,
        request_path,
        prefix,
        request_scheme,
        request_host,
    )
    .await
    .map_err(map_if_error)?;
    plan_http_conditionals(method, headers, resource).map_err(map_plan_error)
}

fn map_if_error(error: DavIfEvaluationError) -> DavConditionalEvaluationError {
    match error {
        DavIfEvaluationError::Protocol(error) => DavConditionalEvaluationError::Protocol(error),
        DavIfEvaluationError::Backend(error) => DavConditionalEvaluationError::Backend(error),
    }
}

fn map_plan_error(error: DavConditionalPlanError) -> DavConditionalEvaluationError {
    match error {
        DavConditionalPlanError::Protocol(error) => DavConditionalEvaluationError::Protocol(error),
        DavConditionalPlanError::InvalidRepresentation => {
            DavConditionalEvaluationError::InvalidRepresentation
        }
    }
}

fn plan(
    outcome: DavConditionalOutcome,
    method: DavMethod,
    validator_headers: HeaderMap,
) -> DavConditionalPlan {
    let range = if outcome == DavConditionalOutcome::Proceed && method == DavMethod::Get {
        DavRangeEvaluation::Evaluate
    } else {
        DavRangeEvaluation::Skip
    };
    DavConditionalPlan {
        outcome,
        range,
        validator_headers,
    }
}

fn validator_headers(
    resource: DavConditionalResource<'_>,
) -> Result<HeaderMap, DavConditionalPlanError> {
    let mut headers = HeaderMap::new();
    if !resource.exists {
        return Ok(headers);
    }
    if let Some(last_modified) = resource.last_modified {
        let value = http_validators::try_format_http_date(last_modified)
            .map_err(|_| DavConditionalPlanError::InvalidRepresentation)?;
        headers.insert(
            LAST_MODIFIED,
            HeaderValue::from_str(&value)
                .map_err(|_| DavConditionalPlanError::InvalidRepresentation)?,
        );
    }
    if let Some(etag) = resource.etag {
        headers.insert(ETAG, entity_tag_header_value(etag)?);
    }
    Ok(headers)
}

fn entity_tag_header_value(etag: &str) -> Result<HeaderValue, DavConditionalPlanError> {
    let etag = etag.trim();
    let rendered = if etag.starts_with('"') || etag.starts_with("W/\"") {
        etag.to_owned()
    } else {
        format!("\"{etag}\"")
    };
    if rendered == "*"
        || http_validators::if_none_match_header_matches(&rendered, true, Some(&rendered)).is_err()
    {
        return Err(DavConditionalPlanError::InvalidRepresentation);
    }
    HeaderValue::from_str(&rendered).map_err(|_| DavConditionalPlanError::InvalidRepresentation)
}

fn optional_http_date(headers: &HeaderMap, name: HeaderName) -> Option<SystemTime> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let raw = value.to_str().ok()?;
    http_validators::parse_http_date(raw).ok()
}
