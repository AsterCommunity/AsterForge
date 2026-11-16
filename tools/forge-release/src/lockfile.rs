//! CI lock validation permits only local version identity changes before locked resolution.
//!
//! Resolving a fresh lock first could conceal a missing third-party dependency in a PR.
//! Normalize existing local identities, then let Cargo --locked reject every other change.

use crate::{manifest::replace_if_unchanged, model::Workspace};
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Stdio},
};
use toml_edit::{DocumentMut, value};

/// Normalize existing path-package entries, then validate the complete committed dependency graph.
/// On failure the original lock is restored; no registry package is silently added.
pub fn validate_committed_lock(root: &Path, workspace: &Workspace) -> Result<()> {
    let path = root.join("Cargo.lock");
    let before = fs::read_to_string(&path).context("CI requires a committed Cargo.lock")?;
    let normalized = normalize_local_versions(&before, workspace)?;
    replace_if_unchanged(&path, &before, &normalized)?;
    let result = (|| {
        let status = Command::new("cargo")
            .args([
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--manifest-path",
            ])
            .arg(root.join("Cargo.toml"))
            .current_dir(root)
            .stdout(Stdio::null())
            .status()
            .context("failed to validate committed Cargo.lock")?;
        if !status.success() {
            bail!(
                "committed Cargo.lock is incomplete or requires changes beyond local versions; update and commit it explicitly"
            );
        }
        Ok(())
    })();
    if result.is_err() {
        replace_if_unchanged(&path, &normalized, &before)?;
    }
    result
}

/// Change package IDs only for existing local members and their disambiguated dependency references.
pub fn normalize_local_versions(text: &str, workspace: &Workspace) -> Result<String> {
    let mut document: DocumentMut = text.parse().context("invalid Cargo.lock")?;
    let packages = document
        .get_mut("package")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .context("Cargo.lock has no packages")?;
    let mut replacements = BTreeMap::new();
    for package in packages.iter_mut() {
        if package.contains_key("source") {
            continue;
        }
        let name = package
            .get("name")
            .and_then(toml_edit::Item::as_str)
            .context("lock entry has no name")?
            .to_owned();
        let Some(member) = workspace.package(&name) else {
            continue;
        };
        let previous = package
            .get("version")
            .and_then(toml_edit::Item::as_str)
            .context("lock entry has no version")?;
        if previous != member.version {
            replacements.insert(
                format!("{name} {previous}"),
                format!("{name} {}", member.version),
            );
            package.insert("version", value(member.version.clone()));
        }
    }
    for package in packages.iter_mut() {
        if let Some(dependencies) = package
            .get_mut("dependencies")
            .and_then(toml_edit::Item::as_array_mut)
        {
            for entry in dependencies.iter_mut() {
                if let Some(replacement) = entry.as_str().and_then(|entry| replacements.get(entry))
                {
                    let decor = entry.decor().clone();
                    *entry = toml_edit::Value::from(replacement.clone());
                    *entry.decor_mut() = decor;
                }
            }
        }
    }
    Ok(document.to_string())
}
