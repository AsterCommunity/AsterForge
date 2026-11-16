//! Dependency graph construction and release impact analysis.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Result, bail};

use crate::model::{CrateVersion, Metadata, ReleasePlan, Workspace};

impl Workspace {
    /// Look up a workspace package by its Cargo name.
    pub fn package(&self, name: &str) -> Option<&crate::model::Package> {
        self.packages.get(name)
    }

    /// Return the number of workspace packages included in the release graph.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Build reverse dependency indexes from cargo metadata output.
    pub fn from_metadata(metadata: Metadata) -> Result<Self> {
        let workspace_ids: BTreeSet<_> = metadata.workspace_members.into_iter().collect();
        let mut packages = BTreeMap::new();
        for package in metadata
            .packages
            .into_iter()
            .filter(|package| workspace_ids.contains(&package.id))
        {
            let name = package.name.clone();
            if packages.insert(name.clone(), package).is_some() {
                bail!("workspace contains duplicate package name {name}");
            }
        }

        if packages.len() != workspace_ids.len() || packages.is_empty() {
            bail!("metadata has missing workspace members or an empty workspace");
        }
        let mut all_dependents: BTreeMap<String, BTreeSet<String>> = packages
            .keys()
            .map(|name| (name.clone(), BTreeSet::new()))
            .collect();
        let mut runtime_or_build_dependents = all_dependents.clone();

        for package in packages.values() {
            for dependency in &package.dependencies {
                if !packages.get(&dependency.name).is_some_and(|target| {
                    dependency.path.as_deref() == target.manifest_path.parent()
                }) {
                    continue;
                }
                all_dependents
                    .entry(dependency.name.clone())
                    .or_default()
                    .insert(package.name.clone());
                if dependency.kind.as_deref() != Some("dev") {
                    runtime_or_build_dependents
                        .entry(dependency.name.clone())
                        .or_default()
                        .insert(package.name.clone());
                }
            }
        }

        Ok(Self {
            packages,
            all_dependents,
            runtime_or_build_dependents,
        })
    }

    /// Calculate impact and publish order without changing repository files.
    pub fn plan(&self, changed: &[String], release: &[String]) -> Result<ReleasePlan> {
        let changed = self.validate_names(changed, "changed")?;
        let release = if release.is_empty() {
            changed.clone()
        } else {
            self.validate_names(release, "release")?
        };
        let all_impact = self.reverse_closure(&changed, &self.all_dependents);
        let runtime_impact = self.reverse_closure(&changed, &self.runtime_or_build_dependents);
        let dev_only_impact = all_impact.difference(&runtime_impact).cloned().collect();
        let order = self.publish_order(&release)?;

        Ok(ReleasePlan {
            changed: self.versions(&changed),
            runtime_or_build_impact: self.versions(&runtime_impact),
            dev_only_impact: self.versions(&dev_only_impact),
            publish_order: order
                .iter()
                .map(|name| CrateVersion {
                    name: name.clone(),
                    version: self.packages[name].version.clone(),
                })
                .collect(),
        })
    }

    /// Return selected workspace packages in dependency-first publish order.
    pub fn publish_order(&self, selected: &BTreeSet<String>) -> Result<Vec<String>> {
        let selected =
            self.validate_names(&selected.iter().cloned().collect::<Vec<_>>(), "release")?;
        let mut pending = selected.clone();
        let mut ordered = Vec::with_capacity(selected.len());

        while !pending.is_empty() {
            let ready: Vec<_> = pending
                .iter()
                .filter(|name| {
                    self.packages[*name].dependencies.iter().all(|dependency| {
                        dependency.kind.as_deref() == Some("dev")
                            || !self.is_internal(dependency)
                            || !selected.contains(&dependency.name)
                            || !pending.contains(&dependency.name)
                    })
                })
                .cloned()
                .collect();
            if ready.is_empty() {
                bail!(
                    "internal dependency cycle among selected crates: {}",
                    pending.into_iter().collect::<Vec<_>>().join(", ")
                );
            }
            for name in ready {
                pending.remove(&name);
                ordered.push(name);
            }
        }
        Ok(ordered)
    }

    pub(crate) fn versions(&self, names: &BTreeSet<String>) -> Vec<CrateVersion> {
        names
            .iter()
            .filter_map(|name| self.packages.get(name))
            .map(|package| CrateVersion {
                name: package.name.clone(),
                version: package.version.clone(),
            })
            .collect()
    }

    fn reverse_closure(
        &self,
        roots: &BTreeSet<String>,
        graph: &BTreeMap<String, BTreeSet<String>>,
    ) -> BTreeSet<String> {
        let mut seen = roots.clone();
        let mut pending: VecDeque<_> = roots.iter().cloned().collect();
        while let Some(name) = pending.pop_front() {
            if let Some(dependents) = graph.get(&name) {
                for dependent in dependents {
                    if seen.insert(dependent.clone()) {
                        pending.push_back(dependent.clone());
                    }
                }
            }
        }
        seen
    }

    fn validate_names(&self, names: &[String], label: &str) -> Result<BTreeSet<String>> {
        if names.is_empty() {
            bail!("{label} crate list must not be empty");
        }
        let unique: BTreeSet<_> = names.iter().cloned().collect();
        let known: BTreeSet<_> = self.packages.keys().cloned().collect();
        let unknown: Vec<_> = unique.difference(&known).cloned().collect();
        if !unknown.is_empty() {
            bail!(
                "unknown {label} crate(s): {}; known workspace crates: {}",
                unknown.join(", "),
                known.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
        Ok(unique)
    }

    /// Determine local identity by path, rather than conflating registry crates of the same name.
    pub fn is_internal(&self, dependency: &crate::model::Dependency) -> bool {
        self.packages
            .get(&dependency.name)
            .is_some_and(|package| dependency.path.as_deref() == package.manifest_path.parent())
    }
}
