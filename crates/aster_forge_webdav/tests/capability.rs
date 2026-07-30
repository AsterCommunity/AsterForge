use std::marker::PhantomData;

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavCapabilityContext, DavCapabilityDeclaration,
    DavCapabilityEvaluationError, DavCapabilityPlanError, DavCapabilityProfile,
    DavCapabilityProvider, DavCapabilityTarget, DavCheckoutInPlaceSupport, DavClass1Profile,
    DavClass1Support, DavClass2And3Profile, DavClass2Support, DavClass3Profile, DavClass3Support,
    DavCollectionSyncExtension, DavCollectionSyncSupport, DavExtensionPackage, DavExtensionSet,
    DavLockingCapability, DavMethod, DavMethodGateError, DavMethodSet, DavNonDavProfile,
    DavOperation, DavPartialPutCapability, DavPartialPutSupport, DavPatchBodyPolicy,
    DavPatchCapability, DavPatchFormat, DavPatchSupport, DavPath, DavPreferExtension,
    DavPreferSupport, DavPreferenceSet, DavPrivateUpdateRangeCapability,
    DavPrivateUpdateRangeSupport, DavQuotaExtension, DavQuotaSupport, DavReportType,
    DavResourceState, DavResourceStateSet, DavSearchCapabilities, DavSearchExtension,
    DavSearchGrammar, DavSearchSupport, DavVersionControlExtension, DavVersionControlSupport,
    DavVersionHistorySupport, DavWithPartialPut, DavWithPatch, DavWithPrivateUpdateRange,
    DavWorkspaceExtension, DavWorkspaceSupport, DavWriteCapabilities, DavWritePrecondition,
    dav_capability_profile, gate_method, method_not_allowed_response, options_response,
    plan_capabilities, plan_capabilities_with_provider,
};
use http::StatusCode;
use http::header::{ALLOW, CACHE_CONTROL};

static SEARCH_GRAMMARS: [DavSearchGrammar; 2] = [
    DavSearchGrammar::BASICSEARCH,
    DavSearchGrammar::new(
        "https://example.test/query",
        "https://example.test/query/xml",
        "query",
    ),
];
static INVALID_SEARCH_CODED_URL: [DavSearchGrammar; 2] = [
    DavSearchGrammar::BASICSEARCH,
    DavSearchGrammar::new("bad grammar", "DAV:", "bad"),
];
static INVALID_SEARCH_CODED_URL_DELIMITER: [DavSearchGrammar; 2] = [
    DavSearchGrammar::BASICSEARCH,
    DavSearchGrammar::new("urn:valid,urn:other", "DAV:", "bad"),
];
static INVALID_SEARCH_NAMESPACE: [DavSearchGrammar; 2] = [
    DavSearchGrammar::BASICSEARCH,
    DavSearchGrammar::new("urn:valid", "bad namespace", "valid"),
];
static INVALID_SEARCH_LOCAL_NAME: [DavSearchGrammar; 2] = [
    DavSearchGrammar::BASICSEARCH,
    DavSearchGrammar::new("urn:valid", "urn:xml", "has:prefix"),
];
static DUPLICATE_SEARCH_CODED_URL: [DavSearchGrammar; 2] =
    [DavSearchGrammar::BASICSEARCH, DavSearchGrammar::BASICSEARCH];
static DUPLICATE_SEARCH_XML_NAME: [DavSearchGrammar; 2] = [
    DavSearchGrammar::BASICSEARCH,
    DavSearchGrammar::new("urn:other", "DAV:", "basicsearch"),
];

fn declaration(resource: DavResourceState, methods: &[DavMethod]) -> DavCapabilityDeclaration {
    let mut declaration =
        DavCapabilityDeclaration::new(resource, DavMethodSet::from_methods(methods));
    declaration.compliance.class1 = true;
    declaration
}

#[test]
fn method_set_covers_every_extension_without_duplicates_or_heap_backing() {
    let set = DavMethodSet::from_methods(&DavMethod::ALL);
    assert_eq!(set.iter().count(), 33);
    let operations = [
        DavOperation::Options,
        DavOperation::Get,
        DavOperation::Head,
        DavOperation::Post,
        DavOperation::Put,
        DavOperation::Patch,
        DavOperation::Delete,
        DavOperation::Mkcol,
        DavOperation::Copy,
        DavOperation::Move,
        DavOperation::Propfind,
        DavOperation::Proppatch,
        DavOperation::Lock,
        DavOperation::Unlock,
        DavOperation::Acl,
        DavOperation::Report,
        DavOperation::VersionControl,
        DavOperation::Checkout,
        DavOperation::Checkin,
        DavOperation::Uncheckout,
        DavOperation::Mkworkspace,
        DavOperation::Update,
        DavOperation::Label,
        DavOperation::Merge,
        DavOperation::BaselineControl,
        DavOperation::Mkactivity,
        DavOperation::Search,
        DavOperation::Orderpatch,
        DavOperation::Mkredirectref,
        DavOperation::Updateredirectref,
        DavOperation::Bind,
        DavOperation::Unbind,
        DavOperation::Rebind,
    ];
    for (method, operation) in DavMethod::ALL.into_iter().zip(operations) {
        assert!(set.contains(method));
        assert_eq!(DavMethod::from_name(method.as_str()), Some(method));
        assert_eq!(method.operation(), operation);
    }
    assert_eq!(
        DavMethodSet::from_methods(&[
            DavMethod::Search,
            DavMethod::Options,
            DavMethod::Search,
            DavMethod::Acl,
        ])
        .render(),
        "OPTIONS, ACL, SEARCH"
    );
    assert!(DavMethodSet::empty().is_empty());
    assert!(DavMethodSet::from_methods(&[DavMethod::Options]).is_subset_of(set));
    assert!(!set.is_subset_of(DavMethodSet::from_methods(&[DavMethod::Options])));
}

#[test]
fn compact_extension_and_resource_sets_cover_set_algebra_boundaries() {
    let versioning = DavExtensionSet::from_packages(&[DavExtensionPackage::VersionControl]);
    let quota = DavExtensionSet::from_packages(&[DavExtensionPackage::Quota]);
    let combined = versioning.union(quota);
    assert!(DavExtensionSet::empty().is_empty());
    assert!(combined.contains_all(versioning));
    assert!(quota.is_subset_of(combined));
    assert!(!combined.is_subset_of(quota));

    let resources = DavResourceStateSet::from_states(&[
        DavResourceState::Unmapped,
        DavResourceState::File,
        DavResourceState::Collection,
        DavResourceState::MountRoot,
        DavResourceState::Principal,
        DavResourceState::RedirectReference,
        DavResourceState::AddMemberEndpoint,
    ]);
    for resource in [
        DavResourceState::Unmapped,
        DavResourceState::File,
        DavResourceState::Collection,
        DavResourceState::MountRoot,
        DavResourceState::Principal,
        DavResourceState::RedirectReference,
        DavResourceState::AddMemberEndpoint,
    ] {
        assert!(resources.contains(resource));
    }
}

#[test]
fn search_grammar_constructor_preserves_the_independent_http_and_xml_names() {
    let grammar = DavSearchGrammar::new("urn:query", "urn:query:xml", "query");
    assert_eq!(grammar.coded_url, "urn:query");
    assert_eq!(grammar.xml_namespace, "urn:query:xml");
    assert_eq!(grammar.xml_local_name, "query");
}

#[test]
fn planner_preserves_all_target_kinds_and_normalizes_head_from_get() {
    for resource in [
        DavResourceState::Unmapped,
        DavResourceState::File,
        DavResourceState::Collection,
        DavResourceState::MountRoot,
        DavResourceState::Principal,
        DavResourceState::RedirectReference,
        DavResourceState::AddMemberEndpoint,
    ] {
        let snapshot =
            plan_capabilities(declaration(resource, &[DavMethod::Options, DavMethod::Get]))
                .expect("basic declaration");
        assert_eq!(snapshot.declaration().resource, resource);
        assert!(snapshot.allows(DavMethod::Head));
        assert_eq!(snapshot.allow_header(), "OPTIONS, GET, HEAD");
    }

    assert_eq!(
        plan_capabilities(declaration(DavResourceState::File, &[DavMethod::Get])),
        Err(DavCapabilityPlanError::OptionsMissing)
    );
    assert_eq!(
        plan_capabilities(declaration(
            DavResourceState::File,
            &[DavMethod::Options, DavMethod::Head],
        )),
        Err(DavCapabilityPlanError::HeadWithoutGet)
    );
}

#[test]
fn class_three_requires_only_class_one_and_remains_independent_from_locking() {
    let mut class3_without_class1 = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options]),
    );
    class3_without_class1.compliance.class3 = true;
    assert_eq!(
        plan_capabilities(class3_without_class1),
        Err(DavCapabilityPlanError::Class3WithoutClass1)
    );

    let mut class13 = declaration(DavResourceState::File, &[DavMethod::Options]);
    class13.compliance.class3 = true;
    assert_eq!(
        plan_capabilities(class13)
            .expect("DAV classes 1 and 3")
            .dav_header()
            .expect("DAV header"),
        "1, 3"
    );

    let mut class123 = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock],
    );
    class123.compliance.class3 = true;
    class123.locking = DavLockingCapability::Class2;
    assert_eq!(
        plan_capabilities(class123)
            .expect("DAV classes 1, 2 and 3")
            .dav_header()
            .expect("DAV header"),
        "1, 2, 3"
    );
}

#[test]
fn locking_still_requires_the_complete_class_two_surface() {
    let mut without_class1 = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock]),
    );
    without_class1.locking = DavLockingCapability::Class2;
    assert_eq!(
        plan_capabilities(without_class1),
        Err(DavCapabilityPlanError::Class2WithoutClass1)
    );

    let mut missing_unlock = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Lock],
    );
    missing_unlock.locking = DavLockingCapability::Class2;
    assert_eq!(
        plan_capabilities(missing_unlock),
        Err(DavCapabilityPlanError::Class2WithoutLockMethods)
    );

    assert_eq!(
        plan_capabilities(declaration(
            DavResourceState::File,
            &[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock],
        )),
        Err(DavCapabilityPlanError::LockMethodsWithoutClass2)
    );
}

#[test]
fn descriptor_catalog_has_exact_tokens_and_separate_deltav_packages() {
    let all = DavExtensionSet::from_packages(&DavExtensionPackage::ALL);
    assert_eq!(all.iter().count(), 22);
    let mut properties = Vec::new();
    let mut reports = Vec::new();
    for package in DavExtensionPackage::ALL {
        for property in package.descriptor().live_properties {
            assert!(
                !properties.contains(property),
                "duplicate live-property descriptor: {property:?}"
            );
            properties.push(*property);
        }
        for report in package.descriptor().reports {
            assert!(
                !reports.contains(report),
                "duplicate REPORT descriptor: {report:?}"
            );
            reports.push(*report);
        }
    }
    let tokens = all
        .iter()
        .filter_map(|package| package.descriptor().dav_token)
        .collect::<Vec<_>>();
    assert_eq!(
        tokens,
        [
            "access-control",
            "version-control",
            "checkout-in-place",
            "version-history",
            "workspace",
            "update",
            "label",
            "working-resource",
            "merge",
            "baseline",
            "activity",
            "version-controlled-collection",
            "extended-mkcol",
            "ordered-collections",
            "redirectrefs",
            "bind",
        ]
    );
    for package in [
        DavExtensionPackage::Search,
        DavExtensionPackage::Quota,
        DavExtensionPackage::CollectionSync,
        DavExtensionPackage::CurrentPrincipal,
        DavExtensionPackage::AddMember,
        DavExtensionPackage::Prefer,
    ] {
        assert_eq!(package.descriptor().dav_token, None);
    }
    assert!(
        DavExtensionPackage::Workspace
            .descriptor()
            .prerequisites
            .contains(DavExtensionPackage::CheckoutInPlace)
    );
    assert!(
        DavExtensionPackage::Workspace
            .descriptor()
            .prerequisites
            .contains(DavExtensionPackage::VersionHistory)
    );
}

#[test]
fn every_package_plans_on_an_applicable_resource_and_projects_its_catalog() {
    let resources = [
        DavResourceState::Unmapped,
        DavResourceState::File,
        DavResourceState::Collection,
        DavResourceState::MountRoot,
        DavResourceState::Principal,
        DavResourceState::RedirectReference,
        DavResourceState::AddMemberEndpoint,
    ];
    for package in DavExtensionPackage::ALL {
        let descriptor = package.descriptor();
        let resource = resources
            .into_iter()
            .find(|resource| descriptor.resources.contains(*resource))
            .expect("every package has an applicable resource");
        let mut declaration = declaration(resource, &[DavMethod::Options]);
        declaration.extensions = descriptor.prerequisites.with(package);
        if package == DavExtensionPackage::Search {
            declaration.search = DavSearchCapabilities {
                grammars: &[DavSearchGrammar::BASICSEARCH],
            };
        }
        let snapshot = plan_capabilities(declaration).unwrap_or_else(|error| {
            panic!("package {package:?} on {resource:?} should plan: {error}")
        });
        assert!(snapshot.supports_extension(package));
        for property in descriptor.live_properties {
            assert!(snapshot.supports_live_property(*property));
        }
        for report in descriptor.reports {
            assert!(snapshot.supports_report(*report));
        }
        for method in descriptor.methods {
            if method.resources.contains(resource) {
                assert!(snapshot.supports(method.method));
            }
        }
    }
}

#[test]
fn maximum_compatible_package_set_plans_for_every_resource_state() {
    for resource in [
        DavResourceState::Unmapped,
        DavResourceState::File,
        DavResourceState::Collection,
        DavResourceState::MountRoot,
        DavResourceState::Principal,
        DavResourceState::RedirectReference,
        DavResourceState::AddMemberEndpoint,
    ] {
        let packages = DavExtensionPackage::ALL
            .into_iter()
            .filter(|package| package.descriptor().resources.contains(resource))
            .fold(DavExtensionSet::empty(), DavExtensionSet::with);
        let mut declaration = declaration(resource, &[DavMethod::Options]);
        declaration.extensions = packages;
        if packages.contains(DavExtensionPackage::Search) {
            declaration.search = DavSearchCapabilities {
                grammars: &[DavSearchGrammar::BASICSEARCH],
            };
        }
        let snapshot = plan_capabilities(declaration).unwrap_or_else(|error| {
            panic!("maximum package set on {resource:?} should plan: {error}")
        });
        assert_eq!(snapshot.extensions(), packages);
        assert_eq!(
            snapshot.extensions().iter().count(),
            packages.iter().count()
        );
    }
}

#[test]
fn deltav_dependencies_and_extension_only_methods_are_validated() {
    let mut workspace = declaration(DavResourceState::Collection, &[DavMethod::Options]);
    workspace.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::Workspace]);
    assert_eq!(
        plan_capabilities(workspace),
        Err(DavCapabilityPlanError::ExtensionMissingPrerequisite {
            package: DavExtensionPackage::Workspace,
            required: DavExtensionPackage::VersionControl,
        })
    );

    let mut incomplete = declaration(DavResourceState::Collection, &[DavMethod::Options]);
    incomplete.extensions = DavExtensionSet::from_packages(&[
        DavExtensionPackage::VersionControl,
        DavExtensionPackage::Workspace,
    ]);
    assert_eq!(
        plan_capabilities(incomplete),
        Err(DavCapabilityPlanError::ExtensionMissingPrerequisite {
            package: DavExtensionPackage::Workspace,
            required: DavExtensionPackage::CheckoutInPlace,
        })
    );

    let mut valid = declaration(
        DavResourceState::Unmapped,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    valid.extensions = DavExtensionSet::from_packages(&[
        DavExtensionPackage::VersionControl,
        DavExtensionPackage::CheckoutInPlace,
        DavExtensionPackage::VersionHistory,
        DavExtensionPackage::Workspace,
    ]);
    let snapshot = plan_capabilities(valid).expect("complete workspace prerequisites");
    assert!(snapshot.supports(DavMethod::Mkworkspace));
    assert_eq!(
        snapshot.dav_header().expect("DAV header"),
        "1, version-control, checkout-in-place, version-history, workspace"
    );

    let method_without_package = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    assert_eq!(
        plan_capabilities(method_without_package),
        Err(DavCapabilityPlanError::ExtensionMethodWithoutPackage {
            method: DavMethod::VersionControl,
        })
    );

    let mut supported_but_disallowed = declaration(DavResourceState::File, &[DavMethod::Options]);
    supported_but_disallowed.extensions =
        DavExtensionSet::from_packages(&[DavExtensionPackage::VersionControl]);
    let supported_but_disallowed =
        plan_capabilities(supported_but_disallowed).expect("package support snapshot");
    assert!(supported_but_disallowed.supports(DavMethod::VersionControl));
    assert!(!supported_but_disallowed.allows(DavMethod::VersionControl));
    assert_eq!(
        supported_but_disallowed.body_policy(DavMethod::VersionControl, 64),
        Err(DavMethodGateError::MethodNotAllowed)
    );
}

#[test]
fn package_applicability_is_checked_before_discovery_is_rendered() {
    for (resource, package) in [
        (DavResourceState::File, DavExtensionPackage::CollectionSync),
        (
            DavResourceState::File,
            DavExtensionPackage::OrderedCollections,
        ),
        (
            DavResourceState::Collection,
            DavExtensionPackage::ExtendedMkcol,
        ),
        (DavResourceState::File, DavExtensionPackage::AddMember),
    ] {
        let mut declaration = declaration(resource, &[DavMethod::Options]);
        declaration.extensions = DavExtensionSet::from_packages(&[package]);
        assert_eq!(
            plan_capabilities(declaration),
            Err(DavCapabilityPlanError::ExtensionNotApplicable { package, resource })
        );
    }
}

#[test]
fn ordinary_post_does_not_imply_the_rfc_5995_add_member_package() {
    let ordinary = plan_capabilities(declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Post],
    ))
    .expect("product-specific POST");
    assert!(!ordinary.supports_extension(DavExtensionPackage::AddMember));
    assert_eq!(
        ordinary.body_policy(DavMethod::Post, 64),
        Ok(Some(aster_forge_webdav::DavBodyPolicy::Stream))
    );

    let mut add_member = declaration(
        DavResourceState::AddMemberEndpoint,
        &[DavMethod::Options, DavMethod::Post],
    );
    add_member.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::AddMember]);
    let add_member = plan_capabilities(add_member).expect("RFC 5995 endpoint");
    assert!(add_member.supports_extension(DavExtensionPackage::AddMember));
    assert_eq!(
        add_member.body_policy(DavMethod::Post, 64),
        Ok(Some(aster_forge_webdav::DavBodyPolicy::Stream))
    );
}

#[test]
fn search_generates_dasl_without_inventing_a_dav_token() {
    let mut search = declaration(
        DavResourceState::Collection,
        &[DavMethod::Options, DavMethod::Search],
    );
    search.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::Search]);
    assert_eq!(
        plan_capabilities(search.clone()),
        Err(DavCapabilityPlanError::SearchWithoutBasicSearch)
    );

    search.search = DavSearchCapabilities {
        grammars: &SEARCH_GRAMMARS,
    };
    let snapshot = plan_capabilities(search).expect("SEARCH discovery");
    assert_eq!(snapshot.dav_header().expect("class 1 only"), "1");
    assert_eq!(
        snapshot.dasl_header().expect("DASL header"),
        "<DAV:basicsearch>, <https://example.test/query>"
    );
    assert!(snapshot.supports(DavMethod::Search));
    assert!(
        snapshot
            .supports_live_property(aster_forge_webdav::DavLiveProperty::SupportedQueryGrammarSet)
    );

    let invalid_grammars: [&'static [DavSearchGrammar]; 6] = [
        &INVALID_SEARCH_CODED_URL,
        &INVALID_SEARCH_CODED_URL_DELIMITER,
        &INVALID_SEARCH_NAMESPACE,
        &INVALID_SEARCH_LOCAL_NAME,
        &DUPLICATE_SEARCH_CODED_URL,
        &DUPLICATE_SEARCH_XML_NAME,
    ];
    for grammars in invalid_grammars {
        let mut invalid = declaration(DavResourceState::Collection, &[DavMethod::Options]);
        invalid.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::Search]);
        invalid.search = DavSearchCapabilities { grammars };
        assert_eq!(
            plan_capabilities(invalid),
            Err(DavCapabilityPlanError::InvalidSearchGrammar),
            "invalid grammar descriptors: {grammars:?}"
        );
    }
}

#[test]
fn snapshot_is_the_single_method_body_and_discovery_source() {
    let basic = plan_capabilities(declaration(
        DavResourceState::Unmapped,
        &[DavMethod::Options, DavMethod::Mkcol],
    ))
    .expect("plain MKCOL");
    assert_eq!(
        basic.body_policy(DavMethod::Mkcol, 64),
        Ok(Some(aster_forge_webdav::DavBodyPolicy::Empty))
    );

    let mut extended = declaration(
        DavResourceState::Unmapped,
        &[DavMethod::Options, DavMethod::Mkcol],
    );
    extended.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::ExtendedMkcol]);
    let extended = plan_capabilities(extended).expect("extended MKCOL");
    assert_eq!(
        extended.body_policy(DavMethod::Mkcol, 64),
        Ok(Some(aster_forge_webdav::DavBodyPolicy::BoundedXml {
            maximum: 64,
        }))
    );
    assert_eq!(
        extended.dav_header().expect("DAV header"),
        "1, extended-mkcol"
    );

    let mut sync = declaration(
        DavResourceState::Collection,
        &[DavMethod::Options, DavMethod::Report],
    );
    sync.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::CollectionSync]);
    let sync = plan_capabilities(sync).expect("collection sync");
    assert_eq!(
        sync.body_policy(DavMethod::Report, 128),
        Ok(Some(aster_forge_webdav::DavBodyPolicy::BoundedXml {
            maximum: 128,
        }))
    );
    assert!(sync.supports_report(DavReportType::SyncCollection));

    let mut acl = declaration(
        DavResourceState::Principal,
        &[DavMethod::Options, DavMethod::Report],
    );
    acl.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::AccessControl]);
    let acl = plan_capabilities(acl).expect("ACL report snapshot");
    assert_eq!(
        acl.body_policy(DavMethod::Report, 96),
        Ok(Some(aster_forge_webdav::DavBodyPolicy::BoundedXml {
            maximum: 96,
        }))
    );

    assert_eq!(
        sync.body_policy(DavMethod::Search, 128),
        Err(DavMethodGateError::MethodNotAllowed)
    );

    let get = plan_capabilities(declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Get],
    ))
    .expect("GET snapshot");
    for method in [DavMethod::Get, DavMethod::Head] {
        assert_eq!(
            get.body_policy(method, 128),
            Ok(Some(aster_forge_webdav::DavBodyPolicy::Unused))
        );
    }

    let mut patch = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Patch],
    );
    patch.writes.patch = DavPatchCapability::Formats(&PATCH_FORMATS);
    let patch = plan_capabilities(patch).expect("PATCH snapshot");
    assert_eq!(patch.body_policy(DavMethod::Patch, 128), Ok(None));
}

#[test]
fn options_405_and_dispatch_observe_the_same_snapshot() {
    let mut declaration = declaration(
        DavResourceState::Collection,
        &[
            DavMethod::Options,
            DavMethod::Get,
            DavMethod::Propfind,
            DavMethod::Search,
        ],
    );
    declaration.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::Search]);
    declaration.search = DavSearchCapabilities {
        grammars: &[DavSearchGrammar::BASICSEARCH],
    };
    declaration.compatibility.ms_author_via = true;
    let snapshot = plan_capabilities(declaration).expect("valid snapshot");
    let options = options_response(&snapshot);
    assert_eq!(options.status, StatusCode::OK);
    assert_eq!(options.headers.get(ALLOW), Some(snapshot.allow_header()));
    assert_eq!(options.headers.get("DAV").expect("DAV header"), "1");
    assert_eq!(
        options.headers.get("DASL").expect("DASL header"),
        "<DAV:basicsearch>"
    );
    assert_eq!(options.headers.get("MS-Author-Via").expect("compat"), "DAV");
    assert_eq!(
        gate_method(Some(DavMethod::Search), &snapshot),
        Ok(DavMethod::Search)
    );
    assert_eq!(
        gate_method(Some(DavMethod::Put), &snapshot),
        Err(DavMethodGateError::MethodNotAllowed)
    );
    let rejected = method_not_allowed_response(&snapshot);
    assert_eq!(rejected.headers.get(ALLOW), options.headers.get(ALLOW));
    assert_eq!(
        rejected.headers.get(CACHE_CONTROL).expect("cache"),
        "no-store"
    );
}

struct CompleteProvider;

impl DavClass1Support for CompleteProvider {}
impl DavClass3Support for CompleteProvider {}
impl DavVersionControlSupport for CompleteProvider {}
impl DavSearchSupport for CompleteProvider {}
impl DavQuotaSupport for CompleteProvider {}
impl DavCollectionSyncSupport for CompleteProvider {}
impl DavPreferSupport for CompleteProvider {}

impl DavCapabilityProvider for CompleteProvider {
    type Profile = dav_capability_profile!(
        DavClass3Profile;
        DavVersionControlExtension,
        DavSearchExtension,
        DavQuotaExtension,
        DavCollectionSyncExtension,
        DavPreferExtension
    );

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        let mut declaration = declaration(
            DavResourceState::Collection,
            &[
                DavMethod::Options,
                DavMethod::Propfind,
                DavMethod::Report,
                DavMethod::Search,
            ],
        );
        declaration.compliance.class3 = true;
        declaration.extensions = DavExtensionSet::from_packages(&[
            DavExtensionPackage::VersionControl,
            DavExtensionPackage::Search,
            DavExtensionPackage::Quota,
            DavExtensionPackage::CollectionSync,
            DavExtensionPackage::Prefer,
        ]);
        declaration.search = DavSearchCapabilities {
            grammars: &[DavSearchGrammar::BASICSEARCH],
        };
        if context.principal.as_deref() == Some("minimal") {
            declaration.extensions = DavExtensionSet::from_packages(&[
                DavExtensionPackage::Quota,
                DavExtensionPackage::Prefer,
            ]);
            declaration.methods =
                DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Propfind]);
            declaration.search = DavSearchCapabilities::default();
        }
        Ok(declaration)
    }
}

#[test]
fn aggregate_profile_compiles_from_marker_traits_and_runtime_can_select_a_subset() {
    let target = DavCapabilityTarget::new(DavPath::new("/").expect("path"), true);
    let complete = futures::executor::block_on(plan_capabilities_with_provider(
        &CompleteProvider,
        &target,
        &DavCapabilityContext::default(),
    ))
    .expect("complete profile");
    assert_eq!(
        complete.dav_header().expect("DAV header"),
        "1, 3, version-control"
    );
    assert_eq!(complete.preferences(), DavPreferenceSet::all());

    let subset = futures::executor::block_on(plan_capabilities_with_provider(
        &CompleteProvider,
        &target,
        &DavCapabilityContext {
            principal: Some("minimal".to_owned()),
        },
    ))
    .expect("runtime subset");
    assert!(subset.supports_extension(DavExtensionPackage::Quota));
    assert!(!subset.supports_extension(DavExtensionPackage::Search));
}

struct WorkspaceProvider;
impl DavClass1Support for WorkspaceProvider {}
impl DavVersionControlSupport for WorkspaceProvider {}
impl DavCheckoutInPlaceSupport for WorkspaceProvider {}
impl DavVersionHistorySupport for WorkspaceProvider {}
impl DavWorkspaceSupport for WorkspaceProvider {}
impl DavCapabilityProvider for WorkspaceProvider {
    type Profile = dav_capability_profile!(DavClass1Profile; DavWorkspaceExtension);

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        let mut declaration = declaration(DavResourceState::Collection, &[DavMethod::Options]);
        declaration.extensions = DavExtensionSet::from_packages(&[
            DavExtensionPackage::VersionControl,
            DavExtensionPackage::CheckoutInPlace,
            DavExtensionPackage::VersionHistory,
            DavExtensionPackage::Workspace,
        ]);
        Ok(declaration)
    }
}

#[test]
fn workspace_marker_includes_all_rfc_prerequisite_packages_in_the_static_profile() {
    let snapshot = futures::executor::block_on(plan_capabilities_with_provider(
        &WorkspaceProvider,
        &DavCapabilityTarget::new(DavPath::new("/workspace").expect("path"), false),
        &DavCapabilityContext::default(),
    ))
    .expect("workspace profile and declaration");

    for package in [
        DavExtensionPackage::VersionControl,
        DavExtensionPackage::CheckoutInPlace,
        DavExtensionPackage::VersionHistory,
        DavExtensionPackage::Workspace,
    ] {
        assert!(snapshot.supports_extension(package), "package: {package:?}");
    }
}

struct Class1DeclaringQuota;
impl DavClass1Support for Class1DeclaringQuota {}
impl DavCapabilityProvider for Class1DeclaringQuota {
    type Profile = DavClass1Profile;

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        let mut declaration = declaration(DavResourceState::Collection, &[DavMethod::Options]);
        declaration.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::Quota]);
        Ok(declaration)
    }
}

#[test]
fn provider_rejects_runtime_classes_and_packages_above_the_static_profile() {
    let error = futures::executor::block_on(plan_capabilities_with_provider(
        &Class1DeclaringQuota,
        &DavCapabilityTarget::new(DavPath::new("/").expect("path"), true),
        &DavCapabilityContext::default(),
    ))
    .expect_err("quota exceeds class 1 profile");
    assert_eq!(
        error,
        DavCapabilityEvaluationError::Plan(DavCapabilityPlanError::ExtensionExceedsProfile {
            package: DavExtensionPackage::Quota,
        })
    );
}

struct DeclarationProvider<Profile> {
    declaration: DavCapabilityDeclaration,
    profile: PhantomData<fn() -> Profile>,
}

impl<Profile> DeclarationProvider<Profile> {
    fn new(declaration: DavCapabilityDeclaration) -> Self {
        Self {
            declaration,
            profile: PhantomData,
        }
    }
}

impl<Profile> DavClass1Support for DeclarationProvider<Profile> {}

impl<Profile> DavCapabilityProvider for DeclarationProvider<Profile>
where
    Profile: DavCapabilityProfile<Self>,
{
    type Profile = Profile;

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        Ok(self.declaration.clone())
    }
}

fn profile_error<Profile>(declaration: DavCapabilityDeclaration) -> DavCapabilityEvaluationError
where
    Profile: DavCapabilityProfile<DeclarationProvider<Profile>>,
{
    futures::executor::block_on(plan_capabilities_with_provider(
        &DeclarationProvider::<Profile>::new(declaration),
        &DavCapabilityTarget::new(DavPath::new("/target").expect("path"), false),
        &DavCapabilityContext::default(),
    ))
    .expect_err("declaration must exceed its profile")
}

#[test]
fn provider_rejects_every_runtime_axis_above_its_static_profile() {
    let class1 = declaration(DavResourceState::File, &[DavMethod::Options]);
    assert_eq!(
        profile_error::<DavNonDavProfile>(class1),
        DavCapabilityEvaluationError::Plan(DavCapabilityPlanError::Class1ExceedsProfile)
    );

    let mut class2 = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock],
    );
    class2.locking = DavLockingCapability::Class2;
    assert_eq!(
        profile_error::<DavClass1Profile>(class2),
        DavCapabilityEvaluationError::Plan(DavCapabilityPlanError::Class2ExceedsProfile)
    );

    let mut class3 = declaration(DavResourceState::File, &[DavMethod::Options]);
    class3.compliance.class3 = true;
    assert_eq!(
        profile_error::<DavClass1Profile>(class3),
        DavCapabilityEvaluationError::Plan(DavCapabilityPlanError::Class3ExceedsProfile)
    );

    let mut partial = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Put]),
    );
    partial.writes.partial_put = DavPartialPutCapability::ContentRangeBytes {
        precondition: DavWritePrecondition::Optional,
    };
    assert_eq!(
        profile_error::<DavNonDavProfile>(partial),
        DavCapabilityEvaluationError::Plan(DavCapabilityPlanError::PartialPutExceedsProfile)
    );

    let mut patch = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Patch]),
    );
    patch.writes.patch = DavPatchCapability::Formats(&PATCH_FORMATS);
    assert_eq!(
        profile_error::<DavNonDavProfile>(patch),
        DavCapabilityEvaluationError::Plan(DavCapabilityPlanError::PatchExceedsProfile)
    );

    let mut private = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Put]),
    );
    private.writes.private_update_range = DavPrivateUpdateRangeCapability::XUpdateRange {
        precondition: DavWritePrecondition::Optional,
    };
    assert_eq!(
        profile_error::<DavNonDavProfile>(private),
        DavCapabilityEvaluationError::Plan(
            DavCapabilityPlanError::PrivateUpdateRangeExceedsProfile
        )
    );
}

struct Class123Provider;
impl DavClass1Support for Class123Provider {}
impl DavClass2Support for Class123Provider {}
impl DavClass3Support for Class123Provider {}
impl DavCapabilityProvider for Class123Provider {
    type Profile = DavClass2And3Profile;

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        let mut declaration = declaration(
            DavResourceState::File,
            &[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock],
        );
        declaration.locking = DavLockingCapability::Class2;
        declaration.compliance.class3 = true;
        Ok(declaration)
    }
}

#[test]
fn class_123_profile_requires_both_independent_support_traits() {
    let snapshot = futures::executor::block_on(plan_capabilities_with_provider(
        &Class123Provider,
        &DavCapabilityTarget::new(DavPath::new("/file").expect("path"), false),
        &DavCapabilityContext::default(),
    ))
    .expect("class 1, 2, 3 profile");
    assert_eq!(snapshot.dav_header().expect("DAV header"), "1, 2, 3");
}

static PATCH_FORMATS: [DavPatchFormat; 2] = [
    DavPatchFormat {
        media_type: "application/merge-patch+json",
        body_policy: DavPatchBodyPolicy::Bounded { maximum: 4096 },
        precondition: DavWritePrecondition::RequireStrongIfMatch,
    },
    DavPatchFormat {
        media_type: "application/example-patch; version=1",
        body_policy: DavPatchBodyPolicy::Stream,
        precondition: DavWritePrecondition::Optional,
    },
];

#[test]
fn write_capabilities_remain_independent_from_rfc_package_tokens() {
    let mut write_declaration = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Put, DavMethod::Patch],
    );
    write_declaration.writes = DavWriteCapabilities {
        partial_put: DavPartialPutCapability::ContentRangeBytes {
            precondition: DavWritePrecondition::RequireStrongIfMatch,
        },
        patch: DavPatchCapability::Formats(&PATCH_FORMATS),
        private_update_range: DavPrivateUpdateRangeCapability::Disabled,
    };
    let snapshot = plan_capabilities(write_declaration).expect("write capabilities");
    assert_eq!(snapshot.dav_header().expect("class 1"), "1");
    assert_eq!(
        snapshot.accept_patch_header().expect("Accept-Patch"),
        "application/merge-patch+json, application/example-patch; version=1"
    );

    let mut partial_without_put = declaration(DavResourceState::File, &[DavMethod::Options]);
    partial_without_put.writes.partial_put = DavPartialPutCapability::ContentRangeBytes {
        precondition: DavWritePrecondition::Optional,
    };
    assert_eq!(
        plan_capabilities(partial_without_put),
        Err(DavCapabilityPlanError::PartialPutWithoutPut)
    );
}

#[test]
fn planner_rejects_incomplete_runtime_extension_and_write_declarations() {
    let mut extension_without_class1 = DavCapabilityDeclaration::new(
        DavResourceState::Collection,
        DavMethodSet::from_methods(&[DavMethod::Options]),
    );
    extension_without_class1.extensions =
        DavExtensionSet::from_packages(&[DavExtensionPackage::Quota]);
    assert_eq!(
        plan_capabilities(extension_without_class1),
        Err(DavCapabilityPlanError::ExtensionWithoutClass1 {
            package: DavExtensionPackage::Quota,
        })
    );

    let mut grammars_without_search =
        declaration(DavResourceState::Collection, &[DavMethod::Options]);
    grammars_without_search.search = DavSearchCapabilities {
        grammars: &[DavSearchGrammar::BASICSEARCH],
    };
    assert_eq!(
        plan_capabilities(grammars_without_search),
        Err(DavCapabilityPlanError::SearchGrammarsWithoutPackage)
    );

    let mut private_without_put = declaration(DavResourceState::File, &[DavMethod::Options]);
    private_without_put.writes.private_update_range =
        DavPrivateUpdateRangeCapability::XUpdateRange {
            precondition: DavWritePrecondition::Optional,
        };
    assert_eq!(
        plan_capabilities(private_without_put),
        Err(DavCapabilityPlanError::PrivateUpdateRangeWithoutPut)
    );

    let patch_without_formats = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Patch],
    );
    assert_eq!(
        plan_capabilities(patch_without_formats),
        Err(DavCapabilityPlanError::PatchWithoutFormats)
    );

    let mut empty_formats = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Patch],
    );
    empty_formats.writes.patch = DavPatchCapability::Formats(&[]);
    assert_eq!(
        plan_capabilities(empty_formats),
        Err(DavCapabilityPlanError::PatchWithoutFormats)
    );

    let mut formats_without_patch = declaration(DavResourceState::File, &[DavMethod::Options]);
    formats_without_patch.writes.patch = DavPatchCapability::Formats(&PATCH_FORMATS);
    assert_eq!(
        plan_capabilities(formats_without_patch),
        Err(DavCapabilityPlanError::PatchFormatsWithoutMethod)
    );
}

#[test]
fn patch_formats_use_complete_mime_equality_for_validation() {
    static INVALID: [DavPatchFormat; 1] = [DavPatchFormat {
        media_type: "not a media type",
        body_policy: DavPatchBodyPolicy::Stream,
        precondition: DavWritePrecondition::Optional,
    }];
    static DUPLICATE: [DavPatchFormat; 2] = [
        DavPatchFormat {
            media_type: "application/merge-patch+json",
            body_policy: DavPatchBodyPolicy::Stream,
            precondition: DavWritePrecondition::Optional,
        },
        DavPatchFormat {
            media_type: "APPLICATION/MERGE-PATCH+JSON",
            body_policy: DavPatchBodyPolicy::Bounded { maximum: 0 },
            precondition: DavWritePrecondition::RequireStrongIfMatch,
        },
    ];
    for (formats, expected) in [
        (&INVALID[..], DavCapabilityPlanError::InvalidPatchMediaType),
        (
            &DUPLICATE[..],
            DavCapabilityPlanError::DuplicatePatchMediaType,
        ),
    ] {
        let mut declaration = declaration(
            DavResourceState::File,
            &[DavMethod::Options, DavMethod::Patch],
        );
        declaration.writes.patch = DavPatchCapability::Formats(formats);
        assert_eq!(plan_capabilities(declaration), Err(expected));
    }
}

struct CompleteWriteProvider;
impl DavPartialPutSupport for CompleteWriteProvider {}
impl DavPatchSupport for CompleteWriteProvider {}
impl DavPrivateUpdateRangeSupport for CompleteWriteProvider {}
impl DavCapabilityProvider for CompleteWriteProvider {
    type Profile = DavWithPrivateUpdateRange<DavWithPatch<DavWithPartialPut<DavNonDavProfile>>>;

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        let mut declaration = DavCapabilityDeclaration::new(
            DavResourceState::File,
            DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Put, DavMethod::Patch]),
        );
        declaration.writes = DavWriteCapabilities {
            partial_put: DavPartialPutCapability::ContentRangeBytes {
                precondition: DavWritePrecondition::RequireStrongIfMatch,
            },
            patch: DavPatchCapability::Formats(&PATCH_FORMATS),
            private_update_range: DavPrivateUpdateRangeCapability::XUpdateRange {
                precondition: DavWritePrecondition::RequireStrongIfMatch,
            },
        };
        Ok(declaration)
    }
}

#[test]
fn composable_write_profiles_still_compile_without_extension_packages() {
    let snapshot = futures::executor::block_on(plan_capabilities_with_provider(
        &CompleteWriteProvider,
        &DavCapabilityTarget::new(DavPath::new("/file").expect("path"), false),
        &DavCapabilityContext::default(),
    ))
    .expect("write profile");
    assert!(snapshot.allows(DavMethod::Put));
    assert!(snapshot.allows(DavMethod::Patch));
}

struct FailingProvider;
impl DavCapabilityProvider for FailingProvider {
    type Profile = DavNonDavProfile;

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        Err(DavBackendError::new(DavBackendErrorKind::Internal))
    }
}

#[test]
fn provider_backend_failures_remain_distinct_from_plan_failures() {
    let error = futures::executor::block_on(plan_capabilities_with_provider(
        &FailingProvider,
        &DavCapabilityTarget::new(DavPath::new("/file").expect("path"), false),
        &DavCapabilityContext::default(),
    ))
    .expect_err("backend error");
    assert_eq!(
        error,
        DavCapabilityEvaluationError::Backend(DavBackendError::new(DavBackendErrorKind::Internal))
    );
}
