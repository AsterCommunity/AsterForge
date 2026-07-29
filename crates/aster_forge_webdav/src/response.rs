//! Transport-neutral WebDAV response model and download response planning.

use std::fmt::Write as _;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use aster_forge_utils::http_range::{
    HttpByteRange, HttpRangeError, parse_byte_ranges, parse_single_byte_range,
};
use aster_forge_utils::http_validators::{http_date_epoch_seconds, parse_http_date};
use bytes::Bytes;
use futures::Stream;
use http::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, IF_RANGE, RANGE,
};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::DavMultiStatusStream;
use crate::{
    DavBackendError, DavBackendErrorKind, DavCapabilityEvaluationError, DavCapabilitySnapshot,
    DavConditionalOutcome, DavConditionalPlan, DavConditionalPlanError, DavConditionalResource,
    DavContentStream, DavDownloadOpenError, DavDownloadSource, DavErrorCondition, DavMethod,
    DavMethodGateError, DavOpenedDownload, DavPath, DavProtocolError, DavProtocolErrorKind,
    DavRangeEvaluation, DavXmlElement, DavXmlError, dav_error_element, plan_http_conditionals,
};

/// Failure while enforcing a request body policy in the transport adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavBodyError {
    #[error("failed to read WebDAV request body")]
    ReadFailed,
    #[error("WebDAV request body is too large")]
    BodyTooLarge,
    #[error("WebDAV method does not accept a request body")]
    BodyNotAllowed,
}

/// Whether a successful GET/HEAD response needs content from the product backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavDownloadBody {
    /// The response has no body because it is a HEAD, 304, or 416 response.
    Empty,
    /// Stream the complete representation with its planned output length.
    Full { expected_length: u64 },
    /// Stream only the selected representation range.
    Range(HttpByteRange),
    /// Stream the selected ranges with RFC 9110 `multipart/byteranges` framing.
    Multipart(DavMultipartDownloadPlan),
}

/// Hard bounds applied before any multi-range backend stream is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavMultiRangeLimits {
    pub maximum_header_bytes: usize,
    /// Maximum raw range specs accepted before coalescing allocates its result.
    ///
    /// Coalescing preserves request order and has superlinear worst-case CPU cost. Products must
    /// choose this bound explicitly; raising it can significantly increase per-request work and
    /// there is no implicit fallback limit.
    pub maximum_raw_ranges: usize,
    pub maximum_segments: usize,
    pub maximum_aggregate_bytes: u64,
    pub maximum_backend_opens: usize,
}

impl DavMultiRangeLimits {
    #[must_use]
    pub const fn new(
        maximum_header_bytes: usize,
        maximum_raw_ranges: usize,
        maximum_segments: usize,
        maximum_aggregate_bytes: u64,
        maximum_backend_opens: usize,
    ) -> Self {
        Self {
            maximum_header_bytes,
            maximum_raw_ranges,
            maximum_segments,
            maximum_aggregate_bytes,
            maximum_backend_opens,
        }
    }
}

/// Stable response policy when an otherwise valid multi-range request exceeds a hard bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavRangeLimitBehavior {
    /// Ignore `Range` and serve the complete representation with `200 OK`.
    IgnoreRange,
    /// Reject the selected range-set with `416 Range Not Satisfiable`.
    RangeNotSatisfiable,
}

/// Explicit policy that enables bounded multi-range response planning for one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavMultiRangePolicy {
    pub limits: DavMultiRangeLimits,
    /// Merge overlap, adjacency, and gaps no larger than this number of representation bytes.
    pub coalesce_gap_bytes: u64,
    pub limit_behavior: DavRangeLimitBehavior,
}

impl DavMultiRangePolicy {
    #[must_use]
    pub const fn new(
        limits: DavMultiRangeLimits,
        coalesce_gap_bytes: u64,
        limit_behavior: DavRangeLimitBehavior,
    ) -> Self {
        Self {
            limits,
            coalesce_gap_bytes,
            limit_behavior,
        }
    }
}

/// One final backend range and its framing location in a multipart plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavMultipartSegmentPlan {
    range: HttpByteRange,
    frame_start: usize,
    frame_end: usize,
}

impl DavMultipartSegmentPlan {
    #[must_use]
    pub const fn range(&self) -> HttpByteRange {
        self.range
    }
}

/// Bounded multipart framing and final backend range-open plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavMultipartDownloadPlan {
    requested_range_count: usize,
    selected_length: u64,
    expected_length: u64,
    segments: Vec<DavMultipartSegmentPlan>,
    framing: Bytes,
    closing_start: usize,
}

impl DavMultipartDownloadPlan {
    /// Returns the sender's non-empty raw range-spec count.
    #[must_use]
    pub const fn requested_range_count(&self) -> usize {
        self.requested_range_count
    }

    /// Returns the final coalesced range-open plans in response order.
    #[must_use]
    pub fn segments(&self) -> &[DavMultipartSegmentPlan] {
        &self.segments
    }

    /// Returns the total representation bytes selected across all final segments.
    #[must_use]
    pub const fn selected_length(&self) -> u64 {
        self.selected_length
    }

    /// Returns the exact multipart body length, including framing.
    #[must_use]
    pub const fn expected_length(&self) -> u64 {
        self.expected_length
    }
}

/// A complete response shell plus the storage read selected by the protocol layer.
pub struct DavDownloadPlan {
    pub response: DavResponse,
    pub body: DavDownloadBody,
}

/// Failure while building a product-neutral download response plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavDownloadPlanError {
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    #[error("invalid WebDAV download representation metadata")]
    InvalidRepresentation,
}

/// WebDAV response body before transport adaptation.
pub enum DavResponseBody {
    Empty,
    Bytes(Bytes),
    Stream(DavContentStream),
    MultiStatus(DavMultiStatusStream),
}

/// Status, headers, and body produced by the protocol layer.
pub struct DavResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: DavResponseBody,
}

impl DavResponse {
    /// Creates an empty response.
    #[must_use]
    pub fn empty(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: DavResponseBody::Empty,
        }
    }

    /// Creates a byte response.
    #[must_use]
    pub fn bytes(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: DavResponseBody::Bytes(body.into()),
        }
    }
}

pub(crate) fn xml_document_response(
    status: StatusCode,
    root: DavXmlElement,
) -> Result<DavResponse, DavXmlError> {
    let mut response = DavResponse::bytes(status, root.to_bytes()?);
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    if status.is_client_error() || status.is_server_error() {
        response
            .headers
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    Ok(response)
}

pub(crate) fn text_document_response(status: StatusCode, body: impl Into<String>) -> DavResponse {
    let mut response = DavResponse::bytes(status, body.into());
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if status.is_client_error() || status.is_server_error() {
        response
            .headers
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

pub(crate) fn xml_request_error_response(
    error: DavXmlError,
    invalid_grammar_message: &'static str,
) -> Result<DavResponse, DavXmlError> {
    match error {
        DavXmlError::ExternalEntity => xml_document_response(
            StatusCode::FORBIDDEN,
            dav_error_element(&DavErrorCondition::NoExternalEntities),
        ),
        DavXmlError::TooLarge => Ok(text_document_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "WebDAV XML body too large",
        )),
        DavXmlError::TooDeep | DavXmlError::Malformed => Ok(text_document_response(
            StatusCode::BAD_REQUEST,
            "Invalid XML body",
        )),
        DavXmlError::InvalidGrammar => Ok(text_document_response(
            StatusCode::BAD_REQUEST,
            invalid_grammar_message,
        )),
    }
}

/// Maps a protocol parsing/precondition failure to its transport-neutral response.
#[must_use]
pub fn protocol_error_response(error: &DavProtocolError) -> DavResponse {
    match error.kind() {
        DavProtocolErrorKind::BadRequest => text_document_response(error.status(), error.message()),
        DavProtocolErrorKind::PreconditionFailed => no_store_empty_response(error.status()),
    }
}

/// Maps conditional request parsing or representation failures to a response.
#[must_use]
pub fn conditional_plan_error_response(error: &DavConditionalPlanError) -> DavResponse {
    match error {
        DavConditionalPlanError::Protocol(error) => protocol_error_response(error),
        DavConditionalPlanError::InvalidRepresentation => {
            no_store_empty_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Maps provider lookup or invalid product capability declarations to a response.
#[must_use]
pub fn capability_evaluation_error_response(error: &DavCapabilityEvaluationError) -> DavResponse {
    match error {
        DavCapabilityEvaluationError::Backend(error) => backend_error_response(error),
        DavCapabilityEvaluationError::Plan(_) => {
            no_store_empty_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Maps a classified product backend failure to the WebDAV status contract.
#[must_use]
pub fn backend_error_response(error: &DavBackendError) -> DavResponse {
    let status = match error.kind {
        DavBackendErrorKind::NotFound => StatusCode::NOT_FOUND,
        DavBackendErrorKind::Forbidden => StatusCode::FORBIDDEN,
        DavBackendErrorKind::Conflict | DavBackendErrorKind::AlreadyExists => StatusCode::CONFLICT,
        DavBackendErrorKind::InsufficientStorage => StatusCode::INSUFFICIENT_STORAGE,
        DavBackendErrorKind::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        DavBackendErrorKind::Locked => StatusCode::LOCKED,
        DavBackendErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
        DavBackendErrorKind::Unsupported => StatusCode::METHOD_NOT_ALLOWED,
        DavBackendErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    no_store_empty_response(status)
}

pub(crate) fn no_store_empty_response(status: StatusCode) -> DavResponse {
    let mut response = DavResponse::empty(status);
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Builds the response to a DAV `OPTIONS` request.
#[must_use]
pub fn options_response(snapshot: &DavCapabilitySnapshot) -> DavResponse {
    let mut response = DavResponse::empty(StatusCode::OK);
    response
        .headers
        .insert(ALLOW, snapshot.allow_header().clone());
    if let Some(dav) = snapshot.dav_header() {
        response.headers.insert("DAV", dav.clone());
    }
    if let Some(accept_patch) = snapshot.accept_patch_header() {
        response
            .headers
            .insert("Accept-Patch", accept_patch.clone());
    }
    if snapshot.has_ms_author_via() {
        response
            .headers
            .insert("MS-Author-Via", HeaderValue::from_static("DAV"));
    }
    response
}

/// Builds the response for an unsupported HTTP/WebDAV method.
#[must_use]
pub fn method_not_allowed_response(snapshot: &DavCapabilitySnapshot) -> DavResponse {
    let mut response = DavResponse::empty(StatusCode::METHOD_NOT_ALLOWED);
    response
        .headers
        .insert(ALLOW, snapshot.allow_header().clone());
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Gates known and unknown methods through the same snapshot used by OPTIONS and 405.
pub fn gate_method(
    method: Option<DavMethod>,
    snapshot: &DavCapabilitySnapshot,
) -> Result<DavMethod, DavMethodGateError> {
    match method {
        Some(method) if snapshot.allows(method) => Ok(method),
        Some(_) | None => Err(DavMethodGateError::MethodNotAllowed),
    }
}

/// Builds the protocol response for a transport body-policy failure.
#[must_use]
pub fn body_error_response(error: DavBodyError) -> DavResponse {
    let (status, body) = match error {
        DavBodyError::ReadFailed => (StatusCode::BAD_REQUEST, Some("Failed to read request body")),
        DavBodyError::BodyTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Some("WebDAV request body too large"),
        ),
        DavBodyError::BodyNotAllowed => (StatusCode::UNSUPPORTED_MEDIA_TYPE, None),
    };
    let mut response = match body {
        Some(body) => DavResponse::bytes(status, body),
        None => DavResponse::empty(status),
    };
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if body.is_some() {
        response.headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
    }
    response
}

/// Builds the GET/HEAD response and storage-read plan after product metadata has been resolved.
pub fn plan_download_response(
    headers: &HeaderMap,
    head_only: bool,
    content_length: u64,
    content_type: &str,
    etag: Option<&str>,
    last_modified: SystemTime,
) -> Result<DavDownloadPlan, DavDownloadPlanError> {
    plan_download_response_with_mode(
        headers,
        head_only,
        content_length,
        content_type,
        etag,
        last_modified,
        DavRangePlanningMode::SingleOnly,
    )
}

/// Builds a GET/HEAD plan with an explicitly bounded multi-range policy.
///
/// Every limit and the final coalescing pass are applied before [`open_download`] opens a
/// backend stream. A request containing one raw range retains the ordinary single-part path.
pub fn plan_download_response_with_multi_range(
    headers: &HeaderMap,
    head_only: bool,
    content_length: u64,
    content_type: &str,
    etag: Option<&str>,
    last_modified: SystemTime,
    policy: DavMultiRangePolicy,
) -> Result<DavDownloadPlan, DavDownloadPlanError> {
    plan_download_response_with_mode(
        headers,
        head_only,
        content_length,
        content_type,
        etag,
        last_modified,
        DavRangePlanningMode::Multi(policy),
    )
}

#[derive(Clone, Copy)]
enum DavRangePlanningMode {
    SingleOnly,
    Multi(DavMultiRangePolicy),
}

enum DavRangeSelection {
    Full,
    NotSatisfiable,
    Single(HttpByteRange),
    Multipart {
        requested_range_count: usize,
        selected_length: u64,
        ranges: Vec<HttpByteRange>,
    },
}

fn plan_download_response_with_mode(
    headers: &HeaderMap,
    head_only: bool,
    content_length: u64,
    content_type: &str,
    etag: Option<&str>,
    last_modified: SystemTime,
    range_mode: DavRangePlanningMode,
) -> Result<DavDownloadPlan, DavDownloadPlanError> {
    let conditional = plan_http_conditionals(
        if head_only {
            DavMethod::Head
        } else {
            DavMethod::Get
        },
        headers,
        DavConditionalResource {
            exists: true,
            etag,
            last_modified: Some(last_modified),
        },
    )
    .map_err(|error| match error {
        DavConditionalPlanError::Protocol(error) => DavDownloadPlanError::Protocol(error),
        DavConditionalPlanError::InvalidRepresentation => {
            DavDownloadPlanError::InvalidRepresentation
        }
    })?;
    match conditional.outcome {
        DavConditionalOutcome::Proceed => {}
        DavConditionalOutcome::NotModified => {
            let mut response = DavResponse::empty(StatusCode::NOT_MODIFIED);
            conditional.apply_response_headers(response.status, &mut response.headers);
            return Ok(DavDownloadPlan {
                response,
                body: DavDownloadBody::Empty,
            });
        }
        DavConditionalOutcome::PreconditionFailed => {
            let mut response = no_store_empty_response(StatusCode::PRECONDITION_FAILED);
            conditional.apply_response_headers(response.status, &mut response.headers);
            return Ok(DavDownloadPlan {
                response,
                body: DavDownloadBody::Empty,
            });
        }
    }

    let range = if conditional.range == DavRangeEvaluation::Skip
        || !if_range_matches(headers, etag, last_modified)
    {
        DavRangeSelection::Full
    } else {
        plan_requested_range(headers, content_length, range_mode)?
    };
    let (status, response_length, response_content_type, content_range, body) = match range {
        DavRangeSelection::Single(range) => (
            StatusCode::PARTIAL_CONTENT,
            range.length(),
            header_value(content_type)?,
            Some(range.content_range_header()),
            DavDownloadBody::Range(range),
        ),
        DavRangeSelection::Multipart {
            requested_range_count,
            selected_length,
            ranges,
        } => {
            let (plan, multipart_content_type) =
                build_multipart_plan(requested_range_count, selected_length, ranges, content_type)?;
            (
                StatusCode::PARTIAL_CONTENT,
                plan.expected_length,
                multipart_content_type,
                None,
                DavDownloadBody::Multipart(plan),
            )
        }
        DavRangeSelection::Full if head_only => (
            StatusCode::OK,
            content_length,
            header_value(content_type)?,
            None,
            DavDownloadBody::Empty,
        ),
        DavRangeSelection::Full => (
            StatusCode::OK,
            content_length,
            header_value(content_type)?,
            None,
            DavDownloadBody::Full {
                expected_length: content_length,
            },
        ),
        DavRangeSelection::NotSatisfiable => {
            return Ok(range_not_satisfiable_plan(content_length, &conditional));
        }
    };
    let mut response = DavResponse::empty(status);
    response
        .headers
        .insert(CONTENT_LENGTH, header_value(&response_length.to_string())?);
    response.headers.insert(CONTENT_TYPE, response_content_type);
    response
        .headers
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
        .headers
        .insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));
    if let Some(content_range) = content_range {
        response
            .headers
            .insert(CONTENT_RANGE, header_value(&content_range)?);
    }
    conditional.apply_response_headers(response.status, &mut response.headers);

    Ok(DavDownloadPlan { response, body })
}

/// Opens the storage stream selected by a download plan and verifies its declared length.
///
/// Full, single-range, and every final multipart segment carry their exact selected length.
/// Multipart segments are all opened and checked before the returned stream can emit bytes;
/// empty plans never call the backend.
pub async fn open_download<Source: DavDownloadSource>(
    source: &Source,
    path: &DavPath,
    body: DavDownloadBody,
) -> Result<Option<DavOpenedDownload>, DavDownloadOpenError> {
    let (opened, planned_length) = match body {
        DavDownloadBody::Empty => return Ok(None),
        DavDownloadBody::Full { expected_length } => {
            (source.open_full(path).await?, expected_length)
        }
        DavDownloadBody::Range(range) => (source.open_range(path, range).await?, range.length()),
        DavDownloadBody::Multipart(plan) => {
            return open_multipart_download(source, path, plan).await;
        }
    };
    if opened.expected_length != planned_length {
        return Err(DavDownloadOpenError::LengthMismatch {
            planned: planned_length,
            opened: opened.expected_length,
        });
    }
    Ok(Some(opened))
}

fn plan_requested_range(
    headers: &HeaderMap,
    content_length: u64,
    mode: DavRangePlanningMode,
) -> Result<DavRangeSelection, DavDownloadPlanError> {
    match mode {
        DavRangePlanningMode::SingleOnly => {
            let Some(value) = headers.get(RANGE) else {
                return Ok(DavRangeSelection::Full);
            };
            let Ok(raw) = value.to_str() else {
                return Ok(DavRangeSelection::NotSatisfiable);
            };
            Ok(match parse_single_byte_range(raw, content_length) {
                Ok(range) => DavRangeSelection::Single(range),
                Err(
                    HttpRangeError::UnsupportedUnit
                    | HttpRangeError::MultipleRangesUnsupported
                    | HttpRangeError::HeaderTooLong
                    | HttpRangeError::TooManyRanges,
                ) => DavRangeSelection::Full,
                Err(
                    HttpRangeError::Malformed
                    | HttpRangeError::InvalidNumber
                    | HttpRangeError::EmptyRepresentation
                    | HttpRangeError::Unsatisfiable,
                ) => DavRangeSelection::NotSatisfiable,
            })
        }
        DavRangePlanningMode::Multi(policy) => plan_multi_range(headers, content_length, policy),
    }
}

fn plan_multi_range(
    headers: &HeaderMap,
    content_length: u64,
    policy: DavMultiRangePolicy,
) -> Result<DavRangeSelection, DavDownloadPlanError> {
    let mut values = headers.get_all(RANGE).iter();
    let Some(value) = values.next() else {
        return Ok(DavRangeSelection::Full);
    };
    if values.next().is_some() {
        return Ok(DavRangeSelection::NotSatisfiable);
    }
    if value.as_bytes().len() > policy.limits.maximum_header_bytes {
        return Ok(range_limit_selection(policy.limit_behavior));
    }
    let Ok(raw) = value.to_str() else {
        return Ok(DavRangeSelection::NotSatisfiable);
    };
    let set = match parse_byte_ranges(
        raw,
        content_length,
        policy.limits.maximum_header_bytes,
        policy.limits.maximum_raw_ranges,
    ) {
        Ok(set) => set,
        Err(HttpRangeError::UnsupportedUnit) => return Ok(DavRangeSelection::Full),
        Err(HttpRangeError::HeaderTooLong | HttpRangeError::TooManyRanges) => {
            return Ok(range_limit_selection(policy.limit_behavior));
        }
        Err(
            HttpRangeError::MultipleRangesUnsupported
            | HttpRangeError::Malformed
            | HttpRangeError::InvalidNumber
            | HttpRangeError::EmptyRepresentation
            | HttpRangeError::Unsatisfiable,
        ) => return Ok(DavRangeSelection::NotSatisfiable),
    };
    let requested_range_count = set.requested_count();
    if requested_range_count == 1 {
        return Ok(set
            .ranges()
            .first()
            .copied()
            .map_or(DavRangeSelection::NotSatisfiable, DavRangeSelection::Single));
    }

    let ranges = coalesce_ranges(set.into_ranges(), policy.coalesce_gap_bytes)?;
    if ranges.len() > policy.limits.maximum_segments
        || ranges.len() > policy.limits.maximum_backend_opens
    {
        return Ok(range_limit_selection(policy.limit_behavior));
    }
    let mut selected_length = 0u64;
    for range in &ranges {
        // Coalesced ranges are non-overlapping intervals in one u64-sized representation.
        selected_length += range.length();
    }
    if selected_length > policy.limits.maximum_aggregate_bytes {
        return Ok(range_limit_selection(policy.limit_behavior));
    }

    Ok(DavRangeSelection::Multipart {
        requested_range_count,
        selected_length,
        ranges,
    })
}

const fn range_limit_selection(behavior: DavRangeLimitBehavior) -> DavRangeSelection {
    match behavior {
        DavRangeLimitBehavior::IgnoreRange => DavRangeSelection::Full,
        DavRangeLimitBehavior::RangeNotSatisfiable => DavRangeSelection::NotSatisfiable,
    }
}

fn coalesce_ranges(
    ranges: Vec<HttpByteRange>,
    maximum_gap: u64,
) -> Result<Vec<HttpByteRange>, DavDownloadPlanError> {
    let mut coalesced = Vec::with_capacity(ranges.len());
    for range in ranges {
        let mut start = range.start();
        let mut end = range.end();
        let mut insertion = coalesced.len();
        let mut index = 0usize;
        while index < coalesced.len() {
            let candidate: HttpByteRange = coalesced[index];
            if ranges_are_close(start, end, candidate.start(), candidate.end(), maximum_gap) {
                insertion = insertion.min(index);
                start = start.min(candidate.start());
                end = end.max(candidate.end());
                coalesced.remove(index);
                index = 0;
            } else {
                index += 1;
            }
        }
        let merged = HttpByteRange::new(start, end, range.total_size())
            .map_err(|_| DavDownloadPlanError::InvalidRepresentation)?;
        if insertion == coalesced.len() {
            coalesced.push(merged);
        } else {
            coalesced.insert(insertion, merged);
        }
    }
    Ok(coalesced)
}

const fn ranges_are_close(
    left_start: u64,
    left_end: u64,
    right_start: u64,
    right_end: u64,
    maximum_gap: u64,
) -> bool {
    if left_end < right_start {
        right_start - left_end - 1 <= maximum_gap
    } else if right_end < left_start {
        left_start - right_end - 1 <= maximum_gap
    } else {
        true
    }
}

fn build_multipart_plan(
    requested_range_count: usize,
    selected_length: u64,
    ranges: Vec<HttpByteRange>,
    content_type: &str,
) -> Result<(DavMultipartDownloadPlan, HeaderValue), DavDownloadPlanError> {
    header_value(content_type)?;
    let boundary = multipart_boundary();
    let multipart_content_type =
        header_value(&format!("multipart/byteranges; boundary={boundary}"))?;
    let estimated_frame_bytes = ranges
        .len()
        .saturating_mul(
            boundary
                .len()
                .saturating_add(content_type.len())
                .saturating_add(96),
        )
        .saturating_add(boundary.len().saturating_add(8));
    let mut framing = String::with_capacity(estimated_frame_bytes);
    let mut segments = Vec::with_capacity(ranges.len());
    for (index, range) in ranges.into_iter().enumerate() {
        let frame_start = framing.len();
        if index == 0 {
            write!(framing, "--{boundary}\r\n")
        } else {
            write!(framing, "\r\n--{boundary}\r\n")
        }
        .map_err(|_| DavDownloadPlanError::InvalidRepresentation)?;
        write!(
            framing,
            "Content-Type: {content_type}\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
            range.start(),
            range.end(),
            range.total_size()
        )
        .map_err(|_| DavDownloadPlanError::InvalidRepresentation)?;
        segments.push(DavMultipartSegmentPlan {
            range,
            frame_start,
            frame_end: framing.len(),
        });
    }
    let closing_start = framing.len();
    write!(framing, "\r\n--{boundary}--\r\n")
        .map_err(|_| DavDownloadPlanError::InvalidRepresentation)?;
    let framing = Bytes::from(framing);
    let framing_length =
        u64::try_from(framing.len()).map_err(|_| DavDownloadPlanError::InvalidRepresentation)?;
    let expected_length = selected_length
        .checked_add(framing_length)
        .ok_or(DavDownloadPlanError::InvalidRepresentation)?;
    Ok((
        DavMultipartDownloadPlan {
            requested_range_count,
            selected_length,
            expected_length,
            segments,
            framing,
            closing_start,
        },
        multipart_content_type,
    ))
}

fn multipart_boundary() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "aster-forge-{:016x}-{:08x}-{sequence:016x}",
        now.as_secs(),
        now.subsec_nanos()
    )
}

async fn open_multipart_download<Source: DavDownloadSource>(
    source: &Source,
    path: &DavPath,
    plan: DavMultipartDownloadPlan,
) -> Result<Option<DavOpenedDownload>, DavDownloadOpenError> {
    let DavMultipartDownloadPlan {
        expected_length,
        segments,
        framing,
        closing_start,
        ..
    } = plan;
    let mut parts = Vec::with_capacity(segments.len());
    for segment in segments {
        let opened = source.open_range(path, segment.range).await?;
        if opened.expected_length != segment.range.length() {
            return Err(DavDownloadOpenError::LengthMismatch {
                planned: segment.range.length(),
                opened: opened.expected_length,
            });
        }
        parts.push(DavOpenedMultipartPart {
            stream: opened.stream,
            expected_length: opened.expected_length,
            seen: 0,
            frame_start: segment.frame_start,
            frame_end: segment.frame_end,
        });
    }
    let stream = DavMultipartStream {
        parts,
        framing,
        closing_start,
        part_index: 0,
        phase: DavMultipartStreamPhase::Frame,
    };
    Ok(Some(DavOpenedDownload::new(
        Box::pin(stream),
        expected_length,
    )))
}

struct DavOpenedMultipartPart {
    stream: DavContentStream,
    expected_length: u64,
    seen: u64,
    frame_start: usize,
    frame_end: usize,
}

#[derive(Clone, Copy)]
enum DavMultipartStreamPhase {
    Frame,
    Body,
    Closing,
    Done,
}

struct DavMultipartStream {
    parts: Vec<DavOpenedMultipartPart>,
    framing: Bytes,
    closing_start: usize,
    part_index: usize,
    phase: DavMultipartStreamPhase,
}

impl Stream for DavMultipartStream {
    type Item = Result<Bytes, DavBackendError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.phase {
                DavMultipartStreamPhase::Frame => {
                    let Some(part) = this.parts.get(this.part_index) else {
                        this.phase = DavMultipartStreamPhase::Closing;
                        continue;
                    };
                    let frame = this.framing.slice(part.frame_start..part.frame_end);
                    this.phase = DavMultipartStreamPhase::Body;
                    return Poll::Ready(Some(Ok(frame)));
                }
                DavMultipartStreamPhase::Body => {
                    // Frame enters Body only after it confirmed this index exists.
                    let part = &mut this.parts[this.part_index];
                    match part.stream.as_mut().poll_next(context) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Some(Ok(chunk))) => {
                            if chunk.is_empty() {
                                continue;
                            }
                            let chunk_length = chunk.len() as u64;
                            if chunk_length > part.expected_length - part.seen {
                                this.phase = DavMultipartStreamPhase::Done;
                                return Poll::Ready(Some(Err(multipart_stream_error())));
                            }
                            part.seen += chunk_length;
                            return Poll::Ready(Some(Ok(chunk)));
                        }
                        Poll::Ready(Some(Err(error))) => {
                            this.phase = DavMultipartStreamPhase::Done;
                            return Poll::Ready(Some(Err(error)));
                        }
                        Poll::Ready(None) if part.seen == part.expected_length => {
                            this.part_index += 1;
                            this.phase = DavMultipartStreamPhase::Frame;
                        }
                        Poll::Ready(None) => {
                            this.phase = DavMultipartStreamPhase::Done;
                            return Poll::Ready(Some(Err(multipart_stream_error())));
                        }
                    }
                }
                DavMultipartStreamPhase::Closing => {
                    this.phase = DavMultipartStreamPhase::Done;
                    return Poll::Ready(Some(Ok(this.framing.slice(this.closing_start..))));
                }
                DavMultipartStreamPhase::Done => return Poll::Ready(None),
            }
        }
    }
}

const fn multipart_stream_error() -> DavBackendError {
    DavBackendError::new(DavBackendErrorKind::Internal)
}

fn if_range_matches(headers: &HeaderMap, etag: Option<&str>, last_modified: SystemTime) -> bool {
    let Some(value) = headers.get(IF_RANGE) else {
        return true;
    };
    let Ok(raw) = value.to_str() else {
        return false;
    };
    let raw = raw.trim();
    if raw.starts_with('"')
        || raw
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("W/"))
    {
        return strong_if_range_etag_matches(raw, etag);
    }
    parse_http_date(raw)
        .is_ok_and(|date| http_date_epoch_seconds(date) == http_date_epoch_seconds(last_modified))
}

fn strong_if_range_etag_matches(candidate: &str, current: Option<&str>) -> bool {
    if candidate
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("W/"))
    {
        return false;
    }
    let Some(candidate) = candidate
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .filter(|value| !value.contains('"'))
    else {
        return false;
    };
    let Some(current) = current else {
        return false;
    };
    if current
        .trim()
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("W/"))
    {
        return false;
    }
    let current = current.trim();
    let current = current
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(current);
    candidate == current
}

/// Builds the response required when a byte range cannot be served.
#[must_use]
pub fn range_not_satisfiable_response(content_length: u64) -> DavResponse {
    let mut response = DavResponse::empty(StatusCode::RANGE_NOT_SATISFIABLE);
    response.headers.insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{content_length}"))
            .unwrap_or_else(|_| HeaderValue::from_static("bytes */0")),
    );
    response
        .headers
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn range_not_satisfiable_plan(
    content_length: u64,
    conditional: &DavConditionalPlan,
) -> DavDownloadPlan {
    let mut response = range_not_satisfiable_response(content_length);
    conditional.apply_response_headers(response.status, &mut response.headers);
    DavDownloadPlan {
        response,
        body: DavDownloadBody::Empty,
    }
}

fn header_value(value: &str) -> Result<HeaderValue, DavDownloadPlanError> {
    HeaderValue::from_str(value).map_err(|_| DavDownloadPlanError::InvalidRepresentation)
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;
    use http::StatusCode;
    use http::header::{CACHE_CONTROL, CONTENT_TYPE};

    use super::{DavResponseBody, text_document_response};

    #[test]
    fn text_document_response_has_explicit_success_and_server_error_cache_policy() {
        let response = text_document_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal error");

        assert_eq!(
            response.headers.get(CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store"))
        );
        assert_eq!(
            response.headers.get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/plain; charset=utf-8"))
        );
        assert!(matches!(
            response.body,
            DavResponseBody::Bytes(ref body) if body == "Internal error"
        ));

        let success = text_document_response(StatusCode::OK, "OK");
        assert!(success.headers.get(CACHE_CONTROL).is_none());
    }
}
