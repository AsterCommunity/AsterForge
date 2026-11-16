//! Detect explicit unpublished versions, sync dependencies, and record a reviewable batch.

use crate::{
    manifest::{ManifestWorkspace, PinUpdate},
    model::{CrateVersion, RegistryVersions, ReleaseManifest, ReleasePlan, Workspace},
    registry::Registry,
};
use anyhow::{Context, Result, bail};
use std::{fs, path::Path};

/// File and package changes displayed in the release pull request.
#[derive(Debug, serde::Serialize)]
pub struct PrepareResult {
    pub pin_updates: Vec<PinUpdate>,
    pub plan: ReleasePlan,
}

/// Select local versions missing from the registry. Existing versions never imply a release.
pub fn discover(workspace: &Workspace, registry: &dyn Registry) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for package in workspace.packages.values() {
        if package.publish.as_ref().is_some_and(Vec::is_empty) {
            continue;
        }
        let versions = registry.versions(&package.name)?;
        let version = semver::Version::parse(&package.version)?;
        if versions.contains(&version) {
            continue;
        }
        validate_new_version(
            &CrateVersion {
                name: package.name.clone(),
                version: package.version.clone(),
            },
            &versions,
        )?;
        names.push(package.name.clone());
    }
    Ok(names)
}

/// Check every release root before editing any file; downstream package versions are untouched.
pub fn prepare(
    manifests: &mut ManifestWorkspace,
    packages: &[String],
    registry: &dyn Registry,
    dry_run: bool,
) -> Result<PrepareResult> {
    let plan = manifests.workspace.plan(packages, packages)?;
    for package in &plan.changed {
        let local = manifests
            .workspace
            .package(&package.name)
            .context("unknown release package")?;
        if local.publish.as_ref().is_some_and(Vec::is_empty) {
            bail!("{} has publish = false", package.name);
        }
        validate_new_version(package, &registry.versions(&package.name)?)?;
    }
    let pin_updates = manifests.update_pins(packages)?;
    // Check newly-versioned dev edges and cycles before writing any manifest.
    let mut plan = manifests.workspace.plan(packages, packages)?;
    if !dry_run {
        manifests.write_manifests()?;
        manifests.reload_workspace()?;
        plan = manifests.workspace.plan(packages, packages)?;
    }
    Ok(PrepareResult { pin_updates, plan })
}

/// Write only a deterministic, explicit release set; a merge is the publication approval.
pub fn write_release_manifest(path: &Path, plan: &ReleasePlan) -> Result<()> {
    let manifest = ReleaseManifest {
        format_version: 1,
        plan: plan.clone(),
    };
    let mut content = serde_json::to_string_pretty(&manifest)?;
    content.push('\n');
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

/// Reject repeated versions and rollbacks within a release line; other maintenance lines remain usable.
pub fn validate_new_version(root: &CrateVersion, versions: &RegistryVersions) -> Result<()> {
    let version = semver::Version::parse(&root.version)
        .with_context(|| format!("invalid version {} {}", root.name, root.version))?;
    if !version.build.is_empty() {
        bail!(
            "{}: release versions must not contain build metadata",
            root.name
        );
    }
    if versions.contains(&version) {
        bail!("{} {} is already published", root.name, version);
    }
    // A 1.x maintenance release may follow 2.x; prereleases share the same core release line.
    let latest = versions
        .0
        .iter()
        .filter(|v| {
            if !version.pre.is_empty() {
                (v.major, v.minor, v.patch) == (version.major, version.minor, version.patch)
            } else {
                v.major == version.major && v.minor == version.minor
            }
        })
        .max();
    if latest.is_some_and(|latest| version <= *latest) {
        bail!(
            "{} {} does not advance its published release line",
            root.name,
            version
        );
    }
    Ok(())
}
