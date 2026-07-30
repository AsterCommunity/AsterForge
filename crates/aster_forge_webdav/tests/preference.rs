use aster_forge_webdav::{
    DavCapabilityDeclaration, DavExtensionPackage, DavExtensionSet, DavMethod, DavMethodSet,
    DavPreferenceSet, DavResourceState, Depth, plan_capabilities, plan_preferences,
};
use http::HeaderMap;
use http::header::{HeaderName, HeaderValue};

fn snapshot(
    resource: DavResourceState,
    methods: &[DavMethod],
    packages: &[DavExtensionPackage],
) -> aster_forge_webdav::DavCapabilitySnapshot {
    let mut declaration =
        DavCapabilityDeclaration::new(resource, DavMethodSet::from_methods(methods));
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(packages);
    plan_capabilities(declaration).expect("preference snapshot")
}

fn prefer(value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Prefer", HeaderValue::from_static(value));
    headers
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
fn depth_noroot_requires_depth_one_or_infinity_and_renders_canonically() {
    let snapshot = snapshot(
        DavResourceState::Collection,
        &[DavMethod::Options, DavMethod::Get, DavMethod::Propfind],
        &[DavExtensionPackage::Prefer],
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
