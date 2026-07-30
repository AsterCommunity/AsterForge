use aster_forge_webdav::{
    DavCapabilityDeclaration, DavExtensionPackage, DavExtensionSet, DavLockingCapability,
    DavMethod, DavMethodSet, DavPreferenceSet, DavResourceState, Depth, plan_capabilities,
    plan_preferences,
};
use http::HeaderMap;
use http::header::{HeaderName, HeaderValue};

static PATCH_FORMATS: [aster_forge_webdav::DavPatchFormat; 1] =
    [aster_forge_webdav::DavPatchFormat {
        media_type: "application/example-patch",
        body_policy: aster_forge_webdav::DavPatchBodyPolicy::Bounded { maximum: 64 },
        precondition: aster_forge_webdav::DavWritePrecondition::Optional,
    }];

fn snapshot(
    resource: DavResourceState,
    methods: &[DavMethod],
    packages: &[DavExtensionPackage],
) -> aster_forge_webdav::DavCapabilitySnapshot {
    let mut declaration =
        DavCapabilityDeclaration::new(resource, DavMethodSet::from_methods(methods));
    declaration.compliance.class1 = true;
    if methods.contains(&DavMethod::Lock) && methods.contains(&DavMethod::Unlock) {
        declaration.locking = DavLockingCapability::Class2;
    }
    declaration.extensions = DavExtensionSet::from_packages(packages);
    plan_capabilities(declaration).expect("preference snapshot")
}

fn prefer(value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Prefer", HeaderValue::from_static(value));
    headers
}

fn snapshot_allowing(method: DavMethod) -> aster_forge_webdav::DavCapabilitySnapshot {
    let resources = [
        DavResourceState::Collection,
        DavResourceState::File,
        DavResourceState::Unmapped,
        DavResourceState::MountRoot,
        DavResourceState::Principal,
        DavResourceState::RedirectReference,
        DavResourceState::AddMemberEndpoint,
    ];
    let extension_only = matches!(
        method,
        DavMethod::Acl
            | DavMethod::Report
            | DavMethod::VersionControl
            | DavMethod::Checkout
            | DavMethod::Checkin
            | DavMethod::Uncheckout
            | DavMethod::Mkworkspace
            | DavMethod::Update
            | DavMethod::Label
            | DavMethod::Merge
            | DavMethod::BaselineControl
            | DavMethod::Mkactivity
            | DavMethod::Search
            | DavMethod::Orderpatch
            | DavMethod::Mkredirectref
            | DavMethod::Updateredirectref
            | DavMethod::Bind
            | DavMethod::Unbind
            | DavMethod::Rebind
    );
    let package_and_resource = extension_only.then(|| {
        DavExtensionPackage::ALL
            .into_iter()
            .find_map(|package| {
                let descriptor = package.descriptor();
                descriptor
                    .methods
                    .iter()
                    .find(|candidate| candidate.method == method)
                    .and_then(|candidate| {
                        resources
                            .into_iter()
                            .find(|resource| {
                                descriptor.resources.contains(*resource)
                                    && candidate.resources.contains(*resource)
                            })
                            .map(|resource| (package, resource))
                    })
            })
            .expect("every extension-only method has an applicable descriptor")
    });
    let resource = package_and_resource.map_or_else(
        || {
            if method == DavMethod::Mkcol {
                DavResourceState::Unmapped
            } else {
                DavResourceState::File
            }
        },
        |(_, resource)| resource,
    );
    let mut methods = DavMethodSet::from_methods(&[DavMethod::Options, method]);
    if method == DavMethod::Head {
        methods = methods.with(DavMethod::Get);
    }
    if matches!(method, DavMethod::Lock | DavMethod::Unlock) {
        methods = methods.with(DavMethod::Lock).with(DavMethod::Unlock);
    }
    let mut declaration = DavCapabilityDeclaration::new(resource, methods);
    declaration.compliance.class1 = true;
    declaration.extensions = package_and_resource.map_or_else(
        || DavExtensionSet::from_packages(&[DavExtensionPackage::Prefer]),
        |(package, _)| {
            package
                .descriptor()
                .prerequisites
                .with(package)
                .with(DavExtensionPackage::Prefer)
        },
    );
    if declaration.extensions.contains(DavExtensionPackage::Search) {
        declaration.search = aster_forge_webdav::DavSearchCapabilities {
            grammars: &[aster_forge_webdav::DavSearchGrammar::BASICSEARCH],
        };
    }
    if matches!(method, DavMethod::Lock | DavMethod::Unlock) {
        declaration.locking = DavLockingCapability::Class2;
    }
    if method == DavMethod::Patch {
        declaration.writes.patch = aster_forge_webdav::DavPatchCapability::Formats(&PATCH_FORMATS);
    }
    plan_capabilities(declaration)
        .unwrap_or_else(|error| panic!("snapshot allowing {method:?}: {error}"))
}

#[test]
fn prefer_is_inert_until_the_typed_package_is_enabled() {
    let snapshot = snapshot(
        DavResourceState::Collection,
        &[DavMethod::Options, DavMethod::Propfind],
        &[],
    );
    let plan = plan_preferences(
        &snapshot,
        &prefer("return=minimal, depth-noroot"),
        DavMethod::Propfind,
        Some(Depth::Infinity),
    );
    assert_eq!(plan, aster_forge_webdav::DavPreferencePlan::default());
}

#[test]
fn return_minimal_applies_only_to_the_rfc_webdav_response_shapes() {
    let snapshot = snapshot(
        DavResourceState::Collection,
        &[
            DavMethod::Options,
            DavMethod::Get,
            DavMethod::Propfind,
            DavMethod::Proppatch,
        ],
        &[DavExtensionPackage::Prefer],
    );
    for method in [DavMethod::Propfind, DavMethod::Proppatch] {
        let plan = plan_preferences(
            &snapshot,
            &prefer("RETURN=MINIMAL; handling=lenient, unknown=value"),
            method,
            None,
        );
        assert!(plan.return_minimal);
        assert_eq!(
            plan.preference_applied_header(DavPreferenceSet::RETURN_MINIMAL)
                .expect("applied"),
            "return=minimal"
        );
    }

    let ignored = plan_preferences(&snapshot, &prefer("return=minimal"), DavMethod::Get, None);
    assert!(!ignored.return_minimal);
    assert!(
        ignored
            .preference_applied_header(DavPreferenceSet::RETURN_MINIMAL)
            .is_none()
    );
}

#[test]
fn extended_mkcol_uses_return_minimal_but_plain_mkcol_does_not() {
    let plain = snapshot(
        DavResourceState::Unmapped,
        &[DavMethod::Options, DavMethod::Mkcol],
        &[DavExtensionPackage::Prefer],
    );
    assert!(
        !plan_preferences(&plain, &prefer("return=minimal"), DavMethod::Mkcol, None,)
            .return_minimal
    );

    let extended = snapshot(
        DavResourceState::Unmapped,
        &[DavMethod::Options, DavMethod::Mkcol],
        &[
            DavExtensionPackage::ExtendedMkcol,
            DavExtensionPackage::Prefer,
        ],
    );
    assert!(
        plan_preferences(&extended, &prefer("return=minimal"), DavMethod::Mkcol, None,)
            .return_minimal
    );
}

#[test]
fn return_representation_is_limited_to_state_changing_representation_methods() {
    let snapshot = snapshot(
        DavResourceState::File,
        &[
            DavMethod::Options,
            DavMethod::Get,
            DavMethod::Put,
            DavMethod::Copy,
            DavMethod::Move,
        ],
        &[DavExtensionPackage::Prefer],
    );
    for method in [DavMethod::Put, DavMethod::Copy, DavMethod::Move] {
        let plan = plan_preferences(&snapshot, &prefer("return=representation"), method, None);
        assert!(plan.return_representation);
    }
    assert!(
        !plan_preferences(
            &snapshot,
            &prefer("return=representation"),
            DavMethod::Get,
            None,
        )
        .return_representation
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "This test is one complete RFC 8144 depth-noroot method and rendering matrix."
)]
fn depth_noroot_requires_depth_one_or_infinity_and_renders_canonically() {
    let snapshot = snapshot(
        DavResourceState::Collection,
        &[
            DavMethod::Options,
            DavMethod::Get,
            DavMethod::Propfind,
            DavMethod::Delete,
            DavMethod::Copy,
            DavMethod::Move,
            DavMethod::Lock,
            DavMethod::Unlock,
            DavMethod::Report,
        ],
        &[
            DavExtensionPackage::VersionControl,
            DavExtensionPackage::Prefer,
        ],
    );
    for depth in [None, Some(Depth::Zero)] {
        assert!(
            !plan_preferences(
                &snapshot,
                &prefer("depth-noroot"),
                DavMethod::Propfind,
                depth,
            )
            .depth_no_root
        );
    }
    for depth in [Some(Depth::One), Some(Depth::Infinity)] {
        let plan = plan_preferences(
            &snapshot,
            &prefer("depth-noroot, return=minimal"),
            DavMethod::Propfind,
            depth,
        );
        assert!(plan.return_minimal);
        assert!(plan.depth_no_root);
        assert_eq!(
            plan.preference_applied_header(
                DavPreferenceSet::RETURN_MINIMAL.union(DavPreferenceSet::DEPTH_NO_ROOT),
            )
            .expect("applied"),
            "return=minimal, depth-noroot"
        );
        assert_eq!(
            plan.preference_applied_header(DavPreferenceSet::RETURN_MINIMAL)
                .expect("only minimal was honored"),
            "return=minimal"
        );
        assert!(
            plan.preference_applied_header(DavPreferenceSet::RETURN_REPRESENTATION)
                .is_none(),
            "an unrequested preference is never claimed"
        );
    }
    assert!(
        !plan_preferences(
            &snapshot,
            &prefer("depth-noroot"),
            DavMethod::Get,
            Some(Depth::One),
        )
        .depth_no_root,
        "GET does not define Depth processing"
    );
    for method in [
        DavMethod::Propfind,
        DavMethod::Delete,
        DavMethod::Copy,
        DavMethod::Move,
        DavMethod::Lock,
        DavMethod::Report,
    ] {
        assert!(
            plan_preferences(
                &snapshot,
                &prefer("depth-noroot"),
                method,
                Some(Depth::Infinity),
            )
            .depth_no_root,
            "{method:?} explicitly supports Depth"
        );
    }
    assert!(
        plan_preferences(
            &snapshot,
            &prefer("depth-noroot"),
            DavMethod::Report,
            Some(Depth::One),
        )
        .depth_no_root,
        "REPORT permits Depth: 1"
    );
    for method in [DavMethod::Options, DavMethod::Get, DavMethod::Unlock] {
        assert!(
            !plan_preferences(
                &snapshot,
                &prefer("depth-noroot"),
                method,
                Some(Depth::Infinity),
            )
            .depth_no_root,
            "{method:?} is outside Forge's RFC 8144 Depth method set"
        );
    }
    for method in DavMethod::ALL {
        let method_snapshot = snapshot_allowing(method);
        let plan = plan_preferences(
            &method_snapshot,
            &prefer("depth-noroot"),
            method,
            Some(Depth::Infinity),
        );
        assert_eq!(
            plan.depth_no_root,
            matches!(
                method,
                DavMethod::Propfind
                    | DavMethod::Delete
                    | DavMethod::Copy
                    | DavMethod::Move
                    | DavMethod::Lock
                    | DavMethod::Report
            ),
            "complete depth-noroot classification for {method:?}"
        );
    }
    assert_eq!(
        plan_preferences(
            &snapshot,
            &prefer("return=representation"),
            DavMethod::Put,
            None,
        ),
        aster_forge_webdav::DavPreferencePlan::default(),
        "preferences are inert for a method rejected by the snapshot"
    );
}

#[test]
fn multiple_prefer_fields_are_combined_without_preserving_unknown_tokens() {
    let snapshot = snapshot(
        DavResourceState::Collection,
        &[DavMethod::Options, DavMethod::Propfind],
        &[DavExtensionPackage::Prefer],
    );
    let mut headers = HeaderMap::new();
    let name = HeaderName::from_static("prefer");
    headers.append(&name, HeaderValue::from_static("unknown=TARGET"));
    headers.append(&name, HeaderValue::from_static("return=minimal"));
    headers.append(&name, HeaderValue::from_static("depth-noroot"));
    let plan = plan_preferences(&snapshot, &headers, DavMethod::Propfind, Some(Depth::One));
    assert_eq!(
        plan.preference_applied_header(
            DavPreferenceSet::RETURN_MINIMAL.union(DavPreferenceSet::DEPTH_NO_ROOT),
        )
        .expect("applied"),
        "return=minimal, depth-noroot"
    );

    headers.append(
        &name,
        HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
    );
    assert!(
        plan_preferences(&snapshot, &headers, DavMethod::Propfind, Some(Depth::One)).return_minimal,
        "a non-text field must be ignored without discarding other Prefer fields"
    );

    let quoted = plan_preferences(
        &snapshot,
        &prefer(r#"foo="x,return=minimal, y""#),
        DavMethod::Propfind,
        Some(Depth::One),
    );
    assert!(!quoted.return_minimal);
    let escaped_quote = plan_preferences(
        &snapshot,
        &prefer(r#"foo="x\",return=minimal, y""#),
        DavMethod::Propfind,
        Some(Depth::One),
    );
    assert!(!escaped_quote.return_minimal);
}

#[test]
fn preference_applied_uses_every_fixed_canonical_combination() {
    let plan = aster_forge_webdav::DavPreferencePlan {
        return_minimal: true,
        return_representation: true,
        depth_no_root: true,
    };
    for (applied, expected) in [
        (DavPreferenceSet::RETURN_MINIMAL, "return=minimal"),
        (
            DavPreferenceSet::RETURN_REPRESENTATION,
            "return=representation",
        ),
        (DavPreferenceSet::DEPTH_NO_ROOT, "depth-noroot"),
        (
            DavPreferenceSet::RETURN_MINIMAL.union(DavPreferenceSet::RETURN_REPRESENTATION),
            "return=minimal, return=representation",
        ),
        (
            DavPreferenceSet::RETURN_MINIMAL.union(DavPreferenceSet::DEPTH_NO_ROOT),
            "return=minimal, depth-noroot",
        ),
        (
            DavPreferenceSet::RETURN_REPRESENTATION.union(DavPreferenceSet::DEPTH_NO_ROOT),
            "return=representation, depth-noroot",
        ),
        (
            DavPreferenceSet::all(),
            "return=minimal, return=representation, depth-noroot",
        ),
    ] {
        assert_eq!(
            plan.preference_applied_header(applied)
                .expect("non-empty applied set"),
            expected
        );
    }
}
