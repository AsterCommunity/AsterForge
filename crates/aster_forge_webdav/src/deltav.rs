//! Product-neutral DeltaV request planning and response composition.

use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};

use crate::xml::{parse_report_request, parse_version_control_request};
use crate::{
    DavCapabilitySnapshot, DavErrorCondition, DavMultiStatusError, DavMultiStatusLimits,
    DavReportType, DavResourceKind, DavResponse, DavVersionXml, DavXmlError, dav_error_element,
    dav_version_multistatus_bytes,
};

/// Failure while selecting a REPORT type through a capability snapshot.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavReportPlanError {
    /// The REPORT body is not safe, well-formed WebDAV XML.
    #[error(transparent)]
    Xml(#[from] DavXmlError),
    /// The REPORT root is not a standard type known to Forge.
    #[error("unknown WebDAV REPORT type: {name}")]
    UnknownType {
        namespace: Option<String>,
        name: String,
    },
    /// The type is known but absent from this target's validated package set.
    #[error("WebDAV REPORT type is not supported by this resource: {report:?}")]
    NotAvailable { report: DavReportType },
}

/// Product-owned mapping for REPORT selection errors that are not XML syntax failures.
pub trait DavReportErrorResponsePolicy {
    fn unknown_type(&self, namespace: Option<&str>, name: &str) -> DavResponse;

    fn not_available(&self, report: DavReportType) -> DavResponse;
}

/// Parses and gates a REPORT body through the same snapshot used for discovery and dispatch.
pub fn plan_report_request(
    snapshot: &DavCapabilitySnapshot,
    body: &[u8],
) -> Result<DavReportType, DavReportPlanError> {
    let root = parse_report_request(body)?;
    if root.namespace.as_deref() != Some("DAV:") {
        return Err(DavReportPlanError::UnknownType {
            namespace: root.namespace,
            name: root.name,
        });
    }
    let report = report_type(&root.name).ok_or(DavReportPlanError::UnknownType {
        namespace: root.namespace,
        name: root.name,
    })?;
    if !snapshot.supports_report(report) {
        return Err(DavReportPlanError::NotAvailable { report });
    }
    Ok(report)
}

/// Validates the optional RFC 3253 `DAV:version-control` request body.
pub fn validate_version_control_request(body: &[u8]) -> Result<(), DavXmlError> {
    parse_version_control_request(body)
}

/// Builds the protocol response for a REPORT grammar selection failure.
pub fn report_plan_error_response<P: DavReportErrorResponsePolicy>(
    error: &DavReportPlanError,
    policy: &P,
) -> Result<DavResponse, DavXmlError> {
    match error {
        DavReportPlanError::Xml(error) => xml_request_error_response(*error),
        DavReportPlanError::UnknownType { namespace, name } => {
            Ok(policy.unknown_type(namespace.as_deref(), name))
        }
        DavReportPlanError::NotAvailable { report } => Ok(policy.not_available(*report)),
    }
}

/// Builds the protocol response for an invalid VERSION-CONTROL XML request body.
pub fn version_control_request_error_response(
    error: DavXmlError,
) -> Result<DavResponse, DavXmlError> {
    xml_request_error_response(error)
}

/// Builds the file-only conflict response for a version-tree REPORT.
#[must_use]
pub fn version_tree_non_file_response() -> DavResponse {
    text_response(
        StatusCode::CONFLICT,
        "Version history is only available for files",
    )
}

/// Builds a complete 207 DeltaV version-tree response.
pub fn version_tree_response(
    versions: Vec<DavVersionXml>,
) -> Result<DavResponse, DavMultiStatusError> {
    version_tree_response_with_limits(versions, DavMultiStatusLimits::default())
}

/// Builds a bounded complete 207 DeltaV version-tree response.
pub fn version_tree_response_with_limits(
    versions: Vec<DavVersionXml>,
    limits: DavMultiStatusLimits,
) -> Result<DavResponse, DavMultiStatusError> {
    let mut response = DavResponse::bytes(
        StatusCode::MULTI_STATUS,
        dav_version_multistatus_bytes(versions, limits)?,
    );
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    Ok(response)
}

/// Selects the VERSION-CONTROL response for the resolved resource kind.
#[must_use]
pub fn version_control_response(kind: DavResourceKind) -> DavResponse {
    match kind {
        DavResourceKind::File => text_response(StatusCode::OK, "Already under version control"),
        DavResourceKind::Collection => text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "Only files support version control",
        ),
    }
}

fn text_response(status: StatusCode, body: impl Into<String>) -> DavResponse {
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

fn xml_request_error_response(error: DavXmlError) -> Result<DavResponse, DavXmlError> {
    match error {
        DavXmlError::ExternalEntity => xml_response(
            StatusCode::FORBIDDEN,
            dav_error_element(&DavErrorCondition::NoExternalEntities),
        ),
        DavXmlError::TooLarge => Ok(text_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "WebDAV XML body too large",
        )),
        DavXmlError::TooDeep | DavXmlError::Malformed | DavXmlError::InvalidGrammar => {
            Ok(text_response(StatusCode::BAD_REQUEST, "Invalid XML body"))
        }
    }
}

fn xml_response(
    status: StatusCode,
    root: crate::DavXmlElement,
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

fn report_type(name: &str) -> Option<DavReportType> {
    DavReportType::ALL
        .into_iter()
        .find(|report| report.local_name() == name)
}
