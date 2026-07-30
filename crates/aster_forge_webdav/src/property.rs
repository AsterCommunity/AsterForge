//! WebDAV property selection and propstat composition.

use std::collections::BTreeMap;
use std::time::SystemTime;

use http::header::CONTENT_TYPE;
use http::{HeaderValue, StatusCode};

use crate::response::{xml_document_response, xml_request_error_response};
use crate::{
    DavBackendError, DavCapabilitySnapshot, DavErrorCondition, DavExtensionPackage,
    DavLiveProperty, DavLockXml, DavMultiStatusError, DavMultiStatusItem, DavMultiStatusLimits,
    DavPath, DavProp, DavPropStat, DavPropfindRequest, DavRequestedProperty, DavResourceState,
    DavResponse, DavXmlElement, DavXmlError, DavXmlNode, dav_dead_property_element, dav_element,
    dav_error_element, dav_lock_discovery_element, dav_multistatus_bytes,
    dav_property_child_element, dav_property_name_element, dav_property_text_element,
    dav_supported_lock_element,
};
use aster_forge_utils::http_validators;
use aster_forge_utils::url::parse_absolute_url;

/// Formats a DAV `creationdate` value as RFC 3339 UTC text.
#[must_use]
pub fn format_creation_date(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

/// Metadata values fetched once for all standard content and validator properties.
#[derive(Debug, Clone, Copy, Default)]
pub struct DavLivePropertyMetadata<'a> {
    pub creation_date: Option<SystemTime>,
    pub display_name: Option<&'a str>,
    pub content_language: Option<&'a str>,
    pub content_length: Option<u64>,
    pub content_type: Option<&'a str>,
    /// A complete entity-tag value suitable for `DAV:getetag`.
    pub etag: Option<&'a str>,
    pub last_modified: Option<SystemTime>,
}

/// One RFC 4331 scope snapshot reused for both quota properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavQuotaSnapshot {
    pub used_bytes: u64,
    /// `None` represents an infinite limit and omits `quota-available-bytes` from `propname`.
    pub available_bytes: Option<u64>,
}

/// RFC 5397 current principal value computed once for the request principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavCurrentPrincipal<'a> {
    /// Absolute HTTP(S) principal URL required by RFC 5397.
    Href(&'a str),
    /// RFC 5397 pseudo-principal used when no authenticated principal exists.
    Unauthenticated,
}

/// Value groups that a product adapter should fetch in one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavLivePropertyRequirements {
    pub metadata: bool,
    pub locks: bool,
    pub dead_properties: bool,
    pub quota: bool,
    pub current_principal: bool,
    pub sync_token: bool,
    pub add_member: bool,
    pub extension_values: bool,
}

/// Product-owned batch snapshot consumed by the property composer.
pub trait DavLivePropertyValueSnapshot: Send {
    fn metadata(&self) -> DavLivePropertyMetadata<'_> {
        DavLivePropertyMetadata::default()
    }

    fn active_locks(&self) -> &[DavLockXml] {
        &[]
    }

    fn dead_properties(&self) -> &[DavProp] {
        &[]
    }

    fn quota(&self) -> Option<DavQuotaSnapshot> {
        None
    }

    fn current_principal(&self) -> Option<DavCurrentPrincipal<'_>> {
        None
    }

    fn sync_token(&self) -> Option<&str> {
        None
    }

    fn add_member_href(&self) -> Option<&str> {
        None
    }

    /// Supplies complex ACL, DeltaV, ordering, redirect, or binding property XML by typed ID.
    fn extension_value(&self, _property: DavLiveProperty) -> Option<&DavXmlElement> {
        None
    }
}

/// Batch product port used by the canonical live-property entrypoint.
#[expect(
    async_fn_in_trait,
    reason = "The provider is generic and intentionally preserves native async futures."
)]
pub trait DavLivePropertyProvider: Send + Sync {
    type Values: DavLivePropertyValueSnapshot;

    async fn live_property_values(
        &self,
        path: &DavPath,
        requirements: DavLivePropertyRequirements,
    ) -> Result<Self::Values, DavBackendError>;
}

/// Failure while rendering authoritative product values as WebDAV live properties.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavLivePropertyError {
    #[error("invalid representation for WebDAV live property {property:?}")]
    InvalidRepresentation { property: DavLiveProperty },
    #[error("required WebDAV live property value is missing: {property:?}")]
    MissingRequiredValue { property: DavLiveProperty },
}

/// Failure across the product batch port and property representation boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavLivePropertyEvaluationError {
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    #[error(transparent)]
    Property(#[from] DavLivePropertyError),
}

/// Computes the value groups required by one PROPFIND selection without allocating.
#[must_use]
pub fn live_property_requirements(
    snapshot: &DavCapabilitySnapshot,
    request: &DavPropfindRequest,
) -> DavLivePropertyRequirements {
    let mut requirements = DavLivePropertyRequirements::default();
    match request {
        DavPropfindRequest::AllProp { include } => {
            requirements.metadata = true;
            requirements.locks = snapshot.declaration().compliance.class1;
            requirements.dead_properties = true;
            requirements.current_principal =
                snapshot.supports_extension(DavExtensionPackage::CurrentPrincipal);
            for property in include {
                add_requirement(snapshot, property, &mut requirements);
            }
        }
        DavPropfindRequest::PropName => {
            requirements.metadata = true;
            requirements.locks = snapshot.declaration().compliance.class1;
            requirements.dead_properties = true;
            requirements.quota = snapshot.supports_extension(DavExtensionPackage::Quota);
            requirements.current_principal =
                snapshot.supports_extension(DavExtensionPackage::CurrentPrincipal);
            requirements.sync_token =
                snapshot.supports_extension(DavExtensionPackage::CollectionSync);
            requirements.add_member = snapshot.supports_extension(DavExtensionPackage::AddMember);
            requirements.extension_values = !snapshot.extensions().is_empty();
        }
        DavPropfindRequest::Prop(properties) => {
            for property in properties {
                add_requirement(snapshot, property, &mut requirements);
            }
        }
    }
    requirements
}

/// Fetches one product snapshot and composes a capability-driven PROPFIND item.
pub async fn build_live_propfind_item_with_provider<P: DavLivePropertyProvider>(
    provider: &P,
    path: &DavPath,
    href: String,
    snapshot: &DavCapabilitySnapshot,
    request: &DavPropfindRequest,
) -> Result<DavMultiStatusItem, DavLivePropertyEvaluationError> {
    let requirements = live_property_requirements(snapshot, request);
    let values = provider.live_property_values(path, requirements).await?;
    build_live_propfind_item(href, snapshot, request, &values).map_err(Into::into)
}

/// Composes one PROPFIND item from the validated capability and one product value snapshot.
pub fn build_live_propfind_item<V: DavLivePropertyValueSnapshot>(
    href: String,
    snapshot: &DavCapabilitySnapshot,
    request: &DavPropfindRequest,
    values: &V,
) -> Result<DavMultiStatusItem, DavLivePropertyError> {
    let groups = match request {
        DavPropfindRequest::AllProp { include } => {
            let mut ok = Vec::new();
            for_each_catalog_property(snapshot, |property| {
                if property_in_allprop(property)
                    && let Some(element) = resolve_live_property(
                        snapshot,
                        values,
                        property,
                        &canonical_property(property),
                    )?
                {
                    ok.push(element);
                }
                Ok(())
            })?;
            append_all_dead_properties(values.dead_properties(), &mut ok);

            let mut missing = Vec::new();
            for requested in include {
                if contains_expanded_name(&ok, requested) {
                    continue;
                }
                match resolve_requested_property(snapshot, values, requested)? {
                    Some(element) => ok.push(element),
                    None => missing.push(dav_property_name_element(requested)),
                }
            }
            propstat_groups(ok, missing)
        }
        DavPropfindRequest::PropName => {
            let mut names = Vec::new();
            for_each_catalog_property(snapshot, |property| {
                if property_is_defined(snapshot, values, property) {
                    names.push(dav_property_name_element(&canonical_property(property)));
                }
                Ok(())
            })?;
            for dead in values.dead_properties() {
                names.push(dav_property_name_element(&dead_property_name(dead)));
            }
            vec![DavPropStat {
                status: 200,
                properties: names,
            }]
        }
        DavPropfindRequest::Prop(requested) => {
            let mut ok = Vec::new();
            let mut missing = Vec::new();
            for property in requested {
                match resolve_requested_property(snapshot, values, property)? {
                    Some(element) => ok.push(element),
                    None => missing.push(dav_property_name_element(property)),
                }
            }
            propstat_groups(ok, missing)
        }
    };
    Ok(DavMultiStatusItem::properties(href, groups))
}

/// Returns whether a requested property is a protected standard live property.
#[must_use]
pub fn is_protected_live_property(
    snapshot: &DavCapabilitySnapshot,
    requested: &DavRequestedProperty,
) -> bool {
    live_property(requested).is_some_and(|property| {
        (is_base_property(property) || snapshot.supports_live_property(property))
            && property_is_protected(property)
    })
}

const BASE_LIVE_PROPERTIES: [DavLiveProperty; 10] = [
    DavLiveProperty::CreationDate,
    DavLiveProperty::DisplayName,
    DavLiveProperty::GetContentLanguage,
    DavLiveProperty::GetContentLength,
    DavLiveProperty::GetContentType,
    DavLiveProperty::GetEtag,
    DavLiveProperty::GetLastModified,
    DavLiveProperty::LockDiscovery,
    DavLiveProperty::ResourceType,
    DavLiveProperty::SupportedLock,
];

fn add_requirement(
    snapshot: &DavCapabilitySnapshot,
    requested: &DavRequestedProperty,
    requirements: &mut DavLivePropertyRequirements,
) {
    let Some(property) = live_property(requested) else {
        requirements.dead_properties = true;
        return;
    };
    if !is_base_property(property) && !snapshot.supports_live_property(property) {
        requirements.dead_properties = true;
        return;
    }
    match property {
        DavLiveProperty::CreationDate
        | DavLiveProperty::DisplayName
        | DavLiveProperty::GetContentLanguage
        | DavLiveProperty::GetContentLength
        | DavLiveProperty::GetContentType
        | DavLiveProperty::GetEtag
        | DavLiveProperty::GetLastModified
        | DavLiveProperty::ResourceType => requirements.metadata = true,
        DavLiveProperty::LockDiscovery | DavLiveProperty::SupportedLock => {
            requirements.locks = true;
        }
        DavLiveProperty::QuotaAvailableBytes | DavLiveProperty::QuotaUsedBytes => {
            requirements.quota = snapshot.supports_extension(DavExtensionPackage::Quota);
        }
        DavLiveProperty::CurrentUserPrincipal => {
            requirements.current_principal =
                snapshot.supports_extension(DavExtensionPackage::CurrentPrincipal);
        }
        DavLiveProperty::SyncToken => {
            requirements.sync_token =
                snapshot.supports_extension(DavExtensionPackage::CollectionSync);
        }
        DavLiveProperty::AddMember => {
            requirements.add_member = snapshot.supports_extension(DavExtensionPackage::AddMember);
        }
        _ => requirements.extension_values = snapshot.supports_live_property(property),
    }
}

fn for_each_catalog_property(
    snapshot: &DavCapabilitySnapshot,
    mut visit: impl FnMut(DavLiveProperty) -> Result<(), DavLivePropertyError>,
) -> Result<(), DavLivePropertyError> {
    for property in BASE_LIVE_PROPERTIES {
        visit(property)?;
    }
    for package in snapshot.extensions().iter() {
        for property in package.descriptor().live_properties {
            visit(*property)?;
        }
    }
    Ok(())
}

fn property_in_allprop(property: DavLiveProperty) -> bool {
    is_base_property(property) || property == DavLiveProperty::CurrentUserPrincipal
}

fn property_is_protected(property: DavLiveProperty) -> bool {
    matches!(
        property,
        DavLiveProperty::GetContentLength
            | DavLiveProperty::GetEtag
            | DavLiveProperty::GetLastModified
            | DavLiveProperty::LockDiscovery
            | DavLiveProperty::ResourceType
            | DavLiveProperty::SupportedLock
            | DavLiveProperty::AlternateUriSet
            | DavLiveProperty::PrincipalUrl
            | DavLiveProperty::GroupMembership
            | DavLiveProperty::SupportedPrivilegeSet
            | DavLiveProperty::CurrentUserPrivilegeSet
            | DavLiveProperty::Acl
            | DavLiveProperty::AclRestrictions
            | DavLiveProperty::InheritedAclSet
            | DavLiveProperty::PrincipalCollectionSet
            | DavLiveProperty::SupportedMethodSet
            | DavLiveProperty::SupportedLivePropertySet
            | DavLiveProperty::SupportedReportSet
            | DavLiveProperty::CheckedIn
            | DavLiveProperty::CheckedOut
            | DavLiveProperty::SuccessorSet
            | DavLiveProperty::CheckoutSet
            | DavLiveProperty::VersionName
            | DavLiveProperty::VersionSet
            | DavLiveProperty::RootVersion
            | DavLiveProperty::VersionHistory
            | DavLiveProperty::WorkspaceCheckoutSet
            | DavLiveProperty::Workspace
            | DavLiveProperty::LabelNameSet
            | DavLiveProperty::AutoUpdate
            | DavLiveProperty::BaselineControlledCollection
            | DavLiveProperty::BaselineCollection
            | DavLiveProperty::VersionControlledConfiguration
            | DavLiveProperty::BaselineControlledCollectionSet
            | DavLiveProperty::ActivityVersionSet
            | DavLiveProperty::ActivityCheckoutSet
            | DavLiveProperty::CurrentWorkspaceSet
            | DavLiveProperty::EclipsedSet
            | DavLiveProperty::VersionControlledBindingSet
            | DavLiveProperty::SupportedQueryGrammarSet
            | DavLiveProperty::QuotaAvailableBytes
            | DavLiveProperty::QuotaUsedBytes
            | DavLiveProperty::SyncToken
            | DavLiveProperty::CurrentUserPrincipal
            | DavLiveProperty::OrderingType
            | DavLiveProperty::RedirectLifetime
            | DavLiveProperty::RefTarget
            | DavLiveProperty::ResourceId
            | DavLiveProperty::ParentSet
            | DavLiveProperty::AddMember
    )
}

fn property_is_defined<V: DavLivePropertyValueSnapshot>(
    snapshot: &DavCapabilitySnapshot,
    values: &V,
    property: DavLiveProperty,
) -> bool {
    let metadata = values.metadata();
    match property {
        DavLiveProperty::CreationDate => metadata.creation_date.is_some(),
        DavLiveProperty::DisplayName => metadata.display_name.is_some(),
        DavLiveProperty::GetContentLanguage => metadata.content_language.is_some(),
        DavLiveProperty::GetContentLength => metadata.content_length.is_some(),
        DavLiveProperty::GetContentType => metadata.content_type.is_some(),
        DavLiveProperty::GetEtag => metadata.etag.is_some(),
        DavLiveProperty::GetLastModified => metadata.last_modified.is_some(),
        DavLiveProperty::LockDiscovery | DavLiveProperty::SupportedLock => {
            snapshot.declaration().compliance.class1
        }
        DavLiveProperty::ResourceType => {
            snapshot.declaration().resource != DavResourceState::Unmapped
        }
        DavLiveProperty::QuotaAvailableBytes => values
            .quota()
            .and_then(|quota| quota.available_bytes)
            .is_some(),
        DavLiveProperty::QuotaUsedBytes => values.quota().is_some(),
        DavLiveProperty::SyncToken => {
            snapshot.supports_extension(DavExtensionPackage::CollectionSync)
        }
        DavLiveProperty::CurrentUserPrincipal => {
            snapshot.supports_extension(DavExtensionPackage::CurrentPrincipal)
        }
        DavLiveProperty::AddMember => snapshot.supports_extension(DavExtensionPackage::AddMember),
        DavLiveProperty::SupportedMethodSet
        | DavLiveProperty::SupportedLivePropertySet
        | DavLiveProperty::SupportedReportSet => {
            snapshot.supports_extension(DavExtensionPackage::VersionControl)
        }
        DavLiveProperty::SupportedQueryGrammarSet => {
            snapshot.supports_extension(DavExtensionPackage::Search)
        }
        _ => values.extension_value(property).is_some(),
    }
}

fn resolve_requested_property<V: DavLivePropertyValueSnapshot>(
    snapshot: &DavCapabilitySnapshot,
    values: &V,
    requested: &DavRequestedProperty,
) -> Result<Option<DavXmlElement>, DavLivePropertyError> {
    if let Some(property) = live_property(requested)
        && (is_base_property(property) || snapshot.supports_live_property(property))
    {
        return resolve_live_property(snapshot, values, property, requested);
    }
    Ok(
        find_dead_property(values.dead_properties(), requested).map(|dead| {
            dav_dead_property_element(
                &dead_property_name(dead),
                Some(requested),
                dead.xml.as_deref(),
            )
        }),
    )
}

fn resolve_live_property<V: DavLivePropertyValueSnapshot>(
    snapshot: &DavCapabilitySnapshot,
    values: &V,
    property: DavLiveProperty,
    requested: &DavRequestedProperty,
) -> Result<Option<DavXmlElement>, DavLivePropertyError> {
    let metadata = values.metadata();
    let element = match property {
        DavLiveProperty::CreationDate => metadata
            .creation_date
            .map(|value| dav_property_text_element(requested, format_creation_date(value))),
        DavLiveProperty::DisplayName => metadata
            .display_name
            .map(|value| dav_property_text_element(requested, value)),
        DavLiveProperty::GetContentLanguage => metadata
            .content_language
            .map(|value| dav_property_text_element(requested, value)),
        DavLiveProperty::GetContentLength => metadata
            .content_length
            .map(|value| dav_property_text_element(requested, value.to_string())),
        DavLiveProperty::GetContentType => metadata
            .content_type
            .map(|value| dav_property_text_element(requested, value)),
        DavLiveProperty::GetEtag => metadata
            .etag
            .map(|value| dav_property_text_element(requested, value)),
        DavLiveProperty::GetLastModified => match metadata.last_modified {
            Some(value) => Some(dav_property_text_element(
                requested,
                http_validators::try_format_http_date(value)
                    .map_err(|_| DavLivePropertyError::InvalidRepresentation { property })?,
            )),
            None => None,
        },
        DavLiveProperty::ResourceType => resource_type_element(snapshot, requested),
        DavLiveProperty::SupportedLock => {
            let element = if snapshot.declaration().locking == crate::DavLockingCapability::Class2 {
                dav_supported_lock_element()
            } else {
                dav_element("supportedlock")
            };
            Some(relexicalize(element, requested))
        }
        DavLiveProperty::LockDiscovery => Some(relexicalize(
            dav_lock_discovery_element(values.active_locks()),
            requested,
        )),
        DavLiveProperty::SupportedMethodSet => Some(supported_method_set(snapshot, requested)),
        DavLiveProperty::SupportedLivePropertySet => {
            Some(supported_live_property_set(snapshot, requested)?)
        }
        DavLiveProperty::SupportedReportSet => Some(supported_report_set(snapshot, requested)),
        DavLiveProperty::SupportedQueryGrammarSet => {
            Some(supported_query_grammar_set(snapshot, requested))
        }
        DavLiveProperty::QuotaUsedBytes => match values.quota() {
            Some(quota) => Some(dav_property_text_element(
                requested,
                quota.used_bytes.to_string(),
            )),
            None if matches!(
                snapshot.declaration().resource,
                DavResourceState::Collection | DavResourceState::MountRoot
            ) =>
            {
                return Err(DavLivePropertyError::MissingRequiredValue { property });
            }
            None => None,
        },
        DavLiveProperty::QuotaAvailableBytes => values.quota().and_then(|quota| {
            quota
                .available_bytes
                .map(|value| dav_property_text_element(requested, value.to_string()))
        }),
        DavLiveProperty::SyncToken => {
            Some(required_uri_text(values.sync_token(), property, requested)?)
        }
        DavLiveProperty::CurrentUserPrincipal => Some(current_principal_element(
            values.current_principal(),
            requested,
        )?),
        DavLiveProperty::AddMember => Some(required_href_element(
            values.add_member_href(),
            property,
            requested,
        )?),
        _ => values
            .extension_value(property)
            .cloned()
            .map(|element| relexicalize(element, requested)),
    };
    Ok(element)
}

fn supported_method_set(
    snapshot: &DavCapabilitySnapshot,
    requested: &DavRequestedProperty,
) -> DavXmlElement {
    let mut root = dav_property_name_element(requested);
    for method in snapshot.supported_methods().iter() {
        let mut supported = dav_element("supported-method");
        supported
            .attributes
            .insert("name".to_owned(), method.as_str().to_owned());
        root.children.push(DavXmlNode::Element(supported));
    }
    root
}

fn supported_live_property_set(
    snapshot: &DavCapabilitySnapshot,
    requested: &DavRequestedProperty,
) -> Result<DavXmlElement, DavLivePropertyError> {
    let mut root = dav_property_name_element(requested);
    for_each_catalog_property(snapshot, |property| {
        let mut supported = dav_element("supported-live-property");
        let mut prop = dav_element("prop");
        prop.children
            .push(DavXmlNode::Element(dav_element(property.local_name())));
        supported.children.push(DavXmlNode::Element(prop));
        root.children.push(DavXmlNode::Element(supported));
        Ok(())
    })?;
    Ok(root)
}

fn supported_report_set(
    snapshot: &DavCapabilitySnapshot,
    requested: &DavRequestedProperty,
) -> DavXmlElement {
    let mut root = dav_property_name_element(requested);
    for package in snapshot.extensions().iter() {
        for report in package.descriptor().reports {
            let mut supported = dav_element("supported-report");
            let mut report_element = dav_element("report");
            report_element
                .children
                .push(DavXmlNode::Element(dav_element(report.local_name())));
            supported.children.push(DavXmlNode::Element(report_element));
            root.children.push(DavXmlNode::Element(supported));
        }
    }
    root
}

fn supported_query_grammar_set(
    snapshot: &DavCapabilitySnapshot,
    requested: &DavRequestedProperty,
) -> DavXmlElement {
    let mut root = dav_property_name_element(requested);
    for grammar in snapshot.declaration().search.grammars {
        let mut supported = dav_element("supported-query-grammar");
        let mut grammar_element = dav_element("grammar");
        let mut grammar_type = DavXmlElement::new(grammar.xml_local_name);
        if !grammar.xml_namespace.is_empty() {
            grammar_type.namespace = Some(grammar.xml_namespace.to_owned());
        }
        grammar_element
            .children
            .push(DavXmlNode::Element(grammar_type));
        supported
            .children
            .push(DavXmlNode::Element(grammar_element));
        root.children.push(DavXmlNode::Element(supported));
    }
    root
}

fn resource_type_element(
    snapshot: &DavCapabilitySnapshot,
    requested: &DavRequestedProperty,
) -> Option<DavXmlElement> {
    let mut root = dav_property_name_element(requested);
    let child = match snapshot.declaration().resource {
        DavResourceState::Unmapped
        | DavResourceState::File
        | DavResourceState::AddMemberEndpoint => None,
        DavResourceState::Collection | DavResourceState::MountRoot => Some("collection"),
        DavResourceState::Principal => Some("principal"),
        DavResourceState::RedirectReference => Some("redirectref"),
    };
    if let Some(child) = child {
        root.children.push(DavXmlNode::Element(dav_element(child)));
    }
    (snapshot.declaration().resource != DavResourceState::Unmapped).then_some(root)
}

fn required_uri_text(
    value: Option<&str>,
    property: DavLiveProperty,
    requested: &DavRequestedProperty,
) -> Result<DavXmlElement, DavLivePropertyError> {
    let value = value.ok_or(DavLivePropertyError::MissingRequiredValue { property })?;
    validate_absolute_uri(value, property)?;
    Ok(dav_property_text_element(requested, value))
}

fn required_href_element(
    value: Option<&str>,
    property: DavLiveProperty,
    requested: &DavRequestedProperty,
) -> Result<DavXmlElement, DavLivePropertyError> {
    let value = value.ok_or(DavLivePropertyError::MissingRequiredValue { property })?;
    validate_add_member_uri(value, property)?;
    Ok(dav_property_child_element(
        requested,
        crate::dav_text_element("href", value),
    ))
}

fn current_principal_element(
    value: Option<DavCurrentPrincipal<'_>>,
    requested: &DavRequestedProperty,
) -> Result<DavXmlElement, DavLivePropertyError> {
    let value = value.ok_or(DavLivePropertyError::MissingRequiredValue {
        property: DavLiveProperty::CurrentUserPrincipal,
    })?;
    let child = match value {
        DavCurrentPrincipal::Href(href) => {
            validate_current_principal_url(href)?;
            crate::dav_text_element("href", href)
        }
        DavCurrentPrincipal::Unauthenticated => dav_element("unauthenticated"),
    };
    Ok(dav_property_child_element(requested, child))
}

fn validate_absolute_uri(
    value: &str,
    property: DavLiveProperty,
) -> Result<(), DavLivePropertyError> {
    if value.trim() == value && parse_absolute_url(value, "WebDAV live property").is_ok() {
        Ok(())
    } else {
        Err(DavLivePropertyError::InvalidRepresentation { property })
    }
}

fn validate_current_principal_url(value: &str) -> Result<(), DavLivePropertyError> {
    let valid = value.parse::<http::Uri>().is_ok_and(|uri| {
        uri.scheme_str().is_some_and(|scheme| {
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
        }) && uri.authority().is_some()
    });
    if valid {
        Ok(())
    } else {
        Err(DavLivePropertyError::InvalidRepresentation {
            property: DavLiveProperty::CurrentUserPrincipal,
        })
    }
}

fn validate_add_member_uri(
    value: &str,
    property: DavLiveProperty,
) -> Result<(), DavLivePropertyError> {
    let valid = value.parse::<http::Uri>().is_ok_and(|uri| {
        uri.scheme_str().map_or_else(
            || uri.path().starts_with('/') && !value.starts_with("//"),
            |scheme| {
                (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
                    && uri.authority().is_some()
            },
        )
    });
    if valid {
        Ok(())
    } else {
        Err(DavLivePropertyError::InvalidRepresentation { property })
    }
}

fn relexicalize(mut element: DavXmlElement, requested: &DavRequestedProperty) -> DavXmlElement {
    element.name.clone_from(&requested.name);
    element.prefix.clone_from(&requested.prefix);
    element.namespace.clone_from(&requested.namespace);
    element
}

fn append_all_dead_properties(dead: &[DavProp], output: &mut Vec<DavXmlElement>) {
    for property in dead {
        let name = dead_property_name(property);
        if !contains_expanded_name(output, &name) {
            output.push(dav_dead_property_element(
                &name,
                None,
                property.xml.as_deref(),
            ));
        }
    }
}

fn find_dead_property<'a>(
    dead: &'a [DavProp],
    requested: &DavRequestedProperty,
) -> Option<&'a DavProp> {
    dead.iter().find(|property| {
        property.name == requested.name && property.namespace == requested.namespace
    })
}

fn dead_property_name(property: &DavProp) -> DavRequestedProperty {
    DavRequestedProperty {
        name: property.name.clone(),
        namespace: property.namespace.clone(),
        prefix: property.prefix.clone(),
    }
}

fn canonical_property(property: DavLiveProperty) -> DavRequestedProperty {
    DavRequestedProperty {
        name: property.local_name().to_owned(),
        namespace: Some("DAV:".to_owned()),
        prefix: Some("D".to_owned()),
    }
}

fn live_property(requested: &DavRequestedProperty) -> Option<DavLiveProperty> {
    if requested.namespace.as_deref() != Some("DAV:") {
        return None;
    }
    BASE_LIVE_PROPERTIES
        .into_iter()
        .chain(
            DavExtensionPackage::ALL
                .into_iter()
                .flat_map(|package| package.descriptor().live_properties.iter().copied()),
        )
        .find(|property| property.local_name() == requested.name)
}

fn is_base_property(property: DavLiveProperty) -> bool {
    BASE_LIVE_PROPERTIES.contains(&property)
}

fn contains_expanded_name(elements: &[DavXmlElement], requested: &DavRequestedProperty) -> bool {
    elements
        .iter()
        .any(|element| element.name == requested.name && element.namespace == requested.namespace)
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
) -> Result<DavResponse, DavMultiStatusError> {
    property_multistatus_response_with_limits(items, DavMultiStatusLimits::default())
}

/// Builds a bounded 207 XML response with product-configured Multi-Status limits.
pub fn property_multistatus_response_with_limits(
    items: Vec<DavMultiStatusItem>,
    limits: DavMultiStatusLimits,
) -> Result<DavResponse, DavMultiStatusError> {
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
