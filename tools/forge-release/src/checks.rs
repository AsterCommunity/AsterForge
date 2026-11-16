//! Repository loading and release metadata consistency checks.

use std::{
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::model::{Metadata, ReleaseIssue, Workspace};

#[derive(Debug, Deserialize)]
struct Lockfile {
    #[serde(default)]
    package: Vec<LockPackage>,
}

#[derive(Debug, Deserialize)]
struct LockPackage {
    name: String,
    version: String,
    source: Option<String>,
}

/// Locate the repository containing tools/forge-release.
pub fn repository_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("forge-release must live under tools/forge-release")
}

/// Load workspace package metadata through Cargo's own resolver.
pub fn load_workspace(root: &Path) -> Result<Workspace> {
    let root = root
        .canonicalize()
        .context("failed to resolve workspace root")?;
    let output = ProcessCommand::new("cargo")
        .args([
            "metadata",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .current_dir(&root)
        .output()
        .context("failed to run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("failed to parse cargo metadata output")?;
    Workspace::from_metadata(metadata).context("failed to build workspace dependency graph")
}

/// Check exact internal dependency pins and workspace package lock entries.
pub fn check_workspace(workspace: &Workspace, root: &Path) -> Result<Vec<ReleaseIssue>> {
    let root_manifest_path = root.join("Cargo.toml");
    let root_manifest_text = std::fs::read_to_string(&root_manifest_path)
        .with_context(|| format!("failed to read {}", root_manifest_path.display()))?;
    let root_manifest: toml::Value =
        toml::from_str(&root_manifest_text).context("failed to parse root Cargo.toml")?;
    let lock_path = root.join("Cargo.lock");
    let lock_text = std::fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read {}", lock_path.display()))?;
    let lockfile: Lockfile = toml::from_str(&lock_text).context("failed to parse Cargo.lock")?;
    let mut issues = Vec::new();

    let workspace_dependencies = root_manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table)
        .context("root Cargo.toml is missing [workspace.dependencies]")?;

    // Inspect unused and renamed root declarations too; metadata only contains consumed edges.
    for (key, dependency) in workspace_dependencies {
        let Some(path) = dependency.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        let directory = root
            .join(path)
            .canonicalize()
            .context("invalid workspace dependency path")?;
        let Some(package) = workspace.packages.values().find(|package| {
            package
                .manifest_path
                .parent()
                .is_some_and(|parent| parent == directory)
        }) else {
            continue;
        };
        let expected = format!("={}", package.version);
        let actual = dependency
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        if actual != expected {
            issues.push(ReleaseIssue(format!(
                "workspace dependency {key} has {actual}, expected {expected}"
            )));
        }
    }
    for package in workspace.packages.values() {
        let lock_has_current = lockfile.package.iter().any(|locked| {
            locked.source.is_none()
                && locked.name == package.name
                && locked.version == package.version
        });
        if !lock_has_current {
            issues.push(ReleaseIssue(format!(
                "{} {} is missing from Cargo.lock",
                package.name, package.version
            )));
        }

        for dependency in &package.dependencies {
            if !workspace.is_internal(dependency) {
                continue;
            }
            let Some(internal) = workspace.packages.get(&dependency.name) else {
                continue;
            };
            let expected = format!("={}", internal.version);
            if dependency.req != expected {
                issues.push(ReleaseIssue(format!(
                    "{} depends on {} with {}, expected {}",
                    package.name, dependency.name, dependency.req, expected
                )));
            }
        }
    }
    Ok(issues)
}

/// Let Cargo refresh path-package entries after a version pin update.
pub fn refresh_lockfile(root: &Path) -> Result<()> {
    let status = ProcessCommand::new("cargo")
        .args(["update", "--workspace", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .current_dir(root)
        .status()
        .context("failed to refresh Cargo.lock")?;
    if !status.success() {
        bail!("cargo update --workspace failed with {status}");
    }
    Ok(())
}
