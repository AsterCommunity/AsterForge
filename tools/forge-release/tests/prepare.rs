#[expect(
    dead_code,
    reason = "Shared fixtures are used by separate integration test crates"
)]
mod support;
use forge_release::{
    checks::{check_workspace, load_workspace, refresh_lockfile},
    manifest::{ManifestWorkspace, replace_if_unchanged},
    model::CrateVersion,
    prepare::{discover, prepare, validate_new_version, write_release_manifest},
};
use std::fs;
use support::*;

#[test]
fn prepare_updates_aliased_inherited_and_target_pins_without_bumping_consumers() {
    let dir = fixture();
    let root = dir.path();
    let mut manifests = ManifestWorkspace::load(root).unwrap();
    let registry = FakeRegistry::default();
    let result = prepare(&mut manifests, &["zbase".into()], &registry, false).unwrap();
    assert_eq!(result.pin_updates.len(), 2);
    assert_eq!(result.plan.runtime_or_build_impact.len(), 2);
    let text = fs::read_to_string(root.join("aconsumer/Cargo.toml")).unwrap();
    assert!(text.contains("version = \"1.0.0\""));
    assert!(text.contains("version = \"=1.0.1\""));
    assert!(text.contains("# preserve target"));
    let text = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(text.contains("default-features = false"));
    assert!(text.contains("# preserve pin"));
    refresh_lockfile(root).unwrap();
    assert!(
        check_workspace(&load_workspace(root).unwrap(), root)
            .unwrap()
            .is_empty()
    );
    let lock = fs::read(root.join("Cargo.lock")).unwrap();
    refresh_lockfile(root).unwrap();
    assert_eq!(lock, fs::read(root.join("Cargo.lock")).unwrap());
    write_release_manifest(&root.join("release-plan.json"), &result.plan).unwrap();
    forge_release::publish::validate_manifest(
        &load_workspace(root).unwrap(),
        &forge_release::publish::read_manifest(&root.join("release-plan.json")).unwrap(),
    )
    .unwrap();
}
#[test]
fn dry_run_and_registry_failure_never_write_files() {
    let dir = fixture();
    let root = dir.path();
    let before = fs::read(root.join("Cargo.toml")).unwrap();
    let mut manifests = ManifestWorkspace::load(root).unwrap();
    prepare(
        &mut manifests,
        &["zbase".into()],
        &FakeRegistry::default(),
        true,
    )
    .unwrap();
    assert_eq!(before, fs::read(root.join("Cargo.toml")).unwrap());
    let mut manifests = ManifestWorkspace::load(root).unwrap();
    assert!(
        prepare(
            &mut manifests,
            &["zbase".into()],
            &FakeRegistry {
                fail: true,
                ..Default::default()
            },
            false
        )
        .is_err()
    );
    assert_eq!(before, fs::read(root.join("Cargo.toml")).unwrap());
    assert!(!root.join("release-plan.json").exists());
}
#[test]
fn discover_only_unpublished_publishable_versions() {
    let mut private = package("private", vec![]);
    private.publish = Some(vec![]);
    let workspace = graph(vec![
        package("published", vec![]),
        package("new", vec![]),
        private,
    ]);
    let mut registry = FakeRegistry::default();
    registry
        .versions
        .insert("published".into(), versions(&["1.0.0"]));
    assert_eq!(discover(&workspace, &registry).unwrap(), ["new"]);
}
#[test]
fn versions_follow_release_lines_and_reject_invalid_or_rollback_versions() {
    for bad in ["bad", "1.0.0+build", "1.0.0-alpha.1", "1.0.0-alpha.2"] {
        let result = validate_new_version(
            &CrateVersion {
                name: "base".into(),
                version: bad.into(),
            },
            &versions(&["1.0.0-alpha.2"]),
        );
        assert!(result.is_err(), "{bad}");
    }
    for good in ["1.0.0-alpha.3", "1.0.0", "0.9.2"] {
        validate_new_version(
            &CrateVersion {
                name: "base".into(),
                version: good.into(),
            },
            &versions(&["1.0.0-alpha.2", "2.0.0", "0.9.1"]),
        )
        .unwrap();
    }
}
#[test]
fn concurrent_edit_is_preserved_and_aborts_before_any_file_write() {
    let dir = fixture();
    let root = dir.path();
    let mut manifests = ManifestWorkspace::load(root).unwrap();
    manifests.update_pins(&["zbase".into()]).unwrap();
    fs::write(root.join("Cargo.toml"), "# concurrent author\n").unwrap();
    assert!(manifests.write_manifests().is_err());
    assert_eq!(
        fs::read_to_string(root.join("Cargo.toml")).unwrap(),
        "# concurrent author\n"
    );
    assert!(
        fs::read_to_string(root.join("aconsumer/Cargo.toml"))
            .unwrap()
            .contains("=1.0.0")
    );
    assert!(replace_if_unchanged(&root.join("Cargo.toml"), "wrong", "overwrite").is_err());
}
#[test]
fn cli_sync_fixes_version_only_edit_and_json_is_clean() {
    let dir = fixture();
    let root = dir.path();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_forge-release"))
        .args(["--root", root.to_str().unwrap(), "sync"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_forge-release"))
        .args([
            "--root",
            root.to_str().unwrap(),
            "plan",
            "--changed",
            "zbase",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let _: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success());
}

#[test]
fn unconsumed_root_alias_is_checked_and_private_or_existing_roots_are_rejected() {
    let dir = fixture();
    let root = dir.path();
    let mut text = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    text.push_str("unused = { package = \"zbase\", path = \"zbase\", version = \"=1.0.0\" }\n");
    fs::write(root.join("Cargo.toml"), text).unwrap();
    let mut manifests = ManifestWorkspace::load(root).unwrap();
    manifests.update_pins(&["zbase".into()]).unwrap();
    manifests.write_manifests().unwrap();
    refresh_lockfile(root).unwrap();
    let text = fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .replace(
            "unused = { package = \"zbase\", path = \"zbase\", version = \"=1.0.1\"",
            "unused = { package = \"zbase\", path = \"zbase\", version = \"=1.0.0\"",
        );
    fs::write(root.join("Cargo.toml"), text).unwrap();
    assert!(
        !check_workspace(&load_workspace(root).unwrap(), root)
            .unwrap()
            .is_empty()
    );
    let mut registry = FakeRegistry::default();
    registry
        .versions
        .insert("zbase".into(), versions(&["1.0.1"]));
    assert!(
        prepare(
            &mut ManifestWorkspace::load(root).unwrap(),
            &["zbase".into()],
            &registry,
            false
        )
        .is_err()
    );
}

#[test]
fn malformed_manifest_and_missing_lockfile_are_reported() {
    let dir = fixture();
    let root = dir.path();
    assert!(check_workspace(&load_workspace(root).unwrap(), root).is_err());
    fs::write(root.join("Cargo.toml"), "[broken").unwrap();
    assert!(ManifestWorkspace::load(root).is_err());
}
