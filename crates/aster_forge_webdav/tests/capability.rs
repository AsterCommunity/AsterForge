use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavCapabilityContext, DavCapabilityDeclaration,
    DavCapabilityPlanError, DavCapabilityProvider, DavCapabilityTarget, DavClass1Profile,
    DavClass1Support, DavClass2Profile, DavClass2Support, DavCompatibilityCapabilities,
    DavComplianceClasses, DavLockingCapability, DavMethod, DavMethodGateError, DavMethodSet,
    DavNonDavProfile, DavPath, DavResourceState, DavVersioningCapability, gate_method,
    method_not_allowed_response, options_response, plan_capabilities,
    plan_capabilities_with_provider,
};
use http::StatusCode;
use http::header::{ALLOW, CACHE_CONTROL};

fn declaration(resource: DavResourceState, methods: &[DavMethod]) -> DavCapabilityDeclaration {
    DavCapabilityDeclaration {
        resource,
        methods: DavMethodSet::from_methods(methods),
        locking: DavLockingCapability::Disabled,
        versioning: DavVersioningCapability::Disabled,
        compatibility: DavCompatibilityCapabilities::default(),
        compliance: DavComplianceClasses { class1: true },
    }
}

#[test]
fn method_set_is_duplicate_free_and_uses_canonical_allow_order() {
    let set = DavMethodSet::from_methods(&[
        DavMethod::Propfind,
        DavMethod::Get,
        DavMethod::Options,
        DavMethod::Get,
        DavMethod::Delete,
    ]);
    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        [
            DavMethod::Options,
            DavMethod::Get,
            DavMethod::Delete,
            DavMethod::Propfind,
        ]
    );
    assert_eq!(set.render(), "OPTIONS, GET, DELETE, PROPFIND");
    assert!(!set.is_empty());
    assert!(DavMethodSet::empty().is_empty());
}

#[test]
fn every_protocol_method_round_trips_through_the_set_and_name_parser() {
    let set = DavMethodSet::from_methods(&DavMethod::ALL);
    assert_eq!(set.iter().count(), DavMethod::ALL.len());
    for method in DavMethod::ALL {
        assert!(set.contains(method));
        assert_eq!(DavMethod::from_name(method.as_str()), Some(method));
    }
}

#[test]
fn planner_preserves_all_resource_states_and_adds_head_for_get() {
    for resource in [
        DavResourceState::Unmapped,
        DavResourceState::File,
        DavResourceState::Collection,
        DavResourceState::MountRoot,
    ] {
        let snapshot =
            plan_capabilities(declaration(resource, &[DavMethod::Options, DavMethod::Get]))
                .expect("valid basic declaration");
        assert_eq!(snapshot.declaration().resource, resource);
        assert!(snapshot.allows(DavMethod::Get));
        assert!(snapshot.allows(DavMethod::Head));
        assert_eq!(snapshot.allow_header(), "OPTIONS, GET, HEAD");
        assert_eq!(snapshot.dav_header().expect("class 1 header"), "1");
    }
}

#[test]
fn planner_rejects_missing_options_and_head_without_get() {
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
fn locking_class_two_requires_class_one_and_both_lock_methods() {
    let mut without_class1 = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock],
    );
    without_class1.compliance.class1 = false;
    without_class1.locking = DavLockingCapability::Class2;
    assert_eq!(
        plan_capabilities(without_class1),
        Err(DavCapabilityPlanError::Class2WithoutClass1)
    );

    let mut incomplete = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Lock],
    );
    incomplete.locking = DavLockingCapability::Class2;
    assert_eq!(
        plan_capabilities(incomplete),
        Err(DavCapabilityPlanError::Class2WithoutLockMethods)
    );

    let lock_methods_without_class2 = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock],
    );
    assert_eq!(
        plan_capabilities(lock_methods_without_class2),
        Err(DavCapabilityPlanError::LockMethodsWithoutClass2)
    );

    for method in [DavMethod::Lock, DavMethod::Unlock] {
        assert_eq!(
            plan_capabilities(declaration(
                DavResourceState::File,
                &[DavMethod::Options, method],
            )),
            Err(DavCapabilityPlanError::Class2WithoutLockMethods)
        );
    }

    let mut valid = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::Lock, DavMethod::Unlock],
    );
    valid.locking = DavLockingCapability::Class2;
    let snapshot = plan_capabilities(valid).expect("valid class 2 declaration");
    assert_eq!(snapshot.dav_header().expect("DAV header"), "1, 2");
}

#[test]
fn core_versioning_requires_class_one_and_both_standard_methods() {
    let mut without_class1 = declaration(
        DavResourceState::File,
        &[
            DavMethod::Options,
            DavMethod::Report,
            DavMethod::VersionControl,
        ],
    );
    without_class1.compliance.class1 = false;
    without_class1.versioning = DavVersioningCapability::Core;
    assert_eq!(
        plan_capabilities(without_class1),
        Err(DavCapabilityPlanError::VersionControlWithoutClass1)
    );

    let mut missing_report = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    missing_report.versioning = DavVersioningCapability::Core;
    assert_eq!(
        plan_capabilities(missing_report),
        Err(DavCapabilityPlanError::VersionControlWithoutMethods)
    );

    let method_without_core = declaration(
        DavResourceState::File,
        &[DavMethod::Options, DavMethod::VersionControl],
    );
    assert_eq!(
        plan_capabilities(method_without_core),
        Err(DavCapabilityPlanError::VersionControlMethodWithoutCore)
    );

    let mut valid = declaration(
        DavResourceState::File,
        &[
            DavMethod::Options,
            DavMethod::Report,
            DavMethod::VersionControl,
        ],
    );
    valid.versioning = DavVersioningCapability::Core;
    let snapshot = plan_capabilities(valid).expect("valid core versioning declaration");
    assert_eq!(
        snapshot.dav_header().expect("DAV header"),
        "1, version-control"
    );
}

#[test]
fn options_405_and_dispatch_use_the_same_snapshot() {
    let mut declaration = declaration(
        DavResourceState::Collection,
        &[DavMethod::Options, DavMethod::Get, DavMethod::Propfind],
    );
    declaration.compatibility.ms_author_via = true;
    let snapshot = plan_capabilities(declaration).expect("valid collection declaration");

    let options = options_response(&snapshot);
    assert_eq!(options.status, StatusCode::OK);
    assert_eq!(options.headers.get(ALLOW), Some(snapshot.allow_header()));
    assert_eq!(options.headers.get("DAV").expect("DAV header"), "1");
    assert_eq!(
        options
            .headers
            .get("MS-Author-Via")
            .expect("compatibility header"),
        "DAV"
    );

    assert!(matches!(
        gate_method(Some(DavMethod::Propfind), &snapshot),
        Ok(DavMethod::Propfind)
    ));
    for method in [Some(DavMethod::Put), None] {
        assert_eq!(
            gate_method(method, &snapshot),
            Err(DavMethodGateError::MethodNotAllowed)
        );
        let response = method_not_allowed_response(&snapshot);
        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers.get(ALLOW), Some(snapshot.allow_header()));
        assert_eq!(
            response.headers.get(CACHE_CONTROL).expect("cache policy"),
            "no-store"
        );
    }

    let rejected = method_not_allowed_response(&snapshot);
    assert_eq!(rejected.headers.get(ALLOW), options.headers.get(ALLOW));
}

#[test]
fn non_dav_and_compatibility_disabled_omit_optional_headers() {
    let mut declaration = declaration(DavResourceState::Unmapped, &[DavMethod::Options]);
    declaration.compliance.class1 = false;
    let snapshot = plan_capabilities(declaration).expect("valid unmapped declaration");
    let response = options_response(&snapshot);
    assert!(response.headers.get("DAV").is_none());
    assert!(response.headers.get("MS-Author-Via").is_none());
}

struct ContextAwareProvider;

impl DavClass1Support for ContextAwareProvider {}

impl DavCapabilityProvider for ContextAwareProvider {
    type Profile = DavClass1Profile;

    async fn capabilities(
        &self,
        target: &DavCapabilityTarget,
        context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        let resource = if target.is_mount_root {
            DavResourceState::MountRoot
        } else {
            DavResourceState::File
        };
        let mut methods = DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Get]);
        if context.principal.as_deref() == Some("writer") {
            methods = methods.with(DavMethod::Put);
        }
        let mut declaration = DavCapabilityDeclaration {
            resource,
            methods,
            locking: DavLockingCapability::Disabled,
            versioning: DavVersioningCapability::Disabled,
            compatibility: DavCompatibilityCapabilities::default(),
            compliance: DavComplianceClasses { class1: true },
        };
        if context.principal.as_deref() == Some("lock-writer") {
            declaration.methods = DavMethodSet::from_methods(&[
                DavMethod::Options,
                DavMethod::Lock,
                DavMethod::Unlock,
            ]);
            declaration.locking = DavLockingCapability::Class2;
        }
        Ok(declaration)
    }
}

#[test]
fn provider_projects_target_and_principal_into_the_snapshot() {
    futures::executor::block_on(async {
        let provider = ContextAwareProvider;
        let target = DavCapabilityTarget::new(DavPath::new("/").expect("root path"), true);

        let reader = plan_capabilities_with_provider(
            &provider,
            &target,
            &DavCapabilityContext {
                principal: Some("reader".to_owned()),
            },
        )
        .await
        .expect("reader snapshot");
        assert_eq!(reader.declaration().resource, DavResourceState::MountRoot);
        assert!(!reader.allows(DavMethod::Put));

        let writer = plan_capabilities_with_provider(
            &provider,
            &target,
            &DavCapabilityContext {
                principal: Some("writer".to_owned()),
            },
        )
        .await
        .expect("writer snapshot");
        assert!(writer.allows(DavMethod::Put));
    });
}

#[test]
fn provider_profile_rejects_runtime_class_two_above_a_class_one_profile() {
    let error = futures::executor::block_on(plan_capabilities_with_provider(
        &ContextAwareProvider,
        &DavCapabilityTarget::new(DavPath::new("/file").expect("path"), false),
        &DavCapabilityContext {
            principal: Some("lock-writer".to_owned()),
        },
    ))
    .expect_err("class 2 exceeds the provider profile");
    assert!(matches!(
        error,
        aster_forge_webdav::DavCapabilityEvaluationError::Plan(
            DavCapabilityPlanError::Class2ExceedsProfile
        )
    ));
}

struct FailingProvider;

impl DavClass1Support for FailingProvider {}

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

struct Class2Provider;

impl DavClass1Support for Class2Provider {}
impl DavClass2Support for Class2Provider {}

impl DavCapabilityProvider for Class2Provider {
    type Profile = DavClass2Profile;

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
        Ok(declaration)
    }
}

#[test]
fn profile_types_compile_only_for_their_required_support_traits() {
    let snapshot = futures::executor::block_on(plan_capabilities_with_provider(
        &Class2Provider,
        &DavCapabilityTarget::new(DavPath::new("/file").expect("path"), false),
        &DavCapabilityContext::default(),
    ))
    .expect("class 2 profile should compile and plan");
    assert_eq!(snapshot.dav_header().expect("DAV header"), "1, 2");
}

#[test]
fn provider_backend_failures_remain_distinct_from_plan_failures() {
    let error = futures::executor::block_on(plan_capabilities_with_provider(
        &FailingProvider,
        &DavCapabilityTarget::new(DavPath::new("/file").expect("path"), false),
        &DavCapabilityContext::default(),
    ))
    .expect_err("provider error should propagate");
    assert!(matches!(
        error,
        aster_forge_webdav::DavCapabilityEvaluationError::Backend(DavBackendError {
            kind: DavBackendErrorKind::Internal
        })
    ));
}
