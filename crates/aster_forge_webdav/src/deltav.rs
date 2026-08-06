//! Product-neutral RFC 3253 core request planning and response composition.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;

use aster_forge_xml::XmlSafetyPolicy;
use async_trait::async_trait;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};

use crate::xml::{parse_report_request, parse_version_control_request};
use crate::{
    DavAutoVersion, DavBackendError, DavBackendErrorKind, DavCancellation, DavCapabilitySnapshot,
    DavErrorCondition, DavMethod, DavMultiStatusError, DavMultiStatusErrorKind, DavMultiStatusItem,
    DavMultiStatusLimits, DavPropStat, DavReportType, DavRequestedProperty, DavResponse,
    DavVersioningState, DavXmlElement, DavXmlError, DavXmlNode, Depth, dav_element,
    dav_error_element, dav_multistatus_bytes, dav_property_name_element, dav_property_text_element,
    dav_text_element,
};

const DAV_NAMESPACE: &str = "DAV:";

/// Independent hard limits for REPORT grammar, traversal, and response composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavReportLimits {
    pub maximum_input_bytes: usize,
    pub maximum_xml_depth: usize,
    /// Maximum nested `DAV:property` levels in the request selection AST, counted from one.
    pub maximum_selection_depth: usize,
    /// Maximum `DAV:property` nodes in the request selection AST.
    pub maximum_selection_properties: usize,
    /// Maximum resource expansion hops, counted from zero at the root resource.
    pub maximum_expansion_depth: usize,
    /// Maximum resources visited during expansion, including the root resource.
    pub maximum_expanded_resources: usize,
    /// Maximum backend property lookups performed during expansion.
    pub maximum_expanded_properties: usize,
    pub multistatus: DavMultiStatusLimits,
}

impl DavReportLimits {
    fn is_valid(self) -> bool {
        self.maximum_input_bytes != 0
            && self.maximum_xml_depth != 0
            && self.maximum_selection_depth != 0
            && self.maximum_selection_properties != 0
            && self.maximum_expansion_depth != 0
            && self.maximum_expanded_resources != 0
            && self.maximum_expanded_properties != 0
    }
}

impl Default for DavReportLimits {
    fn default() -> Self {
        let xml = XmlSafetyPolicy::untrusted();
        Self {
            maximum_input_bytes: xml.max_input_bytes,
            maximum_xml_depth: xml.max_depth,
            maximum_selection_depth: 16,
            maximum_selection_properties: 100_000,
            maximum_expansion_depth: 16,
            maximum_expanded_resources: 10_000,
            maximum_expanded_properties: 100_000,
            multistatus: DavMultiStatusLimits::default(),
        }
    }
}

/// One nested RFC 3253 `DAV:property` selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavExpandPropertySelection {
    pub property: DavRequestedProperty,
    pub nested: Vec<Self>,
}

/// Parsed `DAV:version-tree` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavVersionTreeRequest {
    /// `None` selects the canonical core default property set.
    pub properties: Option<Vec<DavRequestedProperty>>,
    /// REPORT defaults to Depth 0 when the header is absent.
    pub depth: Depth,
}

/// Parsed `DAV:expand-property` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavExpandPropertyRequest {
    pub properties: Vec<DavExpandPropertySelection>,
    /// REPORT defaults to Depth 0 when the header is absent.
    pub depth: Depth,
}

/// Transport-neutral REPORT request selected through one capability snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavReportRequest {
    VersionTree(DavVersionTreeRequest),
    ExpandProperty(DavExpandPropertyRequest),
    /// A report owned by another advertised RFC package.
    Other {
        report: DavReportType,
        depth: Depth,
    },
}

impl DavReportRequest {
    #[must_use]
    pub const fn report_type(&self) -> DavReportType {
        match self {
            Self::VersionTree(_) => DavReportType::VersionTree,
            Self::ExpandProperty(_) => DavReportType::ExpandProperty,
            Self::Other { report, .. } => *report,
        }
    }

    #[must_use]
    pub const fn depth(&self) -> Depth {
        match self {
            Self::VersionTree(request) => request.depth,
            Self::ExpandProperty(request) => request.depth,
            Self::Other { depth, .. } => *depth,
        }
    }
}

pub(crate) enum DavParsedReport {
    VersionTree(Option<Vec<DavRequestedProperty>>),
    ExpandProperty(Vec<DavExpandPropertySelection>),
    Other(DavRequestedProperty),
}

/// Failure while parsing and selecting a REPORT through a capability snapshot.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavReportPlanError {
    #[error("invalid REPORT limits")]
    InvalidLimits,
    #[error(transparent)]
    Xml(#[from] DavXmlError),
    #[error("unknown WebDAV REPORT type {name:?} in namespace {namespace:?}")]
    UnknownType {
        namespace: Option<String>,
        name: String,
    },
    #[error("WebDAV REPORT type is not supported by this resource: {report:?}")]
    NotAvailable { report: DavReportType },
}

/// Product-owned mapping for REPORT selection errors that are not XML syntax failures.
pub trait DavReportErrorResponsePolicy {
    fn unknown_type(&self, namespace: Option<&str>, name: &str) -> DavResponse;
    fn not_available(&self, report: DavReportType) -> DavResponse;
}

/// Parses a REPORT using the RFC default Depth 0 and default hard limits.
///
/// # Errors
///
/// Returns an error when grammar, limits, or the target capability snapshot reject the report.
pub fn plan_report_request(
    snapshot: &DavCapabilitySnapshot,
    body: &[u8],
) -> Result<DavReportRequest, DavReportPlanError> {
    plan_report_request_with_limits(snapshot, body, None, DavReportLimits::default())
}

/// Parses a REPORT with an explicit parsed Depth and product-configured limits.
///
/// `depth == None` applies the RFC 3253 default of Depth 0. An explicitly supplied Depth is
/// retained even when it is also zero, allowing the product traversal adapter to select the
/// required Multi-Status execution shape.
///
/// # Errors
///
/// Returns an error when grammar, limits, or the target capability snapshot reject the report.
pub fn plan_report_request_with_limits(
    snapshot: &DavCapabilitySnapshot,
    body: &[u8],
    depth: Option<Depth>,
    limits: DavReportLimits,
) -> Result<DavReportRequest, DavReportPlanError> {
    if !limits.is_valid() {
        return Err(DavReportPlanError::InvalidLimits);
    }
    let parsed = parse_report_request(
        body,
        limits.maximum_input_bytes,
        limits.maximum_xml_depth,
        limits.maximum_selection_depth,
        limits.maximum_selection_properties,
    )?;
    let depth = depth.unwrap_or(Depth::Zero);
    let request = match parsed {
        DavParsedReport::VersionTree(properties) => {
            DavReportRequest::VersionTree(DavVersionTreeRequest { properties, depth })
        }
        DavParsedReport::ExpandProperty(properties) => {
            DavReportRequest::ExpandProperty(DavExpandPropertyRequest { properties, depth })
        }
        DavParsedReport::Other(root) => {
            if root.namespace.as_deref() != Some(DAV_NAMESPACE) {
                return Err(DavReportPlanError::UnknownType {
                    namespace: root.namespace,
                    name: root.name,
                });
            }
            let report = report_type(&root.name).ok_or(DavReportPlanError::UnknownType {
                namespace: root.namespace,
                name: root.name,
            })?;
            DavReportRequest::Other { report, depth }
        }
    };
    let report = request.report_type();
    if !snapshot.supports_report(report) {
        return Err(DavReportPlanError::NotAvailable { report });
    }
    Ok(request)
}

/// Builds the protocol response for a REPORT grammar selection failure.
///
/// # Errors
///
/// Returns [`DavXmlError`] when a DAV error body cannot be encoded.
pub fn report_plan_error_response<P: DavReportErrorResponsePolicy>(
    error: &DavReportPlanError,
    policy: &P,
) -> Result<DavResponse, DavXmlError> {
    match error {
        DavReportPlanError::InvalidLimits => {
            Ok(text_response(StatusCode::INTERNAL_SERVER_ERROR, ""))
        }
        DavReportPlanError::Xml(error) => xml_request_error_response(*error),
        DavReportPlanError::UnknownType { namespace, name } => {
            Ok(policy.unknown_type(namespace.as_deref(), name))
        }
        DavReportPlanError::NotAvailable { report } => Ok(policy.not_available(*report)),
    }
}

/// VERSION-CONTROL operation selected from target facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavVersionControlAction {
    PutUnderVersionControl,
    AlreadyControlled,
}

/// Transport-neutral VERSION-CONTROL plan passed to a product transaction port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavVersionControlPlan {
    pub action: DavVersionControlAction,
}

/// VERSION-CONTROL planning failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavVersionControlPlanError {
    #[error("VERSION-CONTROL is not allowed for this target")]
    MethodNotAllowed,
    #[error(transparent)]
    Xml(#[from] DavXmlError),
}

/// Plans VERSION-CONTROL without performing product persistence.
///
/// # Errors
///
/// Returns a typed failure when body grammar, method availability, or target facts reject it.
pub fn plan_version_control_request(
    snapshot: &DavCapabilitySnapshot,
    body: &[u8],
) -> Result<DavVersionControlPlan, DavVersionControlPlanError> {
    let DavVersioningMethodPlan::VersionControl(action) =
        plan_versioning_method(snapshot, DavMethod::VersionControl)
            .map_err(|_| DavVersionControlPlanError::MethodNotAllowed)?
    else {
        return Err(DavVersionControlPlanError::MethodNotAllowed);
    };
    parse_version_control_request(body)?;
    Ok(DavVersionControlPlan { action })
}

/// Product transaction result used only for RFC response composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavVersionControlResult {
    /// Optional future-extension children for `DAV:version-control-response`.
    pub response_extensions: Vec<DavXmlElement>,
}

/// Product-owned atomic VERSION-CONTROL transaction boundary.
#[async_trait]
pub trait DavVersionControlPort: Send + Sync {
    async fn version_control(
        &self,
        plan: DavVersionControlPlan,
    ) -> Result<DavVersionControlResult, DavBackendError>;
}

/// Executes an already validated VERSION-CONTROL plan through the product transaction port.
///
/// # Errors
///
/// Returns the product-neutral backend classification without mapping product error text.
pub async fn execute_version_control<P: DavVersionControlPort>(
    port: &P,
    plan: DavVersionControlPlan,
) -> Result<DavVersionControlResult, DavBackendError> {
    port.version_control(plan).await
}

/// Composes the optional successful VERSION-CONTROL response body.
///
/// # Errors
///
/// Returns [`DavXmlError`] when extension elements cannot be encoded safely.
pub fn version_control_response(
    result: DavVersionControlResult,
) -> Result<DavResponse, DavXmlError> {
    if result.response_extensions.is_empty() {
        return Ok(DavResponse::empty(StatusCode::OK));
    }
    let mut root = dav_element("version-control-response");
    root.namespaces
        .insert("D".to_owned(), DAV_NAMESPACE.to_owned());
    root.children.extend(
        result
            .response_extensions
            .into_iter()
            .map(DavXmlNode::Element),
    );
    xml_response(StatusCode::OK, &root)
}

/// Builds the protocol response for VERSION-CONTROL planning failure through its snapshot.
///
/// # Errors
///
/// Returns [`DavXmlError`] when an XML error body cannot be encoded.
pub fn version_control_plan_error_response(
    snapshot: &DavCapabilitySnapshot,
    error: DavVersionControlPlanError,
) -> Result<DavResponse, DavXmlError> {
    match error {
        DavVersionControlPlanError::MethodNotAllowed => {
            Ok(crate::method_not_allowed_response(snapshot))
        }
        DavVersionControlPlanError::Xml(error) => xml_request_error_response(error),
    }
}

/// RFC 3253 method action selected before a product mutation transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavVersioningMethodPlan {
    ReadOnly,
    DirectMutation,
    AutoCheckout,
    AutoCheckoutCheckin,
    AutoCheckinOnUnlock,
    DeleteVersion,
    VersionControl(DavVersionControlAction),
}

/// Typed RFC 3253 precondition selected by core method planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavVersioningPrecondition {
    #[error("method is not allowed by the capability snapshot")]
    MethodNotAllowed,
    #[error("checked-in content requires checkout or auto-version")]
    CannotModifyVersionControlledContent,
    #[error("checked-in dead properties require checkout or auto-version")]
    CannotModifyVersionControlledProperty,
    #[error("immutable version content or dead properties cannot be modified")]
    CannotModifyVersion,
    #[error("immutable versions cannot be renamed")]
    CannotRenameVersion,
    #[error("server policy rejects deleting an immutable version")]
    NoVersionDelete,
}

/// Plans core method behavior from the same target facts used for capability discovery.
///
/// # Errors
///
/// Returns a typed RFC precondition when the target state forbids the method.
pub fn plan_versioning_method(
    snapshot: &DavCapabilitySnapshot,
    method: DavMethod,
) -> Result<DavVersioningMethodPlan, DavVersioningPrecondition> {
    if !snapshot.allows(method) {
        return Err(DavVersioningPrecondition::MethodNotAllowed);
    }
    let facts = snapshot.declaration().versioning;
    match method {
        DavMethod::VersionControl => match facts.state {
            DavVersioningState::Versionable => Ok(DavVersioningMethodPlan::VersionControl(
                DavVersionControlAction::PutUnderVersionControl,
            )),
            DavVersioningState::CheckedIn | DavVersioningState::CheckedOut => Ok(
                DavVersioningMethodPlan::VersionControl(DavVersionControlAction::AlreadyControlled),
            ),
            DavVersioningState::Unsupported | DavVersioningState::Version => {
                Err(DavVersioningPrecondition::MethodNotAllowed)
            }
        },
        DavMethod::Put | DavMethod::Proppatch => match facts.state {
            DavVersioningState::Version => Err(DavVersioningPrecondition::CannotModifyVersion),
            DavVersioningState::CheckedIn => {
                let rejected = if method == DavMethod::Put {
                    DavVersioningPrecondition::CannotModifyVersionControlledContent
                } else {
                    DavVersioningPrecondition::CannotModifyVersionControlledProperty
                };
                match facts.auto_version {
                    DavAutoVersion::None => Err(rejected),
                    DavAutoVersion::CheckoutCheckin => {
                        Ok(DavVersioningMethodPlan::AutoCheckoutCheckin)
                    }
                    DavAutoVersion::CheckoutUnlockedCheckin => {
                        if facts.write_locked {
                            Ok(DavVersioningMethodPlan::AutoCheckout)
                        } else {
                            Ok(DavVersioningMethodPlan::AutoCheckoutCheckin)
                        }
                    }
                    DavAutoVersion::Checkout => Ok(DavVersioningMethodPlan::AutoCheckout),
                    DavAutoVersion::LockedCheckout => {
                        if facts.write_locked {
                            Ok(DavVersioningMethodPlan::AutoCheckout)
                        } else {
                            Err(rejected)
                        }
                    }
                }
            }
            DavVersioningState::Unsupported
            | DavVersioningState::Versionable
            | DavVersioningState::CheckedOut => Ok(DavVersioningMethodPlan::DirectMutation),
        },
        DavMethod::Move if facts.state == DavVersioningState::Version => {
            Err(DavVersioningPrecondition::CannotRenameVersion)
        }
        DavMethod::Delete if facts.state == DavVersioningState::Version => {
            if facts.allow_version_delete {
                Ok(DavVersioningMethodPlan::DeleteVersion)
            } else {
                Err(DavVersioningPrecondition::NoVersionDelete)
            }
        }
        DavMethod::Unlock if facts.auto_checkout_lock => {
            Ok(DavVersioningMethodPlan::AutoCheckinOnUnlock)
        }
        DavMethod::Delete | DavMethod::Copy | DavMethod::Move | DavMethod::Unlock => {
            Ok(DavVersioningMethodPlan::DirectMutation)
        }
        _ => Ok(DavVersioningMethodPlan::ReadOnly),
    }
}

/// Composes a DAV error response for a typed RFC 3253 precondition through its snapshot.
///
/// # Errors
///
/// Returns [`DavXmlError`] when the XML error body cannot be encoded.
pub fn versioning_precondition_response(
    snapshot: &DavCapabilitySnapshot,
    error: DavVersioningPrecondition,
) -> Result<DavResponse, DavXmlError> {
    if error == DavVersioningPrecondition::MethodNotAllowed {
        return Ok(crate::method_not_allowed_response(snapshot));
    }
    let condition = match error {
        DavVersioningPrecondition::CannotModifyVersionControlledContent => {
            DavErrorCondition::CannotModifyVersionControlledContent
        }
        DavVersioningPrecondition::CannotModifyVersionControlledProperty => {
            DavErrorCondition::CannotModifyVersionControlledProperty
        }
        DavVersioningPrecondition::CannotModifyVersion => DavErrorCondition::CannotModifyVersion,
        DavVersioningPrecondition::CannotRenameVersion => DavErrorCondition::CannotRenameVersion,
        DavVersioningPrecondition::NoVersionDelete => DavErrorCondition::NoVersionDelete,
        DavVersioningPrecondition::MethodNotAllowed => {
            return Ok(crate::method_not_allowed_response(snapshot));
        }
    };
    xml_response(StatusCode::FORBIDDEN, &dav_error_element(&condition))
}

/// One product-resolved property for an immutable version resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavVersionProperty {
    pub property: DavRequestedProperty,
    pub result: DavVersionPropertyResult,
}

impl DavVersionProperty {
    #[must_use]
    pub fn value(property: DavRequestedProperty, value: DavXmlElement) -> Self {
        Self {
            property,
            result: DavVersionPropertyResult::Value(value),
        }
    }

    #[must_use]
    pub fn text(property: DavRequestedProperty, value: impl Into<String>) -> Self {
        let element = dav_property_text_element(&property, value);
        Self::value(property, element)
    }

    #[must_use]
    pub fn hrefs(property: DavRequestedProperty, hrefs: impl IntoIterator<Item = String>) -> Self {
        let mut element = property_element(&property);
        element.children.extend(
            hrefs
                .into_iter()
                .map(|href| DavXmlNode::Element(dav_text_element("href", href))),
        );
        Self::value(property, element)
    }

    #[must_use]
    pub fn missing(property: DavRequestedProperty) -> Self {
        Self {
            property,
            result: DavVersionPropertyResult::Missing,
        }
    }

    #[must_use]
    pub fn backend_error(property: DavRequestedProperty, kind: DavBackendErrorKind) -> Self {
        Self {
            property,
            result: DavVersionPropertyResult::BackendError(kind),
        }
    }
}

/// Product property outcome grouped into canonical property-level propstats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavVersionPropertyResult {
    Value(DavXmlElement),
    Missing,
    BackendError(DavBackendErrorKind),
}

/// One version resource supplied by the product history adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavVersionReportItem {
    pub href: String,
    pub properties: Vec<DavVersionProperty>,
}

/// Builds a requested-property-driven bounded version-tree Multi-Status response.
///
/// # Errors
///
/// Returns [`DavMultiStatusError`] when the canonical response exceeds the supplied limits.
pub fn version_tree_response(
    request: &DavVersionTreeRequest,
    versions: Vec<DavVersionReportItem>,
) -> Result<DavResponse, DavMultiStatusError> {
    version_tree_response_with_limits(request, versions, DavMultiStatusLimits::default())
}

/// Builds a requested-property-driven bounded version-tree Multi-Status response.
///
/// # Errors
///
/// Returns [`DavMultiStatusError`] when the canonical response exceeds the supplied limits.
pub fn version_tree_response_with_limits(
    request: &DavVersionTreeRequest,
    versions: Vec<DavVersionReportItem>,
    limits: DavMultiStatusLimits,
) -> Result<DavResponse, DavMultiStatusError> {
    let requested = request
        .properties
        .clone()
        .unwrap_or_else(default_version_tree_properties);
    let items = versions
        .into_iter()
        .map(|version| version_multistatus_item(version, &requested));
    let mut response = DavResponse::bytes(
        StatusCode::MULTI_STATUS,
        dav_multistatus_bytes(items, limits)?,
    );
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    Ok(response)
}

/// Value returned by an expand-property backend port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavExpandPropertyValue {
    Element(DavXmlElement),
    Hrefs(Vec<String>),
}

/// Product port for resolving one property without exposing persistence entities to Forge.
#[async_trait]
pub trait DavExpandPropertyProvider: Send + Sync {
    async fn property(
        &self,
        href: &str,
        property: &DavRequestedProperty,
    ) -> Result<Option<DavExpandPropertyValue>, DavBackendError>;
}

/// Bounded expand-property execution failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavExpandPropertyError {
    #[error("invalid expand-property limits")]
    InvalidLimits,
    #[error("expand-property execution was cancelled")]
    Cancelled,
    #[error("expand-property cycle detected at {href}")]
    Cycle { href: String },
    #[error("expand-property depth limit exceeded")]
    DepthLimitExceeded,
    #[error("expand-property resource limit exceeded")]
    ResourceLimitExceeded,
    #[error("expand-property property limit exceeded")]
    PropertyLimitExceeded,
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    #[error(transparent)]
    MultiStatus(#[from] DavMultiStatusError),
}

/// Executes one bounded `DAV:expand-property` request and composes canonical nested responses.
///
/// Deadline handling is supplied by the same cancellation checkpoint: a transport or product
/// deadline flips its cancellation source, and Forge checks it before every property lookup and
/// nested href traversal.
///
/// # Errors
///
/// Returns a typed limit, cancellation, cycle, backend, or response-composition failure.
pub async fn execute_expand_property<P: DavExpandPropertyProvider, C: DavCancellation>(
    provider: &P,
    root_href: &str,
    request: &DavExpandPropertyRequest,
    limits: DavReportLimits,
    cancellation: &C,
) -> Result<DavResponse, DavExpandPropertyError> {
    if !limits.is_valid() {
        return Err(DavExpandPropertyError::InvalidLimits);
    }
    let mut state = DavExpandExecutionState {
        resources: 0,
        properties: 0,
        active_hrefs: HashSet::new(),
    };
    let item = expand_response_item(
        provider,
        root_href,
        &request.properties,
        0,
        limits,
        cancellation,
        &mut state,
    )
    .await?;
    let mut response = DavResponse::bytes(
        StatusCode::MULTI_STATUS,
        dav_multistatus_bytes([item], limits.multistatus)?,
    );
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    Ok(response)
}

struct DavExpandExecutionState {
    resources: usize,
    properties: usize,
    active_hrefs: HashSet<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "The nested expand-property executor keeps one bounded traversal transaction and one canonical response grouping boundary."
)]
fn expand_response_item<'a, P: DavExpandPropertyProvider, C: DavCancellation>(
    provider: &'a P,
    href: &'a str,
    selections: &'a [DavExpandPropertySelection],
    depth: usize,
    limits: DavReportLimits,
    cancellation: &'a C,
    state: &'a mut DavExpandExecutionState,
) -> Pin<Box<dyn Future<Output = Result<DavMultiStatusItem, DavExpandPropertyError>> + Send + 'a>> {
    Box::pin(async move {
        checkpoint(cancellation)?;
        if depth > limits.maximum_expansion_depth {
            return Err(DavExpandPropertyError::DepthLimitExceeded);
        }
        state.resources = state
            .resources
            .checked_add(1)
            .ok_or(DavExpandPropertyError::ResourceLimitExceeded)?;
        if state.resources > limits.maximum_expanded_resources {
            return Err(DavExpandPropertyError::ResourceLimitExceeded);
        }
        if !state.active_hrefs.insert(href.to_owned()) {
            return Err(DavExpandPropertyError::Cycle {
                href: href.to_owned(),
            });
        }

        let result = async {
            let mut groups: BTreeMap<u16, Vec<DavXmlElement>> = BTreeMap::new();
            for selection in selections {
                checkpoint(cancellation)?;
                state.properties = state
                    .properties
                    .checked_add(1)
                    .ok_or(DavExpandPropertyError::PropertyLimitExceeded)?;
                if state.properties > limits.maximum_expanded_properties {
                    return Err(DavExpandPropertyError::PropertyLimitExceeded);
                }
                match provider.property(href, &selection.property).await? {
                    None => groups
                        .entry(StatusCode::NOT_FOUND.as_u16())
                        .or_default()
                        .push(dav_property_name_element(&selection.property)),
                    Some(DavExpandPropertyValue::Element(element))
                        if selection.nested.is_empty() =>
                    {
                        groups
                            .entry(StatusCode::OK.as_u16())
                            .or_default()
                            .push(relexicalize(element, &selection.property));
                    }
                    Some(DavExpandPropertyValue::Hrefs(hrefs)) if selection.nested.is_empty() => {
                        let mut property = property_element(&selection.property);
                        property.children.extend(
                            hrefs
                                .into_iter()
                                .map(|href| DavXmlNode::Element(dav_text_element("href", href))),
                        );
                        groups
                            .entry(StatusCode::OK.as_u16())
                            .or_default()
                            .push(property);
                    }
                    Some(DavExpandPropertyValue::Hrefs(hrefs)) => {
                        let mut property = property_element(&selection.property);
                        for nested_href in hrefs {
                            checkpoint(cancellation)?;
                            match expand_response_item(
                                provider,
                                &nested_href,
                                &selection.nested,
                                depth + 1,
                                limits,
                                cancellation,
                                state,
                            )
                            .await
                            {
                                Ok(item) => property
                                    .children
                                    .push(DavXmlNode::Element(nested_response_element(item))),
                                Err(DavExpandPropertyError::Backend(error))
                                    if error.kind == DavBackendErrorKind::NotFound =>
                                {
                                    property.children.push(DavXmlNode::Element(
                                        nested_response_element(DavMultiStatusItem::status(
                                            nested_href,
                                            StatusCode::NOT_FOUND.as_u16(),
                                        )),
                                    ));
                                }
                                Err(error) => return Err(error),
                            }
                        }
                        groups
                            .entry(StatusCode::OK.as_u16())
                            .or_default()
                            .push(property);
                    }
                    Some(DavExpandPropertyValue::Element(_)) => {
                        groups
                            .entry(StatusCode::FORBIDDEN.as_u16())
                            .or_default()
                            .push(dav_property_name_element(&selection.property));
                    }
                }
            }
            if groups.is_empty() {
                groups.insert(StatusCode::OK.as_u16(), Vec::new());
            }
            Ok(DavMultiStatusItem::properties(
                href,
                groups
                    .into_iter()
                    .map(|(status, properties)| DavPropStat { status, properties })
                    .collect(),
            ))
        }
        .await;
        state.active_hrefs.remove(href);
        result
    })
}

fn checkpoint(cancellation: &impl DavCancellation) -> Result<(), DavExpandPropertyError> {
    if cancellation.is_cancelled() {
        Err(DavExpandPropertyError::Cancelled)
    } else {
        Ok(())
    }
}

fn version_multistatus_item(
    version: DavVersionReportItem,
    requested: &[DavRequestedProperty],
) -> DavMultiStatusItem {
    let mut groups: BTreeMap<u16, Vec<DavXmlElement>> = BTreeMap::new();
    for property in requested {
        let resolved = version.properties.iter().find(|candidate| {
            candidate.property.name == property.name
                && candidate.property.namespace == property.namespace
        });
        let (status, element) = match resolved.map(|property| &property.result) {
            Some(DavVersionPropertyResult::Value(element)) => {
                (StatusCode::OK, relexicalize(element.clone(), property))
            }
            Some(DavVersionPropertyResult::Missing) | None => {
                (StatusCode::NOT_FOUND, dav_property_name_element(property))
            }
            Some(DavVersionPropertyResult::BackendError(kind)) => {
                (backend_status(*kind), dav_property_name_element(property))
            }
        };
        groups.entry(status.as_u16()).or_default().push(element);
    }
    if groups.is_empty() {
        groups.insert(StatusCode::OK.as_u16(), Vec::new());
    }
    DavMultiStatusItem::properties(
        version.href,
        groups
            .into_iter()
            .map(|(status, properties)| DavPropStat { status, properties })
            .collect(),
    )
}

fn default_version_tree_properties() -> Vec<DavRequestedProperty> {
    [
        "version-name",
        "creator-displayname",
        "comment",
        "predecessor-set",
        "successor-set",
        "checkout-set",
        "getetag",
        "getcontentlength",
        "getcontenttype",
        "getlastmodified",
    ]
    .into_iter()
    .map(dav_property)
    .collect()
}

fn dav_property(name: &str) -> DavRequestedProperty {
    DavRequestedProperty {
        name: name.to_owned(),
        namespace: Some(DAV_NAMESPACE.to_owned()),
        prefix: Some("D".to_owned()),
    }
}

fn property_element(property: &DavRequestedProperty) -> DavXmlElement {
    let mut element = property.prefix.as_ref().map_or_else(
        || DavXmlElement::new(&property.name),
        |prefix| DavXmlElement::new(&format!("{prefix}:{}", property.name)),
    );
    element.namespace.clone_from(&property.namespace);
    if let Some(namespace) = &property.namespace {
        element.namespaces.insert(
            property.prefix.clone().unwrap_or_default(),
            namespace.clone(),
        );
    }
    element
}

fn relexicalize(mut element: DavXmlElement, property: &DavRequestedProperty) -> DavXmlElement {
    element.name.clone_from(&property.name);
    element.prefix.clone_from(&property.prefix);
    element.namespace.clone_from(&property.namespace);
    if let Some(namespace) = &property.namespace {
        element.namespaces.insert(
            property.prefix.clone().unwrap_or_default(),
            namespace.clone(),
        );
    }
    element
}

fn nested_response_element(item: DavMultiStatusItem) -> DavXmlElement {
    let mut response = dav_element("response");
    response
        .children
        .push(DavXmlNode::Element(dav_text_element("href", item.href)));
    if let Some(status) = item.status {
        response.children.push(DavXmlNode::Element(dav_text_element(
            "status",
            status_line(status),
        )));
    }
    for propstat in item.propstats {
        let mut element = dav_element("propstat");
        let mut prop = dav_element("prop");
        prop.children
            .extend(propstat.properties.into_iter().map(DavXmlNode::Element));
        element.children.push(DavXmlNode::Element(prop));
        element.children.push(DavXmlNode::Element(dav_text_element(
            "status",
            status_line(propstat.status),
        )));
        response.children.push(DavXmlNode::Element(element));
    }
    response
}

fn status_line(status: u16) -> String {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    format!(
        "HTTP/1.1 {} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown Status")
    )
}

fn backend_status(kind: DavBackendErrorKind) -> StatusCode {
    match kind {
        DavBackendErrorKind::NotFound => StatusCode::NOT_FOUND,
        DavBackendErrorKind::Forbidden => StatusCode::FORBIDDEN,
        DavBackendErrorKind::Conflict | DavBackendErrorKind::AlreadyExists => StatusCode::CONFLICT,
        DavBackendErrorKind::InsufficientStorage => StatusCode::INSUFFICIENT_STORAGE,
        DavBackendErrorKind::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        DavBackendErrorKind::Locked => StatusCode::LOCKED,
        DavBackendErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
        DavBackendErrorKind::Unsupported => StatusCode::NOT_IMPLEMENTED,
        DavBackendErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Maps an expand-property execution failure that occurs before a response starts.
///
/// # Errors
///
/// Returns [`DavXmlError`] when a DAV error response cannot be encoded.
pub fn expand_property_error_response(
    error: &DavExpandPropertyError,
) -> Result<DavResponse, DavXmlError> {
    let response = match error {
        DavExpandPropertyError::InvalidLimits => {
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "")
        }
        DavExpandPropertyError::Cancelled => text_response(StatusCode::SERVICE_UNAVAILABLE, ""),
        DavExpandPropertyError::Cycle { .. }
        | DavExpandPropertyError::DepthLimitExceeded
        | DavExpandPropertyError::ResourceLimitExceeded
        | DavExpandPropertyError::PropertyLimitExceeded => {
            text_response(StatusCode::INSUFFICIENT_STORAGE, "")
        }
        DavExpandPropertyError::Backend(error) => {
            return Ok(crate::backend_error_response(error));
        }
        DavExpandPropertyError::MultiStatus(error) => match &error.kind {
            DavMultiStatusErrorKind::ItemLimitExceeded
            | DavMultiStatusErrorKind::PropertyLimitExceeded
            | DavMultiStatusErrorKind::OutputLimitExceeded => {
                text_response(StatusCode::INSUFFICIENT_STORAGE, "")
            }
            DavMultiStatusErrorKind::Cancelled => {
                text_response(StatusCode::SERVICE_UNAVAILABLE, "")
            }
            DavMultiStatusErrorKind::Backend(error) => {
                return Ok(crate::backend_error_response(error));
            }
            DavMultiStatusErrorKind::InvalidLimits
            | DavMultiStatusErrorKind::InvalidItem
            | DavMultiStatusErrorKind::Xml
            | DavMultiStatusErrorKind::Write => {
                text_response(StatusCode::INTERNAL_SERVER_ERROR, "")
            }
        },
    };
    Ok(response)
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
            &dav_error_element(&DavErrorCondition::NoExternalEntities),
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

fn xml_response(status: StatusCode, root: &DavXmlElement) -> Result<DavResponse, DavXmlError> {
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
