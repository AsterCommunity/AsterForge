//! Scoped, format-preserving dependency edits based on Cargo's package identities.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, value};

use crate::{
    checks::load_workspace,
    model::{Metadata, Workspace},
};

/// One dependency requirement updated for a manually versioned package.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PinUpdate {
    pub manifest: PathBuf,
    pub package: String,
    pub previous: String,
    pub current: String,
}

struct Document {
    path: PathBuf,
    before: String,
    edited: DocumentMut,
}

/// Loaded Cargo graph and the exact manifests eligible for a dependency-only edit.
pub struct ManifestWorkspace {
    root: PathBuf,
    documents: Vec<Document>,
    pub workspace: Workspace,
}

impl ManifestWorkspace {
    /// Metadata without dependency resolution supports a temporarily stale exact path pin.
    pub fn load(root: &Path) -> Result<Self> {
        let workspace = load_workspace(root)?;
        let mut paths = vec![root.join("Cargo.toml")];
        paths.extend(workspace.packages.values().map(|p| p.manifest_path.clone()));
        paths.sort();
        paths.dedup();
        let documents = paths
            .into_iter()
            .map(|path| {
                let before = fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                let edited = before
                    .parse()
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                Ok(Document {
                    path,
                    before,
                    edited,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            root: root.to_path_buf(),
            documents,
            workspace,
        })
    }

    /// Sync selected local dependency identities without changing package versions or features.
    pub fn update_pins(&mut self, names: &[String]) -> Result<Vec<PinUpdate>> {
        let targets = names
            .iter()
            .map(|name| {
                let package = self
                    .workspace
                    .package(name)
                    .with_context(|| format!("unknown crate {name}"))?;
                let directory = package
                    .manifest_path
                    .parent()
                    .context("manifest has no parent")?
                    .canonicalize()
                    .context("failed to resolve member directory")?;
                Ok((
                    directory,
                    (package.name.clone(), format!("={}", package.version)),
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut updates = Vec::new();
        for document in &mut self.documents {
            let base = document.path.parent().context("manifest has no parent")?;
            edit_sections(
                document.edited.as_table_mut(),
                base,
                &document.path,
                &targets,
                &mut updates,
            )?;
            if let Some(workspace) = document
                .edited
                .get_mut("workspace")
                .and_then(Item::as_table_like_mut)
            {
                edit_sections(workspace, base, &document.path, &targets, &mut updates)?;
            }
            if let Some(targets_table) = document
                .edited
                .get_mut("target")
                .and_then(Item::as_table_like_mut)
            {
                for (_, target) in targets_table.iter_mut() {
                    if let Some(table) = target.as_table_like_mut() {
                        edit_sections(table, base, &document.path, &targets, &mut updates)?;
                    }
                }
            }
        }
        // A previously unversioned dev edge becomes a publication prerequisite after
        // syncing. Keep dry-run planning identical to the graph Cargo will read on disk.
        let mut packages = self
            .workspace
            .packages
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for package in &mut packages {
            for dependency in &mut package.dependencies {
                if let Some((_, requirement)) =
                    dependency.path.as_ref().and_then(|path| targets.get(path))
                {
                    dependency.req.clone_from(requirement);
                }
            }
        }
        self.workspace = Workspace::from_metadata(Metadata {
            workspace_members: packages.iter().map(|package| package.id.clone()).collect(),
            packages,
        })?;
        Ok(updates)
    }

    /// Reload Cargo's actual graph after writes before creating a committed release plan.
    pub fn reload_workspace(&mut self) -> Result<()> {
        self.workspace = load_workspace(&self.root)?;
        Ok(())
    }

    /// Check all original contents before writing, and roll back our writes on an IO failure.
    pub fn write_manifests(&self) -> Result<()> {
        for doc in &self.documents {
            if fs::read_to_string(&doc.path)? != doc.before {
                bail!("{} changed during release preparation", doc.path.display());
            }
        }
        let mut written: Vec<&Document> = Vec::new();
        for doc in &self.documents {
            let replacement = doc.edited.to_string();
            if replacement == doc.before {
                continue;
            }
            if let Err(error) = replace_if_unchanged(&doc.path, &doc.before, &replacement) {
                for previous in written.into_iter().rev() {
                    replace_if_unchanged(
                        &previous.path,
                        &previous.edited.to_string(),
                        &previous.before,
                    )
                    .context("failed to roll back a manifest; inspect workspace changes")?;
                }
                return Err(error);
            }
            written.push(doc);
        }
        Ok(())
    }
}

fn edit_sections(
    table: &mut dyn toml_edit::TableLike,
    base: &Path,
    manifest: &Path,
    targets: &BTreeMap<PathBuf, (String, String)>,
    updates: &mut Vec<PinUpdate>,
) -> Result<()> {
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(dependencies) = table.get_mut(section).and_then(Item::as_table_like_mut) else {
            continue;
        };
        let keys: Vec<_> = dependencies.iter().map(|(key, _)| key.to_owned()).collect();
        for key in keys {
            let item = dependencies.get(&key).context("dependency disappeared")?;
            let Some(path) = item.get("path").and_then(Item::as_str) else {
                continue;
            };
            let directory = base
                .join(path)
                .canonicalize()
                .with_context(|| format!("invalid path dependency {key}"))?;
            let Some((name, version)) = targets.get(&directory) else {
                continue;
            };
            if let Some(mut update) = update_pin(dependencies, &key, version)? {
                update.package.clone_from(name);
                update.manifest = manifest.to_path_buf();
                updates.push(update);
            }
        }
    }
    Ok(())
}

/// Replace only the version value, retaining the declaration's features and comments.
pub fn update_pin(
    dependencies: &mut dyn toml_edit::TableLike,
    name: &str,
    current: &str,
) -> Result<Option<PinUpdate>> {
    let Some(item) = dependencies.get_mut(name) else {
        return Ok(None);
    };
    let table = item
        .as_table_like_mut()
        .with_context(|| format!("dependency {name} is not a table"))?;
    let previous = table
        .get("version")
        .and_then(Item::as_str)
        .unwrap_or_default()
        .to_owned();
    if previous == current {
        return Ok(None);
    }
    let mut replacement = value(current);
    if let (Some(old), Some(new)) = (
        table.get("version").and_then(Item::as_value),
        replacement.as_value_mut(),
    ) {
        *new.decor_mut() = old.decor().clone();
    }
    table.insert("version", replacement);
    Ok(Some(PinUpdate {
        manifest: PathBuf::new(),
        package: name.to_owned(),
        previous,
        current: current.to_owned(),
    }))
}

/// Replace a file through a unique sibling and refuse to overwrite a concurrent edit.
pub fn replace_if_unchanged(path: &Path, expected: &str, replacement: &str) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("refusing to replace a symlink manifest {}", path.display());
    }
    if fs::read_to_string(path)? != expected {
        bail!("{} changed during release preparation", path.display());
    }
    let parent = path.parent().context("file has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temporary.write_all(replacement.as_bytes())?;
    temporary
        .as_file()
        .set_permissions(fs::metadata(path)?.permissions())?;
    temporary.as_file().sync_all()?;
    if fs::read_to_string(path)? != expected {
        bail!("{} changed during release preparation", path.display());
    }
    temporary
        .persist(path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}
