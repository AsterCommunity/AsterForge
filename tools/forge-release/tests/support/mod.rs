use forge_release::{
    model::{CrateVersion, Dependency, Metadata, Package, RegistryVersions, Workspace},
    publish::Publisher,
    registry::Registry,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub fn package(name: &str, dependencies: Vec<Dependency>) -> Package {
    Package {
        id: name.into(),
        name: name.into(),
        version: "1.0.0".into(),
        manifest_path: PathBuf::from(format!("/workspace/{name}/Cargo.toml")),
        publish: None,
        dependencies,
    }
}
pub fn dependency(name: &str, kind: Option<&str>) -> Dependency {
    Dependency {
        name: name.into(),
        req: "=1.0.0".into(),
        kind: kind.map(str::to_owned),
        path: Some(PathBuf::from(format!("/workspace/{name}"))),
    }
}
pub fn graph(packages: Vec<Package>) -> Workspace {
    Workspace::from_metadata(Metadata {
        workspace_members: packages.iter().map(|p| p.id.clone()).collect(),
        packages,
    })
    .unwrap()
}
pub fn versions(values: &[&str]) -> RegistryVersions {
    RegistryVersions(
        values.iter().map(|v| v.parse().unwrap()).collect(),
        BTreeSet::new(),
    )
}
#[derive(Default)]
pub struct FakeRegistry {
    pub versions: BTreeMap<String, RegistryVersions>,
    pub fail: bool,
    pub visible_after_publish: bool,
}
impl Registry for FakeRegistry {
    fn refresh(&self, name: &str) -> anyhow::Result<RegistryVersions> {
        if self.visible_after_publish {
            Ok(versions(&["1.0.0"]))
        } else {
            self.versions(name)
        }
    }
    fn versions(&self, name: &str) -> anyhow::Result<RegistryVersions> {
        anyhow::ensure!(!self.fail, "registry unavailable");
        Ok(self.versions.get(name).cloned().unwrap_or_default())
    }
}
#[derive(Default)]
pub struct FakePublisher {
    pub batches: Vec<(Vec<String>, bool)>,
    pub fail: bool,
}
impl Publisher for FakePublisher {
    fn run(&mut self, packages: &[CrateVersion], dry: bool) -> anyhow::Result<()> {
        self.batches
            .push((packages.iter().map(|p| p.name.clone()).collect(), dry));
        anyhow::ensure!(!self.fail, "simulated batch upload failure");
        Ok(())
    }
}
pub fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("Cargo.toml"), r#"# preserve root
[workspace]
members = ["zbase", "aconsumer"]
resolver = "3"
[workspace.dependencies]
base_alias = { package = "zbase", path = "zbase", version = "=1.0.0", default-features = false } # preserve pin
"#).unwrap();
    write_member(
        root,
        "zbase",
        r#"[package]
name = "zbase"
version = "1.0.1"
edition = "2024"
[features]
extra = []
"#,
    );
    write_member(
        root,
        "aconsumer",
        r#"[package]
name = "aconsumer"
version = "1.0.0"
edition = "2024"
[dependencies]
base_alias = { workspace = true, features = ["extra"] }
[target.'cfg(windows)'.build-dependencies]
base_alias = { package = "zbase", path = "../zbase", version = "=1.0.0" } # preserve target
"#,
    );
    directory
}
pub fn write_member(root: &Path, name: &str, text: &str) {
    std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    std::fs::write(root.join(name).join("Cargo.toml"), text).unwrap();
    std::fs::write(root.join(name).join("src/lib.rs"), "").unwrap();
}
