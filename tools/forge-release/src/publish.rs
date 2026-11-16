//! Publication validates a reviewed batch before delegating archives and uploads.

use crate::{
    model::{CrateVersion, ReleaseManifest, Workspace},
    prepare::validate_new_version,
    registry::Registry,
};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeSet, path::Path, process::Command};

/// Backend boundary between archive verification and real registry uploads.
pub trait Publisher {
    /// Dry-run or upload precisely this batch; never select the whole workspace implicitly.
    fn run(&mut self, packages: &[CrateVersion], dry_run: bool) -> Result<()>;
}

/// Verify batch archives, then use native Cargo for ordered uploads and visibility waits.
pub struct CargoPublisher<'a> {
    root: &'a Path,
}
impl<'a> CargoPublisher<'a> {
    /// Bind publication to the checked-out release commit.
    pub fn new(root: &'a Path) -> Self {
        Self { root }
    }
}
impl Publisher for CargoPublisher<'_> {
    fn run(&mut self, packages: &[CrateVersion], dry_run: bool) -> Result<()> {
        if dry_run {
            return crate::archive::verify_batch(self.root, packages);
        }
        for package in packages {
            let status = Command::new("cargo")
                .args([
                    "publish",
                    "--locked",
                    "--registry",
                    "crates-io",
                    "--package",
                    &package.name,
                ])
                .env("CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE", "allow")
                .current_dir(self.root)
                .status()
                .context("failed to execute cargo publish")?;
            if !status.success() {
                bail!(
                    "cargo publish failed with {status}; honor the registry retry time and rerun the same merge SHA"
                );
            }
        }
        Ok(())
    }
}

/// Validate the entire batch before the first side effect, then skip completed uploads on retry.
pub fn publish_release<R: Registry, P: Publisher>(
    workspace: &Workspace,
    manifest: &ReleaseManifest,
    registry: &R,
    publisher: &mut P,
    upload: bool,
) -> Result<Vec<PublishOutcome>> {
    validate_manifest(workspace, manifest)?;
    let selected: BTreeSet<_> = manifest
        .plan
        .changed
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    let mut pending = Vec::new();
    let mut outcomes = Vec::new();
    for package in &manifest.plan.publish_order {
        let version = semver::Version::parse(&package.version)?;
        let versions = registry.versions(&package.name)?;
        if versions.1.contains(&version) {
            bail!(
                "{} {} is yanked; select a new version",
                package.name,
                version
            );
        }
        if versions.contains(&version) {
            outcomes.push(PublishOutcome {
                package: package.clone(),
                status: PublishStatus::AlreadyPublished,
            });
        } else {
            validate_new_version(package, &versions)?;
            pending.push(package.clone());
            outcomes.push(PublishOutcome {
                package: package.clone(),
                status: if upload {
                    PublishStatus::Published
                } else {
                    PublishStatus::DryRun
                },
            });
        }
        let local = workspace
            .package(&package.name)
            .context("unknown release package")?;
        for dependency in &local.dependencies {
            if dependency.kind.as_deref() == Some("dev") || !workspace.is_internal(dependency) {
                continue;
            }
            if selected.contains(dependency.name.as_str()) {
                continue;
            }
            let target = workspace
                .package(&dependency.name)
                .context("unknown internal dependency")?;
            let expected = semver::Version::parse(&target.version)?;
            let available = registry.versions(&target.name)?;
            if !available.contains(&expected) || available.1.contains(&expected) {
                bail!(
                    "{} needs unpublished or yanked {} {}; add its intentional version to the release set",
                    package.name,
                    target.name,
                    target.version
                );
            }
        }
    }
    if !pending.is_empty() {
        publisher.run(&pending, !upload)?;
        if upload {
            for package in &pending {
                let visible = registry.refresh(&package.name)?;
                let version = semver::Version::parse(&package.version)?;
                if !visible.contains(&version) || visible.1.contains(&version) {
                    bail!(
                        "{} {} upload returned but is not available in the registry API; retry the same reviewed plan",
                        package.name,
                        package.version
                    );
                }
            }
        }
    }
    Ok(outcomes)
}

/// Read the plan written by prepare; malformed and unknown fields fail closed.
pub fn read_manifest(path: &Path) -> Result<ReleaseManifest> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&content).context("failed to parse release-plan.json")
}

/// Reject stale versions, duplicate roots, publish=false members and reordered batch metadata.
pub fn validate_manifest(workspace: &Workspace, manifest: &ReleaseManifest) -> Result<()> {
    if manifest.format_version != 1 {
        bail!(
            "unsupported release plan format {}",
            manifest.format_version
        );
    }
    let names: Vec<_> = manifest
        .plan
        .changed
        .iter()
        .map(|p| p.name.clone())
        .collect();
    for name in &names {
        let package = workspace
            .package(name)
            .with_context(|| format!("unknown crate {name}"))?;
        if package.publish.as_ref().is_some_and(Vec::is_empty) {
            bail!("{name} has publish = false");
        }
    }
    if workspace.plan(&names, &names)? != manifest.plan {
        bail!("release-plan.json is stale; prepare again");
    }
    Ok(())
}

/// Per-package outcome emitted only after Cargo completes successfully.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PublishOutcome {
    pub package: CrateVersion,
    pub status: PublishStatus,
}
/// AlreadyPublished enables rerunning a partially successful batch without duplicate uploads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishStatus {
    Published,
    AlreadyPublished,
    DryRun,
}
