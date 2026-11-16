#[expect(
    dead_code,
    reason = "Shared fixtures are used by separate integration test crates"
)]
mod support;
use forge_release::{
    model::{Metadata, Workspace},
    publish::validate_manifest,
};
use support::*;

#[test]
fn topology_survives_serialization_despite_reverse_alphabetical_names() {
    let workspace = graph(vec![
        package("zdep", vec![]),
        package("aconsumer", vec![dependency("zdep", None)]),
    ]);
    let plan = workspace
        .plan(&["aconsumer".into(), "zdep".into()], &[])
        .unwrap();
    assert_eq!(
        plan.publish_order
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["zdep", "aconsumer"]
    );
    let json = serde_json::to_string(&plan).unwrap();
    let decoded: forge_release::model::ReleasePlan = serde_json::from_str(&json).unwrap();
    assert_eq!(plan, decoded);
}
#[test]
fn dev_path_is_validation_only_even_when_followed_by_a_normal_edge() {
    let workspace = graph(vec![
        package("base", vec![]),
        package("dev", vec![dependency("base", Some("dev"))]),
        package("top", vec![dependency("dev", None)]),
        package("normal", vec![dependency("base", None)]),
    ]);
    let plan = workspace.plan(&["base".into()], &[]).unwrap();
    assert_eq!(
        plan.dev_only_impact
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["dev", "top"]
    );
    assert_eq!(plan.publish_order.len(), 1);
}
#[test]
fn normal_path_wins_over_alternative_dev_path() {
    let workspace = graph(vec![
        package("base", vec![]),
        package(
            "both",
            vec![
                dependency("base", Some("dev")),
                dependency("base", Some("build")),
            ],
        ),
    ]);
    assert!(
        workspace
            .plan(&["base".into()], &[])
            .unwrap()
            .dev_only_impact
            .is_empty()
    );
}
#[test]
fn versioned_dev_cycles_reject_publication_but_unversioned_dev_cycles_do_not() {
    let workspace = graph(vec![
        package("a", vec![dependency("z", Some("dev"))]),
        package("z", vec![dependency("a", None)]),
    ]);
    assert!(workspace.plan(&["a".into(), "z".into()], &[]).is_err());
    let mut unversioned = dependency("z", Some("dev"));
    unversioned.req = "*".into();
    let workspace = graph(vec![
        package("a", vec![unversioned]),
        package("z", vec![dependency("a", None)]),
    ]);
    assert!(workspace.plan(&["a".into(), "z".into()], &[]).is_ok());
}
#[test]
fn normal_cycles_unknown_roots_and_empty_plans_are_rejected() {
    let workspace = graph(vec![
        package("a", vec![dependency("z", None)]),
        package("z", vec![dependency("a", None)]),
    ]);
    assert!(workspace.plan(&["a".into(), "z".into()], &[]).is_err());
    assert!(workspace.plan(&["missing".into()], &[]).is_err());
    assert!(workspace.plan(&[], &[]).is_err());
    assert!(workspace.publish_order(&["missing".into()].into()).is_err());
}
#[test]
fn registry_dependency_with_identical_name_is_not_a_workspace_edge() {
    let mut remote = dependency("local", None);
    remote.path = None;
    let workspace = graph(vec![
        package("local", vec![]),
        package("consumer", vec![remote]),
    ]);
    assert_eq!(
        workspace
            .plan(&["local".into()], &[])
            .unwrap()
            .runtime_or_build_impact
            .len(),
        1
    );
}
#[test]
fn duplicate_package_names_and_stale_manifest_are_rejected() {
    let mut first = package("same", vec![]);
    first.id = "one".into();
    let mut second = first.clone();
    second.id = "two".into();
    assert!(
        Workspace::from_metadata(Metadata {
            packages: vec![first, second],
            workspace_members: vec!["one".into(), "two".into()]
        })
        .is_err()
    );
    let workspace = graph(vec![package("base", vec![])]);
    let mut manifest = forge_release::model::ReleaseManifest {
        format_version: 1,
        plan: workspace.plan(&["base".into()], &[]).unwrap(),
    };
    manifest.plan.changed[0].version = "9.0.0".into();
    assert!(validate_manifest(&workspace, &manifest).is_err());
    manifest.format_version = 2;
    assert!(validate_manifest(&workspace, &manifest).is_err());
}

#[test]
fn versioned_dev_dependency_precedes_consumer_even_in_dev_only_impact() {
    let workspace = graph(vec![
        package("aconsumer", vec![dependency("zsupport", Some("dev"))]),
        package("zsupport", vec![]),
    ]);
    let plan = workspace
        .plan(
            &["zsupport".into()],
            &["aconsumer".into(), "zsupport".into()],
        )
        .unwrap();
    assert_eq!(plan.dev_only_impact[0].name, "aconsumer");
    assert_eq!(
        plan.publish_order
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["zsupport", "aconsumer"]
    );
}
