//! Data models used by the Forge release planner.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use semver::Version;
use serde::{Deserialize, Serialize};

/// The subset of cargo metadata needed by the planner.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub workspace_members: Vec<String>,
}

/// A workspace package and its declared dependencies.
#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub publish: Option<Vec<String>>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

/// A Cargo dependency declaration, including its dependency kind.
#[derive(Clone, Debug, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub req: String,
    pub kind: Option<String>,
    pub path: Option<PathBuf>,
}

/// The parsed workspace package graph.
#[derive(Debug)]
pub struct Workspace {
    pub(crate) packages: BTreeMap<String, Package>,
    pub(crate) all_dependents: BTreeMap<String, BTreeSet<String>>,
    pub(crate) runtime_or_build_dependents: BTreeMap<String, BTreeSet<String>>,
}

/// A package name and its current manifest version.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CrateVersion {
    pub name: String,
    pub version: String,
}

/// The release impact and dependency-first publishing order.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlan {
    pub changed: Vec<CrateVersion>,
    pub runtime_or_build_impact: Vec<CrateVersion>,
    pub dev_only_impact: Vec<CrateVersion>,
    pub publish_order: Vec<CrateVersion>,
}

/// The committed description that authorizes one release batch.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub format_version: u32,
    pub plan: ReleasePlan,
}

/// Registry versions for one package, sorted by SemVer precedence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryVersions(pub Vec<Version>, pub BTreeSet<Version>);

impl RegistryVersions {
    /// Return the highest published version, including yanked versions.
    pub fn latest(&self) -> Option<&Version> {
        self.0.iter().max()
    }

    /// Check whether the exact version already exists in the registry.
    pub fn contains(&self, version: &Version) -> bool {
        self.0.iter().any(|published| published == version)
    }
}

/// One consistency problem found by the release preflight.
#[derive(Debug)]
pub struct ReleaseIssue(pub String);

impl std::fmt::Display for ReleaseIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
