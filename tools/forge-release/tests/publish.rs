#[expect(
    dead_code,
    reason = "Shared fixtures are used by separate integration test crates"
)]
mod support;
use forge_release::{
    model::ReleaseManifest,
    publish::{PublishStatus, publish_release},
};
use support::*;

fn batch() -> (forge_release::model::Workspace, ReleaseManifest) {
    let workspace = graph(vec![
        package("zbase", vec![]),
        package("aconsumer", vec![dependency("zbase", None)]),
    ]);
    let manifest = ReleaseManifest {
        format_version: 1,
        plan: workspace
            .plan(&["zbase".into(), "aconsumer".into()], &[])
            .unwrap(),
    };
    (workspace, manifest)
}
#[test]
fn whole_batch_is_staged_by_cargo_and_preserves_topology() {
    let (workspace, manifest) = batch();
    let mut publisher = FakePublisher::default();
    let result = publish_release(
        &workspace,
        &manifest,
        &FakeRegistry::default(),
        &mut publisher,
        false,
    )
    .unwrap();
    assert_eq!(
        publisher.batches,
        vec![(vec!["zbase".to_owned(), "aconsumer".to_owned()], true)]
    );
    assert!(result.iter().all(|r| r.status == PublishStatus::DryRun));
}
#[test]
fn successful_upload_and_partial_success_resume_use_only_pending_packages() {
    let (workspace, manifest) = batch();
    let mut publisher = FakePublisher::default();
    publish_release(
        &workspace,
        &manifest,
        &FakeRegistry {
            visible_after_publish: true,
            ..Default::default()
        },
        &mut publisher,
        true,
    )
    .unwrap();
    assert_eq!(publisher.batches[0].0, ["zbase", "aconsumer"]);
    assert!(!publisher.batches[0].1);
    let mut registry = FakeRegistry {
        visible_after_publish: true,
        ..Default::default()
    };
    registry
        .versions
        .insert("zbase".into(), versions(&["1.0.0"]));
    publisher.batches.clear();
    let results = publish_release(&workspace, &manifest, &registry, &mut publisher, true).unwrap();
    assert_eq!(results[0].status, PublishStatus::AlreadyPublished);
    assert_eq!(publisher.batches[0].0, ["aconsumer"]);
    registry
        .versions
        .insert("aconsumer".into(), versions(&["1.0.0"]));
    publisher.batches.clear();
    publish_release(&workspace, &manifest, &registry, &mut publisher, true).unwrap();
    assert!(publisher.batches.is_empty());
}
#[test]
fn missing_unselected_dependency_and_yanked_version_fail_before_upload() {
    let (workspace, mut manifest) = batch();
    manifest.plan = workspace.plan(&["aconsumer".into()], &[]).unwrap();
    let mut publisher = FakePublisher::default();
    let mut registry = FakeRegistry::default();
    assert!(publish_release(&workspace, &manifest, &registry, &mut publisher, true).is_err());
    assert!(publisher.batches.is_empty());
    let mut yanked = versions(&["1.0.0"]);
    yanked.1.insert("1.0.0".parse().unwrap());
    registry.versions.insert("zbase".into(), yanked);
    assert!(publish_release(&workspace, &manifest, &registry, &mut publisher, true).is_err());
    assert!(publisher.batches.is_empty());
}
#[test]
fn stale_forged_and_unknown_field_plans_are_rejected() {
    let (workspace, mut manifest) = batch();
    let mut publisher = FakePublisher::default();
    manifest.plan.publish_order.reverse();
    assert!(
        publish_release(
            &workspace,
            &manifest,
            &FakeRegistry::default(),
            &mut publisher,
            true
        )
        .is_err()
    );
    assert!(publisher.batches.is_empty());
    let mut encoded = serde_json::to_value(&manifest).unwrap();
    encoded["injected"] = true.into();
    assert!(serde_json::from_value::<ReleaseManifest>(encoded).is_err());
}
#[test]
fn private_crate_registry_outage_and_cargo_failure_propagate() {
    let (workspace, manifest) = batch();
    let mut publisher = FakePublisher::default();
    assert!(
        publish_release(
            &workspace,
            &manifest,
            &FakeRegistry {
                fail: true,
                ..Default::default()
            },
            &mut publisher,
            true
        )
        .is_err()
    );
    assert!(publisher.batches.is_empty());
    publisher.fail = true;
    assert!(
        publish_release(
            &workspace,
            &manifest,
            &FakeRegistry::default(),
            &mut publisher,
            true
        )
        .is_err()
    );
    let mut private = package("private", vec![]);
    private.publish = Some(vec![]);
    let ws = graph(vec![private]);
    let manifest = ReleaseManifest {
        format_version: 1,
        plan: ws.plan(&["private".into()], &[]).unwrap(),
    };
    assert!(
        publish_release(
            &ws,
            &manifest,
            &FakeRegistry::default(),
            &mut publisher,
            true
        )
        .is_err()
    );
}
#[test]
fn yanked_selected_version_is_never_silently_reported_as_completed() {
    let (workspace, manifest) = batch();
    let mut registry = FakeRegistry::default();
    let mut yanked = versions(&["1.0.0"]);
    yanked.1.insert("1.0.0".parse().unwrap());
    registry.versions.insert("aconsumer".into(), yanked);
    let mut publisher = FakePublisher::default();
    assert!(publish_release(&workspace, &manifest, &registry, &mut publisher, true).is_err());
    assert!(publisher.batches.is_empty());
}

#[test]
fn upload_success_without_registry_visibility_is_not_reported_as_completed() {
    let (workspace, manifest) = batch();
    let mut publisher = FakePublisher::default();
    let error = publish_release(
        &workspace,
        &manifest,
        &FakeRegistry::default(),
        &mut publisher,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("not available"));
    assert_eq!(publisher.batches.len(), 1);
}
