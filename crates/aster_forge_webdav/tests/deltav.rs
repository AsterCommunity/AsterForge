use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use aster_forge_webdav::{
    DavAutoVersion, DavBackendError, DavBackendErrorKind, DavCancellationToken,
    DavCapabilityDeclaration, DavCapabilitySnapshot, DavExpandPropertyError,
    DavExpandPropertyProvider, DavExpandPropertyValue, DavExtensionPackage, DavExtensionSet,
    DavMethod, DavMethodSet, DavMultiStatusError, DavMultiStatusErrorKind, DavMultiStatusLimits,
    DavMultiStatusProgress, DavReportErrorResponsePolicy, DavReportLimits, DavReportPlanError,
    DavReportRequest, DavReportType, DavRequestedProperty, DavResourceState, DavResponse,
    DavResponseBody, DavVersionControlAction, DavVersionControlPlan, DavVersionControlPlanError,
    DavVersionControlPort, DavVersionControlResult, DavVersionProperty, DavVersionReportItem,
    DavVersionTreeRequest, DavVersioningCapabilities, DavVersioningMethodPlan,
    DavVersioningPrecondition, DavVersioningState, DavXmlElement, DavXmlError, DavXmlNode, Depth,
    dav_property_text_element, execute_expand_property, execute_version_control,
    expand_property_error_response, plan_capabilities, plan_report_request,
    plan_report_request_with_limits, plan_version_control_request, plan_versioning_method,
    report_plan_error_response, version_control_plan_error_response, version_control_response,
    version_tree_response, version_tree_response_with_limits, versioning_precondition_response,
};
use aster_forge_xml::BorrowedDocument;
use async_trait::async_trait;
use futures::executor::block_on;
use http::StatusCode;

fn body_text(response: &DavResponse) -> String {
    let DavResponseBody::Bytes(body) = &response.body else {
        panic!("response should contain bytes");
    };
    String::from_utf8(body.to_vec()).expect("UTF-8 response")
}

fn dav_property(name: &str) -> DavRequestedProperty {
    DavRequestedProperty {
        name: name.to_owned(),
        namespace: Some("DAV:".to_owned()),
        prefix: Some("D".to_owned()),
    }
}

fn versioning_snapshot(
    resource: DavResourceState,
    state: DavVersioningState,
    methods: &[DavMethod],
) -> DavCapabilitySnapshot {
    let mut declaration =
        DavCapabilityDeclaration::new(resource, DavMethodSet::from_methods(methods));
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::VersionControl]);
    if methods.contains(&DavMethod::Lock) && methods.contains(&DavMethod::Unlock) {
        declaration.locking = aster_forge_webdav::DavLockingCapability::Class2;
    }
    declaration.versioning = DavVersioningCapabilities {
        state,
        ..DavVersioningCapabilities::default()
    };
    plan_capabilities(declaration).expect("versioning snapshot")
}

fn report_snapshot(state: DavVersioningState) -> DavCapabilitySnapshot {
    versioning_snapshot(
        DavResourceState::File,
        state,
        &[DavMethod::Options, DavMethod::Report],
    )
}

fn bounded_report_limits() -> DavReportLimits {
    DavReportLimits {
        maximum_input_bytes: 4096,
        maximum_xml_depth: 8,
        maximum_selection_depth: 4,
        maximum_selection_properties: 8,
        maximum_expansion_depth: 4,
        maximum_expanded_resources: 8,
        maximum_expanded_properties: 8,
        multistatus: DavMultiStatusLimits::default(),
    }
}

struct ReportResponsePolicy;

impl DavReportErrorResponsePolicy for ReportResponsePolicy {
    fn unknown_type(&self, namespace: Option<&str>, name: &str) -> DavResponse {
        DavResponse::bytes(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown {namespace:?} {name}"),
        )
    }

    fn not_available(&self, report: DavReportType) -> DavResponse {
        DavResponse::bytes(
            StatusCode::CONFLICT,
            format!("unavailable {}", report.local_name()),
        )
    }
}

#[test]
fn version_tree_request_preserves_explicit_properties_qnames_and_depth() {
    let snapshot = report_snapshot(DavVersioningState::CheckedIn);
    let request = plan_report_request_with_limits(
        &snapshot,
        br#"<D:version-tree xmlns:D="DAV:" xmlns:Z="urn:test"><D:prop><Z:custom/><D:version-name/></D:prop></D:version-tree>"#,
        Some(Depth::Infinity),
        DavReportLimits::default(),
    )
    .expect("typed version-tree request");
    let DavReportRequest::VersionTree(request) = request else {
        panic!("expected version-tree request");
    };
    assert_eq!(request.depth, Depth::Infinity);
    assert_eq!(request.properties.as_ref().map(Vec::len), Some(2));
    let custom = &request.properties.expect("explicit properties")[0];
    assert_eq!(custom.name, "custom");
    assert_eq!(custom.namespace.as_deref(), Some("urn:test"));
    assert_eq!(custom.prefix.as_deref(), Some("Z"));

    let default = plan_report_request(&snapshot, br#"<D:version-tree xmlns:D="DAV:"/>"#)
        .expect("default request");
    let DavReportRequest::VersionTree(default) = default else {
        panic!("expected version-tree request");
    };
    assert_eq!(default.depth, Depth::Zero);
    assert!(default.properties.is_none());
}

#[test]
fn expand_property_request_preserves_nested_namespace_selections() {
    let snapshot = report_snapshot(DavVersioningState::CheckedIn);
    let request = plan_report_request(
        &snapshot,
        br#"<D:expand-property xmlns:D="DAV:">
              <D:property name="version-history">
                <D:property name="version-set">
                  <D:property name="creator-displayname"/>
                  <D:property name="custom" namespace="urn:test"/>
                </D:property>
              </D:property>
            </D:expand-property>"#,
    )
    .expect("expand request");
    let DavReportRequest::ExpandProperty(request) = request else {
        panic!("expected expand-property");
    };
    assert_eq!(request.depth, Depth::Zero);
    assert_eq!(request.properties[0].property.name, "version-history");
    assert_eq!(request.properties[0].nested[0].property.name, "version-set");
    assert_eq!(
        request.properties[0].nested[0].nested[1]
            .property
            .namespace
            .as_deref(),
        Some("urn:test")
    );
}

#[test]
fn empty_expand_property_request_produces_an_empty_success_propstat() {
    let request = expand_request(br#"<D:expand-property xmlns:D="DAV:"/>"#);
    let response = block_on(execute_expand_property(
        &ExpandProvider::default(),
        "/resource",
        &request,
        DavReportLimits::default(),
        &DavCancellationToken::new(),
    ))
    .expect("empty expand response");
    let xml = body_text(&response);
    assert!(xml.contains("/resource"), "{xml}");
    assert!(xml.contains("HTTP/1.1 200 OK"), "{xml}");
}

#[test]
fn report_grammar_and_snapshot_availability_fail_closed() {
    let versionable = report_snapshot(DavVersioningState::Versionable);
    assert_eq!(
        plan_report_request(&versionable, br#"<D:version-tree xmlns:D="DAV:"/>"#,),
        Err(DavReportPlanError::NotAvailable {
            report: DavReportType::VersionTree,
        })
    );
    assert!(matches!(
        plan_report_request(&versionable, br#"<D:expand-property xmlns:D="DAV:"/>"#,),
        Ok(DavReportRequest::ExpandProperty(_))
    ));

    let controlled = report_snapshot(DavVersioningState::CheckedIn);
    for body in [
        br#"<D:version-tree xmlns:D="DAV:"><D:prop/><D:prop/></D:version-tree>"#.as_slice(),
        br#"<D:version-tree xmlns:D="DAV:">text</D:version-tree>"#,
        br#"<D:expand-property xmlns:D="DAV:"><D:property/></D:expand-property>"#,
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="bad:name"/></D:expand-property>"#,
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="value" namespace="urn:bad namespace"/></D:expand-property>"#,
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="value" namespace="urn:bad&lt;namespace"/></D:expand-property>"#,
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="value" namespace="urn:bad&quot;namespace"/></D:expand-property>"#,
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="value" namespace="urn:bad&#x7f;namespace"/></D:expand-property>"#,
        br#"<D:expand-property xmlns:D="DAV:"><X:property xmlns:X="urn:x" name="ignored"/></D:expand-property>"#,
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="a"><X:property xmlns:X="urn:x" name="ignored"/></D:property></D:expand-property>"#,
    ] {
        assert_eq!(
            plan_report_request(&controlled, body),
            Err(DavReportPlanError::Xml(DavXmlError::InvalidGrammar))
        );
    }
    assert_eq!(
        plan_report_request(&controlled, br#"<X:version-tree xmlns:X="urn:extension"/>"#,),
        Err(DavReportPlanError::UnknownType {
            namespace: Some("urn:extension".to_owned()),
            name: "version-tree".to_owned(),
        })
    );
    assert_eq!(
        plan_report_request(
            &controlled,
            br#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///TARGET">]><D:version-tree xmlns:D="DAV:">&e;</D:version-tree>"#,
        ),
        Err(DavReportPlanError::Xml(DavXmlError::ExternalEntity))
    );
}

#[test]
fn report_limits_reject_each_zero_field_and_bound_input_bytes() {
    let snapshot = report_snapshot(DavVersioningState::CheckedIn);
    let valid = bounded_report_limits();
    for limits in [
        DavReportLimits {
            maximum_input_bytes: 0,
            ..valid
        },
        DavReportLimits {
            maximum_xml_depth: 0,
            ..valid
        },
        DavReportLimits {
            maximum_selection_depth: 0,
            ..valid
        },
        DavReportLimits {
            maximum_selection_properties: 0,
            ..valid
        },
        DavReportLimits {
            maximum_expansion_depth: 0,
            ..valid
        },
        DavReportLimits {
            maximum_expanded_resources: 0,
            ..valid
        },
        DavReportLimits {
            maximum_expanded_properties: 0,
            ..valid
        },
    ] {
        assert_eq!(
            plan_report_request_with_limits(
                &snapshot,
                br#"<D:version-tree xmlns:D="DAV:"/>"#,
                None,
                limits,
            ),
            Err(DavReportPlanError::InvalidLimits)
        );
    }

    let limits = DavReportLimits {
        maximum_input_bytes: 16,
        ..valid
    };
    assert_eq!(
        plan_report_request_with_limits(
            &snapshot,
            br#"<D:version-tree xmlns:D="DAV:"/>"#,
            None,
            limits,
        ),
        Err(DavReportPlanError::Xml(DavXmlError::TooLarge))
    );
}

#[test]
fn report_limits_cover_exact_xml_depth_and_one_over() {
    let snapshot = report_snapshot(DavVersioningState::CheckedIn);
    let valid = bounded_report_limits();
    let two_xml_levels =
        br#"<D:version-tree xmlns:D="DAV:" xmlns:X="urn:x"><X:future/></D:version-tree>"#;
    assert!(
        plan_report_request_with_limits(
            &snapshot,
            two_xml_levels,
            None,
            DavReportLimits {
                maximum_xml_depth: 2,
                ..valid
            },
        )
        .is_ok()
    );
    assert_eq!(
        plan_report_request_with_limits(
            &snapshot,
            two_xml_levels,
            None,
            DavReportLimits {
                maximum_xml_depth: 1,
                ..valid
            },
        ),
        Err(DavReportPlanError::Xml(DavXmlError::TooDeep))
    );
}

#[test]
fn report_limits_cover_selection_depth_and_property_count_boundaries() {
    let snapshot = report_snapshot(DavVersioningState::CheckedIn);
    let valid = bounded_report_limits();
    let three_selection_levels = br#"<D:expand-property xmlns:D="DAV:"><D:property name="a"><D:property name="b"><D:property name="c"/></D:property></D:property></D:expand-property>"#;
    assert!(
        plan_report_request_with_limits(
            &snapshot,
            three_selection_levels,
            None,
            DavReportLimits {
                maximum_selection_depth: 3,
                ..valid
            },
        )
        .is_ok()
    );
    assert_eq!(
        plan_report_request_with_limits(
            &snapshot,
            three_selection_levels,
            None,
            DavReportLimits {
                maximum_selection_depth: 2,
                ..valid
            },
        ),
        Err(DavReportPlanError::Xml(DavXmlError::TooDeep))
    );

    let two_properties = br#"<D:expand-property xmlns:D="DAV:"><D:property name="a"/><D:property name="b"/></D:expand-property>"#;
    let exact = DavReportLimits {
        maximum_selection_properties: 2,
        ..valid
    };
    assert!(plan_report_request_with_limits(&snapshot, two_properties, None, exact).is_ok());
    let one_too_few = DavReportLimits {
        maximum_selection_properties: 1,
        ..valid
    };
    assert_eq!(
        plan_report_request_with_limits(&snapshot, two_properties, None, one_too_few),
        Err(DavReportPlanError::Xml(DavXmlError::TooLarge))
    );
}

#[test]
fn report_errors_map_xml_unknown_and_unavailable_categories() {
    let invalid_limits =
        report_plan_error_response(&DavReportPlanError::InvalidLimits, &ReportResponsePolicy)
            .expect("invalid limits response");
    assert_eq!(invalid_limits.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        invalid_limits.headers.get("Cache-Control").unwrap(),
        "no-store"
    );
    assert_eq!(body_text(&invalid_limits), "");

    let invalid = report_plan_error_response(
        &DavReportPlanError::Xml(DavXmlError::Malformed),
        &ReportResponsePolicy,
    )
    .expect("invalid response");
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.headers.get("Cache-Control").unwrap(), "no-store");

    let unsupported = report_plan_error_response(
        &DavReportPlanError::UnknownType {
            namespace: Some("urn:extension".to_owned()),
            name: "future".to_owned(),
        },
        &ReportResponsePolicy,
    )
    .expect("unknown response");
    assert_eq!(unsupported.status, StatusCode::UNPROCESSABLE_ENTITY);

    let unavailable = report_plan_error_response(
        &DavReportPlanError::NotAvailable {
            report: DavReportType::VersionTree,
        },
        &ReportResponsePolicy,
    )
    .expect("unavailable response");
    assert_eq!(unavailable.status, StatusCode::CONFLICT);

    let external = report_plan_error_response(
        &DavReportPlanError::Xml(DavXmlError::ExternalEntity),
        &ReportResponsePolicy,
    )
    .expect("external entity response");
    assert_eq!(external.status, StatusCode::FORBIDDEN);
    assert!(body_text(&external).contains("no-external-entities"));
}

#[test]
fn version_control_plans_versionable_and_already_controlled_targets() {
    let versionable = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::Versionable,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    assert_eq!(
        plan_version_control_request(&versionable, b"")
            .expect("versionable plan")
            .action,
        DavVersionControlAction::PutUnderVersionControl
    );
    let controlled = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::CheckedOut,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    assert_eq!(
        plan_version_control_request(
            &controlled,
            br#"<D:version-control xmlns:D="DAV:"><X:future xmlns:X="urn:x"/></D:version-control>"#,
        )
        .expect("already controlled plan")
        .action,
        DavVersionControlAction::AlreadyControlled
    );
    assert_eq!(
        plan_version_control_request(&controlled, br#"<X:version-control xmlns:X="urn:x"/>"#),
        Err(DavVersionControlPlanError::Xml(DavXmlError::InvalidGrammar))
    );
    assert_eq!(
        plan_version_control_request(
            &controlled,
            br#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///TARGET">]><D:version-control xmlns:D="DAV:">&e;</D:version-control>"#,
        ),
        Err(DavVersionControlPlanError::Xml(DavXmlError::ExternalEntity))
    );

    let checked_in = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::CheckedIn,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    assert_eq!(
        plan_version_control_request(&checked_in, b"")
            .expect("checked-in plan")
            .action,
        DavVersionControlAction::AlreadyControlled
    );

    let immutable = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::Version,
        &[DavMethod::Options],
    );
    assert_eq!(
        plan_version_control_request(&immutable, b""),
        Err(DavVersionControlPlanError::MethodNotAllowed)
    );

    let unsupported = plan_capabilities(DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options]),
    ))
    .expect("unsupported snapshot");
    assert_eq!(
        plan_version_control_request(&unsupported, b""),
        Err(DavVersionControlPlanError::MethodNotAllowed)
    );
}

#[test]
fn version_control_response_is_empty_without_extensions_and_structured_with_them() {
    let empty = version_control_response(DavVersionControlResult {
        response_extensions: Vec::new(),
    })
    .expect("empty success");
    assert_eq!(empty.status, StatusCode::OK);
    assert!(matches!(empty.body, DavResponseBody::Empty));

    let mut extension = DavXmlElement::new("X:created-version");
    extension.namespace = Some("urn:test".to_owned());
    extension
        .namespaces
        .insert("X".to_owned(), "urn:test".to_owned());
    extension
        .children
        .push(DavXmlNode::Text("/versions/1".to_owned()));
    let response = version_control_response(DavVersionControlResult {
        response_extensions: vec![extension],
    })
    .expect("extension success");
    let xml = body_text(&response);
    assert!(xml.contains("version-control-response"), "{xml}");
    assert!(xml.contains("created-version"), "{xml}");
}

#[derive(Debug)]
struct RecordingVersionControlPort {
    seen: Mutex<Vec<DavVersionControlPlan>>,
    result: Result<DavVersionControlResult, DavBackendError>,
}

#[async_trait]
impl DavVersionControlPort for RecordingVersionControlPort {
    async fn version_control(
        &self,
        plan: DavVersionControlPlan,
    ) -> Result<DavVersionControlResult, DavBackendError> {
        self.seen.lock().expect("recording lock").push(plan);
        self.result.clone()
    }
}

#[test]
fn version_control_executor_passes_the_plan_once_and_preserves_backend_classification() {
    let plan = DavVersionControlPlan {
        action: DavVersionControlAction::PutUnderVersionControl,
    };
    let port = RecordingVersionControlPort {
        seen: Mutex::new(Vec::new()),
        result: Ok(DavVersionControlResult {
            response_extensions: Vec::new(),
        }),
    };
    let result = block_on(execute_version_control(&port, plan)).expect("transaction result");
    assert!(result.response_extensions.is_empty());
    assert_eq!(*port.seen.lock().expect("recorded plan"), vec![plan]);

    let failing = RecordingVersionControlPort {
        seen: Mutex::new(Vec::new()),
        result: Err(DavBackendError::new(DavBackendErrorKind::Conflict)),
    };
    assert_eq!(
        block_on(execute_version_control(&failing, plan)),
        Err(DavBackendError::new(DavBackendErrorKind::Conflict))
    );
    assert_eq!(*failing.seen.lock().expect("recorded plan"), vec![plan]);
}

#[test]
fn checked_in_mutations_cover_every_auto_version_mode_and_lock_state() {
    let methods = [
        DavMethod::Options,
        DavMethod::Put,
        DavMethod::Proppatch,
        DavMethod::Delete,
        DavMethod::Copy,
        DavMethod::Move,
        DavMethod::Lock,
        DavMethod::Unlock,
        DavMethod::Report,
    ];
    let checked_in = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::CheckedIn,
        &methods,
    );
    assert_eq!(
        plan_versioning_method(&checked_in, DavMethod::Put),
        Err(DavVersioningPrecondition::CannotModifyVersionControlledContent)
    );
    assert_eq!(
        plan_versioning_method(&checked_in, DavMethod::Proppatch),
        Err(DavVersioningPrecondition::CannotModifyVersionControlledProperty)
    );

    let mut declaration = checked_in.declaration().clone();
    declaration.versioning.auto_version = DavAutoVersion::CheckoutCheckin;
    let auto = plan_capabilities(declaration).expect("auto-version snapshot");
    assert_eq!(
        plan_versioning_method(&auto, DavMethod::Put),
        Ok(DavVersioningMethodPlan::AutoCheckoutCheckin)
    );

    for (auto_version, write_locked, expected) in [
        (
            DavAutoVersion::CheckoutUnlockedCheckin,
            false,
            DavVersioningMethodPlan::AutoCheckoutCheckin,
        ),
        (
            DavAutoVersion::CheckoutUnlockedCheckin,
            true,
            DavVersioningMethodPlan::AutoCheckout,
        ),
        (
            DavAutoVersion::Checkout,
            false,
            DavVersioningMethodPlan::AutoCheckout,
        ),
        (
            DavAutoVersion::LockedCheckout,
            true,
            DavVersioningMethodPlan::AutoCheckout,
        ),
    ] {
        let mut declaration = checked_in.declaration().clone();
        declaration.versioning.auto_version = auto_version;
        declaration.versioning.write_locked = write_locked;
        let snapshot = plan_capabilities(declaration).expect("auto-version snapshot");
        assert_eq!(
            plan_versioning_method(&snapshot, DavMethod::Put),
            Ok(expected)
        );
    }

    let mut locked_checkout = checked_in.declaration().clone();
    locked_checkout.versioning.auto_version = DavAutoVersion::LockedCheckout;
    let locked_checkout = plan_capabilities(locked_checkout).expect("unlocked target snapshot");
    assert_eq!(
        plan_versioning_method(&locked_checkout, DavMethod::Put),
        Err(DavVersioningPrecondition::CannotModifyVersionControlledContent)
    );
    assert_eq!(
        plan_versioning_method(&locked_checkout, DavMethod::Proppatch),
        Err(DavVersioningPrecondition::CannotModifyVersionControlledProperty)
    );
}

#[test]
fn checked_out_mutations_and_unlock_use_explicit_automatic_checkout_lock_facts() {
    let methods = [
        DavMethod::Options,
        DavMethod::Put,
        DavMethod::Lock,
        DavMethod::Unlock,
    ];
    let checked_out = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::CheckedOut,
        &methods,
    );
    assert_eq!(
        plan_versioning_method(&checked_out, DavMethod::Put),
        Ok(DavVersioningMethodPlan::DirectMutation)
    );
    let mut auto_checkout_lock = checked_out.declaration().clone();
    auto_checkout_lock.versioning.auto_version = DavAutoVersion::CheckoutUnlockedCheckin;
    auto_checkout_lock.versioning.write_locked = true;
    auto_checkout_lock.versioning.auto_checkout_lock = true;
    let auto_checkout_lock =
        plan_capabilities(auto_checkout_lock).expect("automatic checkout lock snapshot");
    assert_eq!(
        plan_versioning_method(&auto_checkout_lock, DavMethod::Unlock),
        Ok(DavVersioningMethodPlan::AutoCheckinOnUnlock)
    );
}

#[test]
fn immutable_version_mutations_cover_modify_rename_copy_and_delete_policy() {
    let methods = [
        DavMethod::Options,
        DavMethod::Put,
        DavMethod::Proppatch,
        DavMethod::Delete,
        DavMethod::Copy,
        DavMethod::Move,
        DavMethod::Report,
    ];
    let version = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::Version,
        &methods,
    );
    assert_eq!(
        plan_versioning_method(&version, DavMethod::Put),
        Err(DavVersioningPrecondition::CannotModifyVersion)
    );
    assert_eq!(
        plan_versioning_method(&version, DavMethod::Proppatch),
        Err(DavVersioningPrecondition::CannotModifyVersion)
    );
    assert_eq!(
        plan_versioning_method(&version, DavMethod::Move),
        Err(DavVersioningPrecondition::CannotRenameVersion)
    );
    assert_eq!(
        plan_versioning_method(&version, DavMethod::Delete),
        Err(DavVersioningPrecondition::NoVersionDelete)
    );
    assert_eq!(
        plan_versioning_method(&version, DavMethod::Copy),
        Ok(DavVersioningMethodPlan::DirectMutation)
    );
    let mut delete_allowed = version.declaration().clone();
    delete_allowed.versioning.allow_version_delete = true;
    let delete_allowed = plan_capabilities(delete_allowed).expect("version DELETE snapshot");
    assert_eq!(
        plan_versioning_method(&delete_allowed, DavMethod::Delete),
        Ok(DavVersioningMethodPlan::DeleteVersion)
    );
}

#[test]
fn version_control_target_kinds_and_precondition_responses_are_typed() {
    for resource in [DavResourceState::Collection, DavResourceState::Unmapped] {
        let snapshot = versioning_snapshot(
            resource,
            DavVersioningState::Versionable,
            &[DavMethod::Options, DavMethod::VersionControl],
        );
        assert_eq!(
            plan_versioning_method(&snapshot, DavMethod::VersionControl),
            Ok(DavVersioningMethodPlan::VersionControl(
                DavVersionControlAction::PutUnderVersionControl
            ))
        );
    }

    let response_snapshot = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::Version,
        &[
            DavMethod::Options,
            DavMethod::Put,
            DavMethod::Proppatch,
            DavMethod::Delete,
            DavMethod::Move,
        ],
    );
    for error in [
        DavVersioningPrecondition::CannotModifyVersionControlledContent,
        DavVersioningPrecondition::CannotModifyVersionControlledProperty,
        DavVersioningPrecondition::CannotModifyVersion,
        DavVersioningPrecondition::CannotRenameVersion,
        DavVersioningPrecondition::NoVersionDelete,
    ] {
        let response = versioning_precondition_response(&response_snapshot, error)
            .expect("DAV error response");
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert!(body_text(&response).contains("D:error"));
    }
}

#[test]
fn version_tree_response_groups_success_missing_unknown_and_backend_errors() {
    let request = DavVersionTreeRequest {
        properties: Some(vec![
            dav_property("version-name"),
            dav_property("predecessor-set"),
            dav_property("comment"),
            DavRequestedProperty {
                name: "custom".to_owned(),
                namespace: Some("urn:test".to_owned()),
                prefix: Some("Z".to_owned()),
            },
        ]),
        depth: Depth::Zero,
    };
    let response = version_tree_response(
        &request,
        vec![DavVersionReportItem {
            href: "/versions/1".to_owned(),
            properties: vec![
                DavVersionProperty::text(dav_property("version-name"), "V1"),
                DavVersionProperty::hrefs(
                    dav_property("predecessor-set"),
                    ["/versions/0".to_owned()],
                ),
                DavVersionProperty::backend_error(
                    dav_property("comment"),
                    DavBackendErrorKind::Internal,
                ),
            ],
        }],
    )
    .expect("version-tree response");
    let xml = body_text(&response);
    assert!(xml.contains("HTTP/1.1 200 OK"), "{xml}");
    assert!(xml.contains("HTTP/1.1 404 Not Found"), "{xml}");
    assert!(xml.contains("HTTP/1.1 500 Internal Server Error"), "{xml}");
    assert!(xml.contains("/versions/0"), "{xml}");
    assert!(xml.contains("Z:custom"), "{xml}");
}

#[test]
fn version_tree_response_escapes_text_and_hrefs_and_rejects_invalid_xml_characters() {
    let request = DavVersionTreeRequest {
        properties: Some(vec![
            dav_property("comment"),
            dav_property("predecessor-set"),
        ]),
        depth: Depth::Zero,
    };
    let response = version_tree_response(
        &request,
        vec![DavVersionReportItem {
            href: "/versions/a&b".to_owned(),
            properties: vec![
                DavVersionProperty::text(dav_property("comment"), "<note>&\"'"),
                DavVersionProperty::hrefs(
                    dav_property("predecessor-set"),
                    ["/versions/0?left=1&right=2".to_owned()],
                ),
            ],
        }],
    )
    .expect("escaped version-tree response");
    let xml = body_text(&response);
    assert!(xml.contains("/versions/a&amp;b"), "{xml}");
    assert!(xml.contains("&lt;note&gt;&amp;&quot;&apos;"), "{xml}");
    assert!(xml.contains("/versions/0?left=1&amp;right=2"), "{xml}");
    assert!(!xml.contains("<note>"), "{xml}");
    BorrowedDocument::parse(xml.as_bytes()).expect("response must remain well-formed XML");

    let result = version_tree_response(
        &request,
        vec![DavVersionReportItem {
            href: "/versions/invalid".to_owned(),
            properties: vec![DavVersionProperty::text(
                dav_property("comment"),
                "invalid\u{1}text",
            )],
        }],
    );
    let Err(error) = result else {
        panic!("XML 1.0 forbidden control characters must be rejected");
    };
    assert_eq!(error.kind, DavMultiStatusErrorKind::Xml);
}

#[test]
fn version_tree_response_covers_empty_default_href_sets_and_output_boundaries() {
    let empty_selection = DavVersionTreeRequest {
        properties: Some(Vec::new()),
        depth: Depth::Zero,
    };
    let response = version_tree_response(
        &empty_selection,
        vec![DavVersionReportItem {
            href: "/versions/empty".to_owned(),
            properties: Vec::new(),
        }],
    )
    .expect("empty requested set");
    let xml = body_text(&response);
    assert!(xml.contains("/versions/empty"), "{xml}");
    assert!(xml.contains("<D:prop></D:prop>"), "{xml}");
    assert!(xml.contains("HTTP/1.1 200 OK"), "{xml}");

    let default_request = DavVersionTreeRequest {
        properties: None,
        depth: Depth::Zero,
    };
    let version = DavVersionReportItem {
        href: "/versions/1".to_owned(),
        properties: vec![
            DavVersionProperty::text(dav_property("version-name"), "V1"),
            DavVersionProperty::hrefs(dav_property("predecessor-set"), ["/versions/0".to_owned()]),
            DavVersionProperty::hrefs(
                dav_property("successor-set"),
                ["/versions/2".to_owned(), "/versions/branch".to_owned()],
            ),
            DavVersionProperty::hrefs(dav_property("checkout-set"), ["/working/file".to_owned()]),
        ],
    };
    let response = version_tree_response(&default_request, vec![version.clone()])
        .expect("default property response");
    let xml = body_text(&response);
    for expected in [
        "V1",
        "/versions/0",
        "/versions/2",
        "/versions/branch",
        "/working/file",
    ] {
        assert!(xml.contains(expected), "missing {expected}: {xml}");
    }

    let empty_history =
        version_tree_response(&default_request, Vec::new()).expect("empty version history");
    assert_eq!(empty_history.status, StatusCode::MULTI_STATUS);
    assert!(!body_text(&empty_history).contains("<D:response>"));

    let too_small = DavMultiStatusLimits::new(32, 8, 32, 8);
    let Err(error) = version_tree_response_with_limits(&default_request, vec![version], too_small)
    else {
        panic!("expected output byte limit");
    };
    assert_eq!(error.kind, DavMultiStatusErrorKind::OutputLimitExceeded);
}

#[derive(Default)]
struct ExpandProvider {
    values: HashMap<(String, String), Result<Option<DavExpandPropertyValue>, DavBackendError>>,
}

struct CancellingExpandProvider {
    cancellation: DavCancellationToken,
    calls: AtomicUsize,
}

#[async_trait]
impl DavExpandPropertyProvider for CancellingExpandProvider {
    async fn property(
        &self,
        _href: &str,
        _property: &DavRequestedProperty,
    ) -> Result<Option<DavExpandPropertyValue>, DavBackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.cancellation.cancel();
        Ok(None)
    }
}

impl ExpandProvider {
    fn set(
        &mut self,
        href: &str,
        property: &str,
        value: Result<Option<DavExpandPropertyValue>, DavBackendError>,
    ) {
        self.values
            .insert((href.to_owned(), property.to_owned()), value);
    }
}

#[async_trait]
impl DavExpandPropertyProvider for ExpandProvider {
    async fn property(
        &self,
        href: &str,
        property: &DavRequestedProperty,
    ) -> Result<Option<DavExpandPropertyValue>, DavBackendError> {
        self.values
            .get(&(href.to_owned(), property.name.clone()))
            .cloned()
            .unwrap_or(Ok(None))
    }
}

fn expand_request(body: &[u8]) -> aster_forge_webdav::DavExpandPropertyRequest {
    let snapshot = report_snapshot(DavVersioningState::CheckedIn);
    let DavReportRequest::ExpandProperty(request) =
        plan_report_request(&snapshot, body).expect("expand request")
    else {
        panic!("expected expand request");
    };
    request
}

#[test]
fn expand_property_executes_nested_success_and_missing_href() {
    let request = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="version-set"><D:property name="version-name"/></D:property></D:expand-property>"#,
    );
    let mut provider = ExpandProvider::default();
    provider.set(
        "/history/1",
        "version-set",
        Ok(Some(DavExpandPropertyValue::Hrefs(vec![
            "/versions/1".to_owned(),
            "/versions/missing".to_owned(),
        ]))),
    );
    provider.set(
        "/versions/1",
        "version-name",
        Ok(Some(DavExpandPropertyValue::Element(
            DavVersionProperty::text(dav_property("version-name"), "V1")
                .result
                .into_value(),
        ))),
    );
    provider.set(
        "/versions/missing",
        "version-name",
        Err(DavBackendError::new(DavBackendErrorKind::NotFound)),
    );
    let response = block_on(execute_expand_property(
        &provider,
        "/history/1",
        &request,
        DavReportLimits::default(),
        &DavCancellationToken::new(),
    ))
    .expect("expanded response");
    let xml = body_text(&response);
    assert!(xml.contains("/versions/1"), "{xml}");
    assert!(xml.contains("V1"), "{xml}");
    assert!(xml.contains("HTTP/1.1 404 Not Found"), "{xml}");
}

trait VersionPropertyResultExt {
    fn into_value(self) -> DavXmlElement;
}

impl VersionPropertyResultExt for aster_forge_webdav::DavVersionPropertyResult {
    fn into_value(self) -> DavXmlElement {
        let aster_forge_webdav::DavVersionPropertyResult::Value(value) = self else {
            panic!("expected value");
        };
        value
    }
}

#[test]
fn expand_property_enforces_cycle_and_resource_limits() {
    let request = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="next"><D:property name="next"/></D:property></D:expand-property>"#,
    );
    let mut cyclic = ExpandProvider::default();
    cyclic.set(
        "/a",
        "next",
        Ok(Some(DavExpandPropertyValue::Hrefs(vec!["/a".to_owned()]))),
    );
    assert!(matches!(
        block_on(execute_expand_property(
            &cyclic,
            "/a",
            &request,
            DavReportLimits::default(),
            &DavCancellationToken::new(),
        )),
        Err(DavExpandPropertyError::Cycle { href }) if href == "/a"
    ));

    let limited = DavReportLimits {
        maximum_expanded_resources: 1,
        ..DavReportLimits::default()
    };
    assert!(matches!(
        block_on(execute_expand_property(
            &cyclic,
            "/a",
            &request,
            limited,
            &DavCancellationToken::new(),
        )),
        Err(DavExpandPropertyError::ResourceLimitExceeded)
    ));

    let mut two_resources = ExpandProvider::default();
    two_resources.set(
        "/a",
        "next",
        Ok(Some(DavExpandPropertyValue::Hrefs(vec!["/b".to_owned()]))),
    );
    let exact_resources = DavReportLimits {
        maximum_expanded_resources: 2,
        ..DavReportLimits::default()
    };
    block_on(execute_expand_property(
        &two_resources,
        "/a",
        &request,
        exact_resources,
        &DavCancellationToken::new(),
    ))
    .expect("exact resource limit");
}

#[test]
fn expand_property_checks_cancellation_before_and_between_backend_lookups() {
    let request = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="next"><D:property name="next"/></D:property></D:expand-property>"#,
    );
    let cyclic = ExpandProvider::default();
    let cancelled = DavCancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        block_on(execute_expand_property(
            &cyclic,
            "/a",
            &request,
            DavReportLimits::default(),
            &cancelled,
        )),
        Err(DavExpandPropertyError::Cancelled)
    ));

    let cancellation = DavCancellationToken::new();
    let cancelling = CancellingExpandProvider {
        cancellation: cancellation.clone(),
        calls: AtomicUsize::new(0),
    };
    let two = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="one"/><D:property name="two"/></D:expand-property>"#,
    );
    assert!(matches!(
        block_on(execute_expand_property(
            &cancelling,
            "/a",
            &two,
            DavReportLimits::default(),
            &cancellation,
        )),
        Err(DavExpandPropertyError::Cancelled)
    ));
    assert_eq!(cancelling.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn expand_property_preserves_backend_failure_classification() {
    let flat = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="value"/></D:expand-property>"#,
    );
    let mut failing = ExpandProvider::default();
    failing.set(
        "/a",
        "value",
        Err(DavBackendError::new(DavBackendErrorKind::Internal)),
    );
    assert!(matches!(
        block_on(execute_expand_property(
            &failing,
            "/a",
            &flat,
            DavReportLimits::default(),
            &DavCancellationToken::new(),
        )),
        Err(DavExpandPropertyError::Backend(error))
            if error == DavBackendError::new(DavBackendErrorKind::Internal)
    ));
}

#[test]
fn expand_property_enforces_depth_and_property_count_limits() {
    let nested = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="next"><D:property name="next"><D:property name="value"/></D:property></D:property></D:expand-property>"#,
    );
    let mut provider = ExpandProvider::default();
    provider.set(
        "/a",
        "next",
        Ok(Some(DavExpandPropertyValue::Hrefs(vec!["/b".to_owned()]))),
    );
    provider.set(
        "/b",
        "next",
        Ok(Some(DavExpandPropertyValue::Hrefs(vec!["/c".to_owned()]))),
    );
    provider.set(
        "/c",
        "value",
        Ok(Some(DavExpandPropertyValue::Element(
            dav_property_text_element(&dav_property("value"), "done"),
        ))),
    );
    let depth_limited = DavReportLimits {
        maximum_expansion_depth: 1,
        ..DavReportLimits::default()
    };
    assert!(matches!(
        block_on(execute_expand_property(
            &provider,
            "/a",
            &nested,
            depth_limited,
            &DavCancellationToken::new(),
        )),
        Err(DavExpandPropertyError::DepthLimitExceeded)
    ));

    let flat = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="one"/><D:property name="two"/></D:expand-property>"#,
    );
    let exact = DavReportLimits {
        maximum_expanded_properties: 2,
        ..DavReportLimits::default()
    };
    block_on(execute_expand_property(
        &ExpandProvider::default(),
        "/a",
        &flat,
        exact,
        &DavCancellationToken::new(),
    ))
    .expect("exact property limit");
    let one_too_few = DavReportLimits {
        maximum_expanded_properties: 1,
        ..DavReportLimits::default()
    };
    assert!(matches!(
        block_on(execute_expand_property(
            &ExpandProvider::default(),
            "/a",
            &flat,
            one_too_few,
            &DavCancellationToken::new(),
        )),
        Err(DavExpandPropertyError::PropertyLimitExceeded)
    ));
}

#[test]
fn expand_property_reports_non_href_nested_values_and_bounds_output_bytes() {
    let mut scalar = ExpandProvider::default();
    scalar.set(
        "/a",
        "next",
        Ok(Some(DavExpandPropertyValue::Element(
            dav_property_text_element(&dav_property("next"), "not-an-href-set"),
        ))),
    );
    scalar.set(
        "/a",
        "sibling",
        Ok(Some(DavExpandPropertyValue::Element(
            dav_property_text_element(&dav_property("sibling"), "still-returned"),
        ))),
    );
    let invalid_nested = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="next"><D:property name="value"/></D:property><D:property name="sibling"/></D:expand-property>"#,
    );
    let response = block_on(execute_expand_property(
        &scalar,
        "/a",
        &invalid_nested,
        DavReportLimits::default(),
        &DavCancellationToken::new(),
    ))
    .expect("unexpandable property remains property-scoped");
    let xml = body_text(&response);
    assert!(xml.contains("HTTP/1.1 403 Forbidden"), "{xml}");
    assert!(xml.contains("HTTP/1.1 200 OK"), "{xml}");
    assert!(xml.contains("still-returned"), "{xml}");

    let value = expand_request(
        br#"<D:expand-property xmlns:D="DAV:"><D:property name="value"/></D:expand-property>"#,
    );
    let mut large = ExpandProvider::default();
    large.set(
        "/a",
        "value",
        Ok(Some(DavExpandPropertyValue::Element(
            dav_property_text_element(&dav_property("value"), "x".repeat(256)),
        ))),
    );
    let output_limited = DavReportLimits {
        multistatus: DavMultiStatusLimits::new(64, 8, 8, 8),
        ..DavReportLimits::default()
    };
    assert!(matches!(
        block_on(execute_expand_property(
            &large,
            "/a",
            &value,
            output_limited,
            &DavCancellationToken::new(),
        )),
        Err(DavExpandPropertyError::MultiStatus(error))
            if error.kind == DavMultiStatusErrorKind::OutputLimitExceeded
    ));
}

#[test]
fn version_control_plan_errors_render_protocol_status_and_condition() {
    let snapshot = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::Versionable,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    let response = version_control_plan_error_response(
        &snapshot,
        DavVersionControlPlanError::Xml(DavXmlError::ExternalEntity),
    )
    .expect("external entity response");
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(body_text(&response).contains("no-external-entities"));

    let response = versioning_precondition_response(
        &snapshot,
        DavVersioningPrecondition::CannotModifyVersionControlledContent,
    )
    .expect("precondition response");
    assert!(body_text(&response).contains("cannot-modify-version-controlled-content"));

    let disallowed = versioning_snapshot(
        DavResourceState::File,
        DavVersioningState::Versionable,
        &[DavMethod::Options],
    );
    let response = version_control_plan_error_response(
        &disallowed,
        DavVersionControlPlanError::MethodNotAllowed,
    )
    .expect("405 response");
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers.get("Allow").expect("Allow"), "OPTIONS");
    let response =
        versioning_precondition_response(&disallowed, DavVersioningPrecondition::MethodNotAllowed)
            .expect("versioning 405 response");
    assert_eq!(response.headers.get("Allow").expect("Allow"), "OPTIONS");
}

#[test]
fn expand_property_failures_map_before_response_start() {
    for (error, expected) in [
        (
            DavExpandPropertyError::Cycle {
                href: "/a".to_owned(),
            },
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavExpandPropertyError::DepthLimitExceeded,
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavExpandPropertyError::ResourceLimitExceeded,
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavExpandPropertyError::PropertyLimitExceeded,
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavExpandPropertyError::Cancelled,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            DavExpandPropertyError::InvalidLimits,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let response = expand_property_error_response(&error).expect("failure response");
        assert_eq!(response.status, expected);
        assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
    }

    let backend = expand_property_error_response(&DavExpandPropertyError::Backend(
        DavBackendError::new(DavBackendErrorKind::Forbidden),
    ))
    .expect("backend response");
    assert_eq!(backend.status, StatusCode::FORBIDDEN);

    for (kind, expected) in [
        (
            DavMultiStatusErrorKind::InvalidLimits,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DavMultiStatusErrorKind::ItemLimitExceeded,
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavMultiStatusErrorKind::PropertyLimitExceeded,
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavMultiStatusErrorKind::InvalidItem,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DavMultiStatusErrorKind::OutputLimitExceeded,
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavMultiStatusErrorKind::Cancelled,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            DavMultiStatusErrorKind::Xml,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DavMultiStatusErrorKind::Write,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let error = DavExpandPropertyError::MultiStatus(DavMultiStatusError {
            kind,
            progress: DavMultiStatusProgress::default(),
        });
        let response = expand_property_error_response(&error).expect("Multi-Status response");
        assert_eq!(response.status, expected);
        assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
    }

    let multistatus_backend = DavExpandPropertyError::MultiStatus(DavMultiStatusError {
        kind: DavMultiStatusErrorKind::Backend(DavBackendError::new(DavBackendErrorKind::Locked)),
        progress: DavMultiStatusProgress::default(),
    });
    let response = expand_property_error_response(&multistatus_backend)
        .expect("Multi-Status backend response");
    assert_eq!(response.status, StatusCode::LOCKED);
}
