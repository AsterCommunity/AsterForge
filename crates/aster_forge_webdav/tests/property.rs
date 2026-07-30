use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavCapabilityDeclaration, DavCapabilitySnapshot,
    DavCurrentPrincipal, DavExtensionPackage, DavExtensionSet, DavLiveProperty,
    DavLivePropertyError, DavLivePropertyEvaluationError, DavLivePropertyMetadata,
    DavLivePropertyProvider, DavLivePropertyRequirements, DavLivePropertyValueSnapshot,
    DavLockingCapability, DavMethod, DavMethodSet, DavProp, DavPropfindRequest, DavQuotaSnapshot,
    DavReportType, DavRequestedProperty, DavResourceState, DavResponseBody, DavSearchCapabilities,
    DavSearchGrammar, DavXmlElement, DavXmlError, DavXmlNode, build_live_propfind_item,
    build_live_propfind_item_with_provider, build_proppatch_item, dav_property_name_element,
    dav_property_text_element, format_creation_date, is_protected_live_property,
    live_property_requirements, plan_atomic_proppatch, plan_capabilities,
    property_multistatus_response, propfind_finite_depth_response, propfind_request_label,
    propfind_xml_error_response, proppatch_xml_error_response,
};
use http::StatusCode;

static LIVE_SEARCH_GRAMMARS: [DavSearchGrammar; 2] = [
    DavSearchGrammar::BASICSEARCH,
    DavSearchGrammar::new(
        "https://example.test/custom-query",
        "https://example.test/search/xml",
        "custom-query",
    ),
];

fn property(name: &str, namespace: Option<&str>, prefix: Option<&str>) -> DavRequestedProperty {
    DavRequestedProperty {
        name: name.to_owned(),
        namespace: namespace.map(str::to_owned),
        prefix: prefix.map(str::to_owned),
    }
}

fn dav_property(name: &str) -> DavRequestedProperty {
    property(name, Some("DAV:"), Some("D"))
}

fn live_snapshot() -> DavCapabilitySnapshot {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::Collection,
        DavMethodSet::from_methods(&[
            DavMethod::Options,
            DavMethod::Propfind,
            DavMethod::Report,
            DavMethod::Search,
        ]),
    );
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(&[
        DavExtensionPackage::VersionControl,
        DavExtensionPackage::Search,
        DavExtensionPackage::Quota,
        DavExtensionPackage::CollectionSync,
        DavExtensionPackage::CurrentPrincipal,
        DavExtensionPackage::AddMember,
        DavExtensionPackage::Prefer,
    ]);
    declaration.search = DavSearchCapabilities {
        grammars: &LIVE_SEARCH_GRAMMARS,
    };
    plan_capabilities(declaration).expect("live-property capability snapshot")
}

#[derive(Debug, Clone)]
struct Values {
    display_name: String,
    content_type: String,
    etag: String,
    quota: Option<DavQuotaSnapshot>,
    current_principal: Option<String>,
    unauthenticated: bool,
    sync_token: Option<String>,
    add_member: Option<String>,
    extension_values: Vec<(DavLiveProperty, DavXmlElement)>,
    dead: Vec<DavProp>,
}

impl Default for Values {
    fn default() -> Self {
        Self {
            display_name: "documents".to_owned(),
            content_type: "httpd/unix-directory".to_owned(),
            etag: "\"collection-v1\"".to_owned(),
            quota: Some(DavQuotaSnapshot {
                used_bytes: 400,
                available_bytes: Some(600),
            }),
            current_principal: Some("https://dav.example/principals/users/alice".to_owned()),
            unauthenticated: false,
            sync_token: Some("https://dav.example/sync/42".to_owned()),
            add_member: Some("/documents;add-member/".to_owned()),
            extension_values: vec![(
                DavLiveProperty::Comment,
                dav_property_text_element(&dav_property("comment"), "project files"),
            )],
            dead: vec![DavProp {
                name: "color".to_owned(),
                prefix: Some("x".to_owned()),
                namespace: Some("urn:test".to_owned()),
                xml: Some(br#"<x:color xmlns:x="urn:test">blue</x:color>"#.to_vec()),
            }],
        }
    }
}

impl DavLivePropertyValueSnapshot for Values {
    fn metadata(&self) -> DavLivePropertyMetadata<'_> {
        DavLivePropertyMetadata {
            creation_date: Some(UNIX_EPOCH + Duration::from_secs(784_111_777)),
            display_name: Some(&self.display_name),
            content_language: None,
            content_length: None,
            content_type: Some(&self.content_type),
            etag: Some(&self.etag),
            last_modified: Some(UNIX_EPOCH + Duration::from_secs(784_111_777)),
        }
    }

    fn dead_properties(&self) -> &[DavProp] {
        &self.dead
    }

    fn quota(&self) -> Option<DavQuotaSnapshot> {
        self.quota
    }

    fn current_principal(&self) -> Option<DavCurrentPrincipal<'_>> {
        if self.unauthenticated {
            Some(DavCurrentPrincipal::Unauthenticated)
        } else {
            self.current_principal
                .as_deref()
                .map(DavCurrentPrincipal::Href)
        }
    }

    fn sync_token(&self) -> Option<&str> {
        self.sync_token.as_deref()
    }

    fn add_member_href(&self) -> Option<&str> {
        self.add_member.as_deref()
    }

    fn extension_value(&self, property: DavLiveProperty) -> Option<&DavXmlElement> {
        self.extension_values
            .iter()
            .find_map(|(candidate, value)| (*candidate == property).then_some(value))
    }
}

struct DefaultValues;

impl DavLivePropertyValueSnapshot for DefaultValues {}

fn snapshot_for(
    resource: DavResourceState,
    packages: &[DavExtensionPackage],
) -> DavCapabilitySnapshot {
    let mut declaration = DavCapabilityDeclaration::new(
        resource,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Propfind]),
    );
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(packages);
    if declaration.extensions.contains(DavExtensionPackage::Search) {
        declaration.search = DavSearchCapabilities {
            grammars: &[DavSearchGrammar::BASICSEARCH],
        };
    }
    plan_capabilities(declaration).expect("property snapshot")
}

fn property_names(item: &aster_forge_webdav::DavMultiStatusItem, status: u16) -> Vec<&str> {
    item.propstats
        .iter()
        .find(|group| group.status == status)
        .map(|group| {
            group
                .properties
                .iter()
                .map(|property| property.name.as_str())
                .collect()
        })
        .unwrap_or_default()
}

fn property_element<'a>(
    item: &'a aster_forge_webdav::DavMultiStatusItem,
    status: u16,
    name: &str,
) -> &'a DavXmlElement {
    item.propstats
        .iter()
        .find(|group| group.status == status)
        .and_then(|group| {
            group
                .properties
                .iter()
                .find(|property| property.name == name)
        })
        .expect("property element")
}

#[test]
fn creation_date_uses_rfc3339_utc_format() {
    assert_eq!(
        format_creation_date(UNIX_EPOCH + Duration::from_secs(784_111_777)),
        "1994-11-06T08:49:37+00:00"
    );
}

#[test]
fn requirements_select_value_groups_without_per_property_backend_calls() {
    let snapshot = live_snapshot();
    assert_eq!(
        live_property_requirements(
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("quota-used-bytes")]),
        ),
        DavLivePropertyRequirements {
            quota: true,
            ..DavLivePropertyRequirements::default()
        }
    );
    assert_eq!(
        live_property_requirements(
            &snapshot,
            &DavPropfindRequest::Prop(vec![property("color", Some("urn:test"), Some("x"))]),
        ),
        DavLivePropertyRequirements {
            dead_properties: true,
            ..DavLivePropertyRequirements::default()
        }
    );
    let allprop =
        live_property_requirements(&snapshot, &DavPropfindRequest::AllProp { include: vec![] });
    assert!(allprop.metadata);
    assert!(allprop.dead_properties);
    assert!(allprop.current_principal);
    assert!(!allprop.extension_values);
    assert!(!allprop.quota, "quota is excluded from default allprop");

    let allprop_include = live_property_requirements(
        &snapshot,
        &DavPropfindRequest::AllProp {
            include: vec![
                dav_property("quota-used-bytes"),
                dav_property("sync-token"),
                dav_property("add-member"),
                dav_property("comment"),
            ],
        },
    );
    assert!(allprop_include.quota);
    assert!(allprop_include.sync_token);
    assert!(allprop_include.add_member);
    assert!(allprop_include.extension_values);

    assert_eq!(
        live_property_requirements(&snapshot, &DavPropfindRequest::PropName),
        DavLivePropertyRequirements {
            metadata: true,
            locks: true,
            dead_properties: true,
            quota: true,
            current_principal: true,
            sync_token: true,
            add_member: true,
            extension_values: true,
        }
    );

    assert_eq!(
        live_property_requirements(
            &snapshot,
            &DavPropfindRequest::Prop(vec![
                dav_property("displayname"),
                dav_property("supportedlock"),
                dav_property("quota-used-bytes"),
                dav_property("current-user-principal"),
                dav_property("sync-token"),
                dav_property("add-member"),
                dav_property("comment"),
                property("unknown", Some("urn:test"), Some("x")),
            ]),
        ),
        DavLivePropertyRequirements {
            metadata: true,
            locks: true,
            dead_properties: true,
            quota: true,
            current_principal: true,
            sync_token: true,
            add_member: true,
            extension_values: true,
        }
    );
}

#[test]
fn value_snapshot_defaults_are_empty() {
    let values = DefaultValues;
    let metadata = values.metadata();
    assert!(metadata.creation_date.is_none());
    assert!(metadata.display_name.is_none());
    assert!(metadata.content_language.is_none());
    assert!(metadata.content_length.is_none());
    assert!(metadata.content_type.is_none());
    assert!(metadata.etag.is_none());
    assert!(metadata.last_modified.is_none());
    assert!(values.active_locks().is_empty());
    assert!(values.dead_properties().is_empty());
    assert!(values.quota().is_none());
    assert!(values.current_principal().is_none());
    assert!(values.sync_token().is_none());
    assert!(values.add_member_href().is_none());
    assert!(values.extension_value(DavLiveProperty::Acl).is_none());
}

#[test]
fn propfind_request_labels_cover_all_selectors() {
    assert_eq!(
        propfind_request_label(&DavPropfindRequest::AllProp { include: vec![] }),
        "allprop"
    );
    assert_eq!(
        propfind_request_label(&DavPropfindRequest::PropName),
        "propname"
    );
    assert_eq!(
        propfind_request_label(&DavPropfindRequest::Prop(vec![])),
        "prop"
    );
}

#[test]
fn disabled_standard_extension_name_can_still_resolve_as_a_dead_property() {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Propfind]),
    );
    declaration.compliance.class1 = true;
    let snapshot = plan_capabilities(declaration).expect("class 1 snapshot");
    let request = DavPropfindRequest::Prop(vec![dav_property("quota-used-bytes")]);
    assert_eq!(
        live_property_requirements(&snapshot, &request),
        DavLivePropertyRequirements {
            dead_properties: true,
            ..DavLivePropertyRequirements::default()
        }
    );
    let values = Values {
        dead: vec![DavProp {
            name: "quota-used-bytes".to_owned(),
            prefix: Some("D".to_owned()),
            namespace: Some("DAV:".to_owned()),
            xml: Some(
                br#"<D:quota-used-bytes xmlns:D="DAV:">custom</D:quota-used-bytes>"#.to_vec(),
            ),
        }],
        ..Values::default()
    };
    let item = build_live_propfind_item("/file".to_owned(), &snapshot, &request, &values)
        .expect("dead property with disabled standard name");
    assert_eq!(property_names(&item, 200), ["quota-used-bytes"]);
    assert!(property_names(&item, 404).is_empty());
}

#[test]
fn allprop_excludes_expensive_rfc_properties_and_include_restores_them() {
    let snapshot = live_snapshot();
    let values = Values::default();
    let allprop = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::AllProp { include: vec![] },
        &values,
    )
    .expect("allprop item");
    let names = property_names(&allprop, 200);
    assert!(names.contains(&"displayname"));
    assert!(names.contains(&"resourcetype"));
    assert!(names.contains(&"current-user-principal"));
    assert!(names.contains(&"color"));
    for excluded in [
        "quota-used-bytes",
        "quota-available-bytes",
        "sync-token",
        "add-member",
        "supported-method-set",
        "supported-live-property-set",
        "supported-report-set",
        "supported-query-grammar-set",
        "comment",
        "getcontentlanguage",
    ] {
        assert!(
            !names.contains(&excluded),
            "unexpected allprop property {excluded}"
        );
    }

    let included = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::AllProp {
            include: vec![
                dav_property("displayname"),
                dav_property("quota-used-bytes"),
                dav_property("sync-token"),
                dav_property("comment"),
                dav_property("getcontentlanguage"),
            ],
        },
        &values,
    )
    .expect("allprop include item");
    let ok = property_names(&included, 200);
    assert!(ok.contains(&"quota-used-bytes"));
    assert!(ok.contains(&"sync-token"));
    assert!(ok.contains(&"comment"));
    assert_eq!(ok.iter().filter(|name| **name == "displayname").count(), 1);
    assert_eq!(property_names(&included, 404), ["getcontentlanguage"]);

    let invalid_principal = Values {
        current_principal: Some("not a principal URL".to_owned()),
        ..Values::default()
    };
    assert_eq!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::AllProp { include: vec![] },
            &invalid_principal,
        ),
        Err(DavLivePropertyError::InvalidRepresentation {
            property: DavLiveProperty::CurrentUserPrincipal,
        })
    );
}

#[test]
fn propname_uses_dynamic_quota_and_metadata_definition_rules() {
    let snapshot = live_snapshot();
    let values = Values {
        quota: Some(DavQuotaSnapshot {
            used_bytes: 400,
            available_bytes: None,
        }),
        ..Values::default()
    };
    let item = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::PropName,
        &values,
    )
    .expect("propname item");
    let names = property_names(&item, 200);
    assert!(names.contains(&"quota-used-bytes"));
    assert!(!names.contains(&"quota-available-bytes"));
    assert!(!names.contains(&"getcontentlanguage"));
    assert!(names.contains(&"sync-token"));
    assert!(names.contains(&"current-user-principal"));
    assert!(names.contains(&"add-member"));
    assert!(names.contains(&"comment"));
    assert!(
        !names.contains(&"checked-in"),
        "enabled packages do not make an undefined property exist"
    );
    assert!(names.contains(&"color"));
}

#[test]
fn explicit_properties_group_success_and_missing_with_requested_qnames() {
    let snapshot = live_snapshot();
    let mut extension = dav_property_text_element(&dav_property("comment"), "project files");
    extension
        .namespaces
        .insert("stale".to_owned(), "urn:stale".to_owned());
    extension
        .namespaces
        .insert("keep".to_owned(), "urn:attribute".to_owned());
    extension
        .namespaces
        .insert("child".to_owned(), "urn:child".to_owned());
    extension
        .namespaces
        .insert("meta".to_owned(), "urn:metadata".to_owned());
    extension
        .attributes
        .insert("keep:flag".to_owned(), "yes".to_owned());
    let mut child = DavXmlElement::new("child:value");
    child
        .attributes
        .insert("meta:kind".to_owned(), "nested".to_owned());
    extension.children.push(DavXmlNode::Element(child));
    let values = Values {
        quota: Some(DavQuotaSnapshot {
            used_bytes: 400,
            available_bytes: None,
        }),
        extension_values: vec![(DavLiveProperty::Comment, extension)],
        ..Values::default()
    };
    let item = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![
            property("displayname", Some("DAV:"), Some("client")),
            property("comment", Some("DAV:"), Some("client")),
            dav_property("quota-used-bytes"),
            dav_property("quota-available-bytes"),
            dav_property("getcontentlanguage"),
            property("color", Some("urn:test"), Some("custom")),
            property("missing", Some("urn:test"), Some("custom")),
        ]),
        &values,
    )
    .expect("explicit property item");
    assert_eq!(
        property_names(&item, 200),
        ["displayname", "comment", "quota-used-bytes", "color"]
    );
    assert_eq!(
        property_names(&item, 404),
        ["quota-available-bytes", "getcontentlanguage", "missing"]
    );
    let client_xml = String::from_utf8(
        property_element(&item, 200, "comment")
            .to_bytes()
            .expect("client-prefixed property XML"),
    )
    .expect("UTF-8 client property XML");
    assert!(client_xml.contains("<client:comment"), "{client_xml}");
    assert!(client_xml.contains("xmlns:client=\"DAV:\""), "{client_xml}");
    assert!(!client_xml.contains("urn:stale"), "{client_xml}");
    assert!(
        client_xml.contains("xmlns:keep=\"urn:attribute\""),
        "{client_xml}"
    );
    assert!(client_xml.contains("<child:value"), "{client_xml}");
    assert!(client_xml.contains("meta:kind=\"nested\""), "{client_xml}");
    assert!(
        client_xml.contains("xmlns:child=\"urn:child\""),
        "{client_xml}"
    );
    assert!(
        client_xml.contains("xmlns:meta=\"urn:metadata\""),
        "{client_xml}"
    );
    let custom_xml = String::from_utf8(
        property_element(&item, 200, "color")
            .to_bytes()
            .expect("custom-prefixed property XML"),
    )
    .expect("UTF-8 custom property XML");
    assert!(custom_xml.contains("<custom:color"), "{custom_xml}");
    assert!(
        custom_xml.contains("xmlns:custom=\"urn:test\""),
        "{custom_xml}"
    );
    assert_eq!(
        property_element(&item, 200, "displayname")
            .prefix
            .as_deref(),
        Some("client")
    );
    assert_eq!(
        property_element(&item, 200, "color").prefix.as_deref(),
        Some("custom")
    );

    let default_namespace_item = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![property("comment", Some("DAV:"), None)]),
        &values,
    )
    .expect("default-namespace live property");
    let default_namespace_xml = String::from_utf8(
        property_element(&default_namespace_item, 200, "comment")
            .to_bytes()
            .expect("default-namespace property XML"),
    )
    .expect("UTF-8 default-namespace property XML");
    assert!(
        default_namespace_xml.contains("<comment"),
        "{default_namespace_xml}"
    );
    assert!(
        default_namespace_xml.contains("xmlns=\"DAV:\""),
        "{default_namespace_xml}"
    );
}

#[test]
fn discovery_properties_are_generated_from_the_same_snapshot() {
    let snapshot = live_snapshot();
    let item = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![
            dav_property("supported-method-set"),
            dav_property("supported-live-property-set"),
            dav_property("supported-report-set"),
            dav_property("supported-query-grammar-set"),
        ]),
        &Values::default(),
    )
    .expect("discovery item");

    let methods = property_element(&item, 200, "supported-method-set")
        .to_bytes()
        .expect("method XML");
    let methods = String::from_utf8(methods).expect("UTF-8 method XML");
    assert!(methods.contains("name=\"SEARCH\""), "{methods}");
    assert!(methods.contains("name=\"REPORT\""), "{methods}");
    assert!(methods.contains("name=\"VERSION-CONTROL\""), "{methods}");

    let live = String::from_utf8(
        property_element(&item, 200, "supported-live-property-set")
            .to_bytes()
            .expect("live XML"),
    )
    .expect("UTF-8 live XML");
    assert!(live.contains("quota-used-bytes"), "{live}");
    assert!(live.contains("sync-token"), "{live}");
    assert!(live.contains("add-member"), "{live}");

    let reports = String::from_utf8(
        property_element(&item, 200, "supported-report-set")
            .to_bytes()
            .expect("report XML"),
    )
    .expect("UTF-8 report XML");
    assert!(reports.contains("version-tree"), "{reports}");
    assert!(reports.contains("sync-collection"), "{reports}");

    let grammars = String::from_utf8(
        property_element(&item, 200, "supported-query-grammar-set")
            .to_bytes()
            .expect("grammar XML"),
    )
    .expect("UTF-8 grammar XML");
    assert!(grammars.contains("basicsearch"), "{grammars}");
    assert!(grammars.contains("custom-query"), "{grammars}");
    assert!(
        grammars.contains("xmlns=\"https://example.test/search/xml\""),
        "{grammars}"
    );
    assert!(!grammars.contains("custom-query</D:"), "{grammars}");
}

#[test]
fn report_discovery_covers_every_typed_report_once() {
    let snapshot = snapshot_for(
        DavResourceState::Collection,
        &[
            DavExtensionPackage::AccessControl,
            DavExtensionPackage::VersionControl,
            DavExtensionPackage::VersionHistory,
            DavExtensionPackage::Merge,
            DavExtensionPackage::Baseline,
            DavExtensionPackage::Activity,
            DavExtensionPackage::CollectionSync,
        ],
    );
    let item = build_live_propfind_item(
        "/reports/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![dav_property("supported-report-set")]),
        &DefaultValues,
    )
    .expect("complete report catalog");
    let property = property_element(&item, 200, "supported-report-set");
    let parsed = DavXmlElement::parse(&property.to_bytes().expect("report XML"))
        .expect("serialized report XML parses");
    let mut actual = parsed
        .child_elements()
        .map(|supported| {
            assert_eq!(supported.name, "supported-report");
            let mut report_wrappers = supported.child_elements();
            let report = report_wrappers.next().expect("report wrapper");
            assert!(
                report_wrappers.next().is_none(),
                "each supported-report must contain exactly one report wrapper"
            );
            assert_eq!(report.name, "report");
            let mut report_types = report.child_elements();
            let report_type = report_types.next().expect("typed report");
            assert!(
                report_types.next().is_none(),
                "each report wrapper must contain exactly one typed report"
            );
            report_type.name.clone()
        })
        .collect::<Vec<_>>();
    let mut expected = DavReportType::ALL
        .map(|report| report.local_name().to_owned())
        .to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

#[test]
fn resource_type_and_optional_standard_values_follow_resource_state() {
    for (resource, status, child) in [
        (DavResourceState::Unmapped, 404, None),
        (DavResourceState::File, 200, None),
        (DavResourceState::Collection, 200, Some("collection")),
        (DavResourceState::MountRoot, 200, Some("collection")),
        (DavResourceState::Principal, 200, Some("principal")),
        (
            DavResourceState::RedirectReference,
            200,
            Some("redirectref"),
        ),
        (DavResourceState::AddMemberEndpoint, 200, None),
    ] {
        let snapshot = snapshot_for(resource, &[]);
        let item = build_live_propfind_item(
            "/target".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("resourcetype")]),
            &DefaultValues,
        )
        .expect("resource type");
        assert_eq!(property_names(&item, status), ["resourcetype"]);
        if let Some(child) = child {
            let xml = String::from_utf8(
                property_element(&item, status, "resourcetype")
                    .to_bytes()
                    .expect("resource type XML"),
            )
            .expect("UTF-8 resource type XML");
            assert!(xml.contains(child), "resource {resource:?}: {xml}");
        }
    }

    let file_quota = snapshot_for(DavResourceState::File, &[DavExtensionPackage::Quota]);
    let values = Values {
        quota: None,
        ..Values::default()
    };
    let item = build_live_propfind_item(
        "/file".to_owned(),
        &file_quota,
        &DavPropfindRequest::Prop(vec![
            dav_property("quota-used-bytes"),
            dav_property("getlastmodified"),
        ]),
        &values,
    )
    .expect("optional file values");
    assert_eq!(property_names(&item, 404), ["quota-used-bytes"]);

    let empty = build_live_propfind_item(
        "/file".to_owned(),
        &snapshot_for(DavResourceState::File, &[]),
        &DavPropfindRequest::Prop(vec![dav_property("getlastmodified")]),
        &DefaultValues,
    )
    .expect("missing last-modified");
    assert_eq!(property_names(&empty, 404), ["getlastmodified"]);
}

#[test]
fn class_two_supported_lock_and_empty_namespace_search_grammar_are_rendered() {
    let mut class2 = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[
            DavMethod::Options,
            DavMethod::Propfind,
            DavMethod::Lock,
            DavMethod::Unlock,
        ]),
    );
    class2.compliance.class1 = true;
    class2.locking = DavLockingCapability::Class2;
    let class2 = plan_capabilities(class2).expect("class 2 snapshot");
    let supported_lock = build_live_propfind_item(
        "/file".to_owned(),
        &class2,
        &DavPropfindRequest::Prop(vec![dav_property("supportedlock")]),
        &DefaultValues,
    )
    .expect("supported lock");
    let xml = String::from_utf8(
        property_element(&supported_lock, 200, "supportedlock")
            .to_bytes()
            .expect("supported lock XML"),
    )
    .expect("UTF-8 supported lock XML");
    assert!(xml.contains("lockentry"), "{xml}");

    static GRAMMARS: [DavSearchGrammar; 2] = [
        DavSearchGrammar::BASICSEARCH,
        DavSearchGrammar::new("urn:no-namespace", "", "query"),
    ];
    let mut search = DavCapabilityDeclaration::new(
        DavResourceState::Collection,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Propfind]),
    );
    search.compliance.class1 = true;
    search.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::Search]);
    search.search = DavSearchCapabilities {
        grammars: &GRAMMARS,
    };
    let search = plan_capabilities(search).expect("SEARCH snapshot");
    let item = build_live_propfind_item(
        "/search/".to_owned(),
        &search,
        &DavPropfindRequest::Prop(vec![dav_property("supported-query-grammar-set")]),
        &DefaultValues,
    )
    .expect("query grammar discovery");
    let xml = String::from_utf8(
        property_element(&item, 200, "supported-query-grammar-set")
            .to_bytes()
            .expect("query grammar XML"),
    )
    .expect("UTF-8 query grammar XML");
    assert!(xml.contains("<query"), "{xml}");
    assert!(!xml.contains("xmlns=\"\""), "{xml}");
}

#[test]
fn propname_omits_an_empty_success_group() {
    let snapshot = plan_capabilities(DavCapabilityDeclaration::new(
        DavResourceState::Unmapped,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Propfind]),
    ))
    .expect("unmapped non-DAV snapshot");
    let item = build_live_propfind_item(
        "/missing".to_owned(),
        &snapshot,
        &DavPropfindRequest::PropName,
        &DefaultValues,
    )
    .expect("empty propname item");
    assert!(item.propstats.is_empty());
}

#[test]
fn metadata_is_read_once_per_propfind_item() {
    struct CountingValues {
        calls: AtomicUsize,
    }

    impl DavLivePropertyValueSnapshot for CountingValues {
        fn metadata(&self) -> DavLivePropertyMetadata<'_> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            DavLivePropertyMetadata {
                display_name: Some("documents"),
                content_type: Some("httpd/unix-directory"),
                ..DavLivePropertyMetadata::default()
            }
        }
    }

    let values = CountingValues {
        calls: AtomicUsize::new(0),
    };
    build_live_propfind_item(
        "/documents/".to_owned(),
        &live_snapshot(),
        &DavPropfindRequest::Prop(vec![
            dav_property("displayname"),
            dav_property("getcontenttype"),
        ]),
        &values,
    )
    .expect("metadata-backed properties");
    assert_eq!(values.calls.load(Ordering::Relaxed), 1);
}

struct RecordingProvider {
    calls: Mutex<Vec<DavLivePropertyRequirements>>,
    values: Values,
}

struct FailingProvider;

impl DavLivePropertyProvider for FailingProvider {
    type Values = DefaultValues;

    async fn live_property_values(
        &self,
        _path: &aster_forge_webdav::DavPath,
        _requirements: DavLivePropertyRequirements,
    ) -> Result<Self::Values, DavBackendError> {
        Err(DavBackendError::new(DavBackendErrorKind::Internal))
    }
}

impl DavLivePropertyProvider for RecordingProvider {
    type Values = Values;

    async fn live_property_values(
        &self,
        _path: &aster_forge_webdav::DavPath,
        requirements: DavLivePropertyRequirements,
    ) -> Result<Self::Values, DavBackendError> {
        self.calls.lock().expect("calls").push(requirements);
        Ok(self.values.clone())
    }
}

#[test]
fn canonical_provider_entrypoint_fetches_one_snapshot_per_resource() {
    let snapshot = live_snapshot();
    let provider = RecordingProvider {
        calls: Mutex::new(Vec::new()),
        values: Values::default(),
    };
    let path = aster_forge_webdav::DavPath::new("/documents/").expect("path");
    let item = futures::executor::block_on(build_live_propfind_item_with_provider(
        &provider,
        &path,
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![
            dav_property("quota-used-bytes"),
            dav_property("quota-available-bytes"),
        ]),
    ))
    .expect("provider item");
    assert_eq!(property_names(&item, 200).len(), 2);
    assert_eq!(
        provider.calls.lock().expect("calls").as_slice(),
        &[DavLivePropertyRequirements {
            quota: true,
            ..DavLivePropertyRequirements::default()
        }]
    );

    let error = futures::executor::block_on(build_live_propfind_item_with_provider(
        &FailingProvider,
        &path,
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::PropName,
    ))
    .expect_err("backend failure");
    assert_eq!(
        error,
        DavLivePropertyEvaluationError::Backend(DavBackendError::new(
            DavBackendErrorKind::Internal
        ))
    );
}

#[test]
fn sync_token_accepts_absolute_uris_and_rejects_missing_or_invalid_values() {
    let snapshot = live_snapshot();
    let urn_token = Values {
        sync_token: Some("urn:uuid:12345678-1234-1234-1234-123456789abc".to_owned()),
        ..Values::default()
    };
    assert!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("sync-token")]),
            &urn_token,
        )
        .is_ok(),
        "RFC 6578 sync tokens accept absolute non-HTTP URIs"
    );

    let missing = Values {
        sync_token: None,
        ..Values::default()
    };
    assert_eq!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("sync-token")]),
            &missing,
        ),
        Err(DavLivePropertyError::MissingRequiredValue {
            property: DavLiveProperty::SyncToken,
        })
    );

    let invalid = Values {
        sync_token: Some("bad token".to_owned()),
        ..Values::default()
    };
    assert_eq!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("sync-token")]),
            &invalid,
        ),
        Err(DavLivePropertyError::InvalidRepresentation {
            property: DavLiveProperty::SyncToken,
        })
    );
}

#[test]
fn current_principal_rejects_missing_or_invalid_urls() {
    let snapshot = live_snapshot();
    for value in [
        "/principals/users/alice",
        "https:principal.example/alice",
        " https://principal.example/alice",
    ] {
        let invalid_principal = Values {
            current_principal: Some(value.to_owned()),
            ..Values::default()
        };
        assert_eq!(
            build_live_propfind_item(
                "/documents/".to_owned(),
                &snapshot,
                &DavPropfindRequest::Prop(vec![dav_property("current-user-principal")]),
                &invalid_principal,
            ),
            Err(DavLivePropertyError::InvalidRepresentation {
                property: DavLiveProperty::CurrentUserPrincipal,
            }),
            "invalid RFC 5397 principal URL: {value}"
        );
    }

    let missing_principal = Values {
        current_principal: None,
        ..Values::default()
    };
    assert_eq!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("current-user-principal")]),
            &missing_principal,
        ),
        Err(DavLivePropertyError::MissingRequiredValue {
            property: DavLiveProperty::CurrentUserPrincipal,
        })
    );
}

#[test]
fn add_member_accepts_supported_uri_shapes_and_rejects_missing_or_invalid_values() {
    let snapshot = live_snapshot();
    for value in ["urn:example:add-member", "//other.example/post"] {
        let invalid_add_member = Values {
            add_member: Some(value.to_owned()),
            ..Values::default()
        };
        assert_eq!(
            build_live_propfind_item(
                "/documents/".to_owned(),
                &snapshot,
                &DavPropfindRequest::Prop(vec![dav_property("add-member")]),
                &invalid_add_member,
            ),
            Err(DavLivePropertyError::InvalidRepresentation {
                property: DavLiveProperty::AddMember,
            }),
            "invalid Add-Member URI: {value}"
        );
    }

    let missing_add_member = Values {
        add_member: None,
        ..Values::default()
    };
    assert_eq!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("add-member")]),
            &missing_add_member,
        ),
        Err(DavLivePropertyError::MissingRequiredValue {
            property: DavLiveProperty::AddMember,
        })
    );

    let absolute_add_member = Values {
        add_member: Some("https://dav.example/documents;add-member/".to_owned()),
        ..Values::default()
    };
    build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![dav_property("add-member")]),
        &absolute_add_member,
    )
    .expect("absolute HTTP(S) Add-Member URI shape");
}

#[test]
fn quota_used_bytes_requires_a_snapshot_for_supported_collections() {
    let snapshot = live_snapshot();
    let quota = Values {
        quota: None,
        ..Values::default()
    };
    assert_eq!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("quota-used-bytes")]),
            &quota,
        ),
        Err(DavLivePropertyError::MissingRequiredValue {
            property: DavLiveProperty::QuotaUsedBytes,
        })
    );
}

#[test]
fn current_principal_and_add_member_render_typed_href_values() {
    let snapshot = live_snapshot();
    let values = Values {
        unauthenticated: true,
        ..Values::default()
    };
    let item = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![
            dav_property("current-user-principal"),
            dav_property("add-member"),
        ]),
        &values,
    )
    .expect("principal and add-member");
    let principal = String::from_utf8(
        property_element(&item, 200, "current-user-principal")
            .to_bytes()
            .expect("principal XML"),
    )
    .expect("UTF-8 principal XML");
    assert!(principal.contains("unauthenticated"), "{principal}");
    let add_member = String::from_utf8(
        property_element(&item, 200, "add-member")
            .to_bytes()
            .expect("add-member XML"),
    )
    .expect("UTF-8 add-member XML");
    assert!(
        add_member.contains("/documents;add-member/"),
        "{add_member}"
    );
}

#[test]
fn allprop_prefers_live_values_over_duplicate_dead_properties() {
    let snapshot = live_snapshot();
    let values = Values {
        dead: vec![
            DavProp {
                name: "displayname".to_owned(),
                prefix: Some("D".to_owned()),
                namespace: Some("DAV:".to_owned()),
                xml: Some(br#"<D:displayname xmlns:D="DAV:">stale</D:displayname>"#.to_vec()),
            },
            DavProp {
                name: "color".to_owned(),
                prefix: Some("x".to_owned()),
                namespace: Some("urn:test".to_owned()),
                xml: Some(br#"<x:color xmlns:x="urn:test">blue</x:color>"#.to_vec()),
            },
        ],
        ..Values::default()
    };
    let item = build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::AllProp { include: vec![] },
        &values,
    )
    .expect("allprop item");
    assert_eq!(
        property_names(&item, 200)
            .iter()
            .filter(|name| **name == "displayname")
            .count(),
        1
    );
    let xml = String::from_utf8(
        property_element(&item, 200, "displayname")
            .to_bytes()
            .expect("displayname XML"),
    )
    .expect("UTF-8 displayname XML");
    assert!(xml.contains("documents"), "{xml}");
    assert!(!xml.contains("stale"), "{xml}");
}

#[test]
fn standard_live_properties_are_protected_but_unknown_dead_properties_are_not() {
    let snapshot = live_snapshot();
    assert!(is_protected_live_property(
        &snapshot,
        &dav_property("quota-used-bytes")
    ));
    assert!(is_protected_live_property(
        &snapshot,
        &dav_property("getetag")
    ));
    assert!(!is_protected_live_property(
        &snapshot,
        &property("color", Some("urn:test"), Some("x"))
    ));

    let mut acl = DavCapabilityDeclaration::new(
        DavResourceState::Principal,
        DavMethodSet::from_methods(&[DavMethod::Options]),
    );
    acl.compliance.class1 = true;
    acl.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::AccessControl]);
    let acl = plan_capabilities(acl).expect("ACL snapshot");
    for product_selectable in ["group-member-set", "owner", "group"] {
        assert!(
            !is_protected_live_property(&acl, &dav_property(product_selectable)),
            "RFC 3744 leaves {product_selectable} protection to the server"
        );
    }
}

#[test]
fn atomic_proppatch_and_response_helpers_preserve_existing_error_boundaries() {
    let success = plan_atomic_proppatch([false, false]);
    assert!(success.apply);
    assert_eq!(success.statuses, [StatusCode::OK, StatusCode::OK]);

    let rejected = plan_atomic_proppatch([false, true, false]);
    assert!(!rejected.apply);
    assert_eq!(
        rejected.statuses,
        [
            StatusCode::FAILED_DEPENDENCY,
            StatusCode::FORBIDDEN,
            StatusCode::FAILED_DEPENDENCY,
        ]
    );

    let item = build_proppatch_item(
        "/documents/".to_owned(),
        [(
            200,
            dav_property_name_element(&property("color", Some("urn:test"), Some("x"))),
        )],
    );
    let multistatus = property_multistatus_response(vec![item]).expect("multistatus");
    assert_eq!(multistatus.status, StatusCode::MULTI_STATUS);

    let finite = propfind_finite_depth_response().expect("finite depth response");
    assert_eq!(finite.status, StatusCode::FORBIDDEN);
    let DavResponseBody::Bytes(body) = finite.body else {
        panic!("finite depth should contain XML");
    };
    assert!(String::from_utf8_lossy(&body).contains("propfind-finite-depth"));

    let propfind = propfind_xml_error_response(DavXmlError::InvalidGrammar).expect("error");
    assert_eq!(propfind.status, StatusCode::BAD_REQUEST);
    let proppatch = proppatch_xml_error_response(DavXmlError::TooLarge).expect("error");
    assert_eq!(proppatch.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[test]
fn system_time_boundary_for_last_modified_is_reported_only_when_requested() {
    struct ExtremeTime;
    impl DavLivePropertyValueSnapshot for ExtremeTime {
        fn metadata(&self) -> DavLivePropertyMetadata<'_> {
            DavLivePropertyMetadata {
                display_name: Some("safe"),
                last_modified: Some(
                    SystemTime::UNIX_EPOCH
                        .checked_sub(Duration::from_secs(1))
                        .expect("one second before the Unix epoch is representable"),
                ),
                ..DavLivePropertyMetadata::default()
            }
        }
    }
    let snapshot = live_snapshot();
    build_live_propfind_item(
        "/documents/".to_owned(),
        &snapshot,
        &DavPropfindRequest::Prop(vec![dav_property("displayname")]),
        &ExtremeTime,
    )
    .expect("unrequested invalid date must not fail");
    assert_eq!(
        build_live_propfind_item(
            "/documents/".to_owned(),
            &snapshot,
            &DavPropfindRequest::Prop(vec![dav_property("getlastmodified")]),
            &ExtremeTime,
        ),
        Err(DavLivePropertyError::InvalidRepresentation {
            property: DavLiveProperty::GetLastModified,
        })
    );
}
