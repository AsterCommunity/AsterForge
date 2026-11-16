#[expect(
    dead_code,
    reason = "Shared fixtures are used by separate integration test crates"
)]
mod support;
use forge_release::{
    checks::refresh_lockfile,
    lockfile::{normalize_local_versions, validate_committed_lock},
    manifest::ManifestWorkspace,
};
use std::fs;
use support::*;

#[test]
fn normalization_preserves_external_versions_checksums_and_dependencies() {
    let workspace = graph(vec![package("local", vec![])]);
    let before = r#"version = 4
[[package]]
name = "local"
version = "0.9.0"
[[package]]
name = "local"
version = "0.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "unchanged"
dependencies = ["local 0.9.0", "local 0.9.0 (registry+https://github.com/rust-lang/crates.io-index)"]
"#;
    let normalized = normalize_local_versions(before, &workspace).unwrap();
    let parsed: toml::Value = toml::from_str(&normalized).unwrap();
    let packages = parsed["package"].as_array().unwrap();
    assert_eq!(packages[0]["version"].as_str(), Some("1.0.0"));
    assert_eq!(packages[1]["version"].as_str(), Some("0.9.0"));
    assert_eq!(packages[1]["checksum"].as_str(), Some("unchanged"));
    assert_eq!(packages[1]["dependencies"][0].as_str(), Some("local 1.0.0"));
    assert!(
        packages[1]["dependencies"][1]
            .as_str()
            .unwrap()
            .contains("0.9.0 (registry")
    );
}
#[test]
fn ci_sync_accepts_version_only_edit_but_rejects_missing_dependency_lock_entries() {
    let directory = fixture();
    let root = directory.path();
    let mut manifests = ManifestWorkspace::load(root).unwrap();
    manifests.update_pins(&["zbase".into()]).unwrap();
    manifests.write_manifests().unwrap();
    refresh_lockfile(root).unwrap();
    let path = root.join("Cargo.lock");
    let lock = fs::read_to_string(&path)
        .unwrap()
        .replace("version = \"1.0.1\"", "version = \"1.0.0\"");
    fs::write(&path, lock).unwrap();
    validate_committed_lock(root, &ManifestWorkspace::load(root).unwrap().workspace).unwrap();
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("version = \"1.0.1\"")
    );
    let lock = fs::read_to_string(&path).unwrap();
    // Deleting an entry is not a version bump: --locked must refuse to heal it.
    let incomplete = lock.replace("[[package]]\nname = \"zbase\"\nversion = \"1.0.1\"\n", "");
    assert_ne!(incomplete, lock);
    fs::write(&path, &incomplete).unwrap();
    assert!(
        validate_committed_lock(root, &ManifestWorkspace::load(root).unwrap().workspace).is_err()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), incomplete);
}
#[test]
fn ci_sync_rejects_missing_lockfile_and_invalid_toml() {
    let dir = fixture();
    let root = dir.path();
    let manifests = ManifestWorkspace::load(root).unwrap();
    assert!(validate_committed_lock(root, &manifests.workspace).is_err());
    fs::write(root.join("Cargo.lock"), "invalid [").unwrap();
    assert!(validate_committed_lock(root, &manifests.workspace).is_err());
}

#[test]
fn ci_lock_check_does_not_heal_new_third_party_path_dependency() {
    let dir = fixture();
    let root = dir.path();
    let mut manifests = ManifestWorkspace::load(root).unwrap();
    manifests.update_pins(&["zbase".into()]).unwrap();
    manifests.write_manifests().unwrap();
    refresh_lockfile(root).unwrap();
    let before = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    let root_manifest = root.join("Cargo.toml");
    let text = fs::read_to_string(&root_manifest)
        .unwrap()
        .replace("[workspace]", "[workspace]\nexclude = [\"third_party\"]");
    fs::write(root_manifest, text).unwrap();
    // A nonmember dependency has its own workspace and is not a permitted local version identity.
    support::write_member(
        root,
        "third_party",
        "[workspace]\n[package]\nname=\"third_party\"\nversion=\"1.0.0\"\nedition=\"2024\"\n",
    );
    let manifest = root.join("aconsumer/Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap().replace(
        "[dependencies]",
        "[dependencies]\nthird_party={path=\"../third_party\"}",
    );
    fs::write(&manifest, text).unwrap();
    assert!(
        validate_committed_lock(root, &ManifestWorkspace::load(root).unwrap().workspace).is_err()
    );
    assert_eq!(before, fs::read_to_string(root.join("Cargo.lock")).unwrap());
}
