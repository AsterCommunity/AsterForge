//! WebDAV property selection and propstat composition.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use http::StatusCode;

use crate::response::{xml_document_response, xml_request_error_response};
use crate::{
    DavErrorCondition, DavMultiStatusItem, DavPropStat, DavPropfindRequest, DavRequestedProperty,
    DavResponse, DavXmlElement, DavXmlError, dav_error_element, dav_multistatus_element,
    dav_property_name_element,
};

type PropertyKey = (String, Option<String>);

/// Formats a DAV `creationdate` value as RFC 3339 UTC text.
#[must_use]
pub fn format_creation_date(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

/// Atomic PROPPATCH execution decision and per-property protocol statuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavProppatchAtomicPlan {
    /// Whether the product adapter should persist every requested property mutation.
    pub apply: bool,
    /// Status assigned to each property in request order.
    pub statuses: Vec<StatusCode>,
}

/// Selects the RFC 4918 atomic PROPPATCH statuses from product-owned protection decisions.
///
/// When any property is protected, that property receives `403` and every otherwise valid
/// property receives `424`. If none are protected, every property receives `200` and the adapter
/// may apply the entire transaction.
pub fn plan_atomic_proppatch(protected: impl IntoIterator<Item = bool>) -> DavProppatchAtomicPlan {
    let protected = protected.into_iter().collect::<Vec<_>>();
    let has_protected = protected.iter().any(|protected| *protected);
    let statuses = protected
        .into_iter()
        .map(|protected| {
            if has_protected {
                if protected {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::FAILED_DEPENDENCY
                }
            } else {
                StatusCode::OK
            }
        })
        .collect();
    DavProppatchAtomicPlan {
        apply: !has_protected,
        statuses,
    }
}

/// Builds one PROPFIND multistatus item from a product-supplied property catalog.
///
/// `available` contains every property exposed by `allprop`/`propname`. `resolve` supplies a value
/// for the requested QName and may intentionally return `None` for unavailable or hidden values.
pub fn build_propfind_item<E, F>(
    href: String,
    request: &DavPropfindRequest,
    available: &[DavRequestedProperty],
    mut resolve: F,
) -> Result<DavMultiStatusItem, E>
where
    F: FnMut(&DavRequestedProperty) -> Result<Option<DavXmlElement>, E>,
{
    let groups = match request {
        DavPropfindRequest::AllProp { include } => {
            let mut keys = available.iter().map(property_key).collect::<BTreeSet<_>>();
            let mut ok = resolve_available(available, &mut resolve)?;
            let extra = include
                .iter()
                .filter(|property| keys.insert(property_key(property)))
                .cloned()
                .collect::<Vec<_>>();
            let (mut included, missing) = resolve_requested(&extra, &mut resolve)?;
            ok.append(&mut included);
            propstat_groups(ok, missing)
        }
        DavPropfindRequest::PropName => vec![DavPropStat {
            status: 200,
            properties: available.iter().map(dav_property_name_element).collect(),
        }],
        DavPropfindRequest::Prop(requested) => {
            let (ok, missing) = resolve_requested(requested, &mut resolve)?;
            propstat_groups(ok, missing)
        }
    };
    Ok(DavMultiStatusItem::properties(href, groups))
}

/// Builds one PROPPATCH multistatus item by grouping property outcomes by status.
pub fn build_proppatch_item(
    href: String,
    outcomes: impl IntoIterator<Item = (u16, DavXmlElement)>,
) -> DavMultiStatusItem {
    let mut groups = BTreeMap::<u16, Vec<DavXmlElement>>::new();
    for (status, property) in outcomes {
        groups.entry(status).or_default().push(property);
    }
    DavMultiStatusItem::properties(
        href,
        groups
            .into_iter()
            .map(|(status, properties)| DavPropStat { status, properties })
            .collect(),
    )
}

/// Builds the 207 XML response for PROPFIND or PROPPATCH items.
pub fn property_multistatus_response(
    items: Vec<DavMultiStatusItem>,
) -> Result<DavResponse, DavXmlError> {
    xml_document_response(StatusCode::MULTI_STATUS, dav_multistatus_element(items))
}

/// Maps PROPFIND XML failures to their protocol response.
pub fn propfind_xml_error_response(error: DavXmlError) -> Result<DavResponse, DavXmlError> {
    xml_request_error_response(error, "Invalid PROPFIND body")
}

/// Maps PROPPATCH XML failures to their protocol response.
pub fn proppatch_xml_error_response(error: DavXmlError) -> Result<DavResponse, DavXmlError> {
    xml_request_error_response(error, "Invalid PROPPATCH body")
}

/// Builds the RFC 4918 finite-depth precondition response.
pub fn propfind_finite_depth_response() -> Result<DavResponse, DavXmlError> {
    xml_document_response(
        StatusCode::FORBIDDEN,
        dav_error_element(&DavErrorCondition::PropfindFiniteDepth),
    )
}

/// Returns a stable label for protocol metrics and tracing.
#[must_use]
pub const fn propfind_request_label(request: &DavPropfindRequest) -> &'static str {
    match request {
        DavPropfindRequest::AllProp { .. } => "allprop",
        DavPropfindRequest::PropName => "propname",
        DavPropfindRequest::Prop(_) => "prop",
    }
}

fn resolve_available<E, F>(
    available: &[DavRequestedProperty],
    resolve: &mut F,
) -> Result<Vec<DavXmlElement>, E>
where
    F: FnMut(&DavRequestedProperty) -> Result<Option<DavXmlElement>, E>,
{
    let mut elements = Vec::with_capacity(available.len());
    for property in available {
        if let Some(element) = resolve(property)? {
            elements.push(element);
        }
    }
    Ok(elements)
}

fn resolve_requested<E, F>(
    requested: &[DavRequestedProperty],
    resolve: &mut F,
) -> Result<(Vec<DavXmlElement>, Vec<DavXmlElement>), E>
where
    F: FnMut(&DavRequestedProperty) -> Result<Option<DavXmlElement>, E>,
{
    let mut ok = Vec::new();
    let mut missing = Vec::new();
    for property in requested {
        match resolve(property)? {
            Some(element) => ok.push(element),
            None => missing.push(dav_property_name_element(property)),
        }
    }
    Ok((ok, missing))
}

fn propstat_groups(ok: Vec<DavXmlElement>, missing: Vec<DavXmlElement>) -> Vec<DavPropStat> {
    let mut groups = Vec::with_capacity(2);
    if !ok.is_empty() {
        groups.push(DavPropStat {
            status: 200,
            properties: ok,
        });
    }
    if !missing.is_empty() {
        groups.push(DavPropStat {
            status: 404,
            properties: missing,
        });
    }
    groups
}

fn property_key(property: &DavRequestedProperty) -> PropertyKey {
    (property.name.clone(), property.namespace.clone())
}
