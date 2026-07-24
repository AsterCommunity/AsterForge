//! Product-neutral DeltaV request planning and response composition.

use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};

use crate::{
    DavErrorCondition, DavResourceKind, DavResponse, DavVersionXml, DavXmlError, dav_error_element,
    dav_version_multistatus_element, parse_report_root,
};

/// Failure while selecting the supported DeltaV REPORT grammar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavVersionTreeReportError {
    /// The REPORT body is not safe, well-formed WebDAV XML.
    #[error(transparent)]
    Xml(#[from] DavXmlError),
    /// The REPORT root is not the supported `DAV:version-tree` report.
    #[error("unsupported DeltaV REPORT type: {name}")]
    Unsupported { name: String },
}

/// Validates that a REPORT request selects `DAV:version-tree`.
pub fn validate_version_tree_report(body: &[u8]) -> Result<(), DavVersionTreeReportError> {
    let root = parse_report_root(body)?;
    if root.name == "version-tree" && root.namespace.as_deref() == Some("DAV:") {
        Ok(())
    } else {
        Err(DavVersionTreeReportError::Unsupported { name: root.name })
    }
}

/// Builds the protocol response for a REPORT grammar selection failure.
pub fn version_tree_report_error_response(
    error: &DavVersionTreeReportError,
) -> Result<DavResponse, DavXmlError> {
    match error {
        DavVersionTreeReportError::Xml(DavXmlError::ExternalEntity) => xml_response(
            StatusCode::FORBIDDEN,
            dav_error_element(&DavErrorCondition::NoExternalEntities),
        ),
        DavVersionTreeReportError::Xml(
            DavXmlError::TooDeep | DavXmlError::Malformed | DavXmlError::InvalidGrammar,
        ) => Ok(text_response(StatusCode::BAD_REQUEST, "Invalid XML body")),
        DavVersionTreeReportError::Unsupported { name } => Ok(text_response(
            StatusCode::NOT_IMPLEMENTED,
            format!("Unsupported REPORT type: {name}"),
        )),
    }
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
pub fn version_tree_response(versions: Vec<DavVersionXml>) -> Result<DavResponse, DavXmlError> {
    xml_response(
        StatusCode::MULTI_STATUS,
        dav_version_multistatus_element(versions),
    )
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
