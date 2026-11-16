//! Bounded, cached crates.io lookups; outages are errors rather than release intent.

use crate::model::RegistryVersions;
use anyhow::{Context, Result, bail};
use semver::Version;
use serde::Deserialize;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

/// Registry boundary shared by discovery, preparation and idempotent publication.
pub trait Registry {
    /// A missing crate is empty; every other HTTP, decoding or timeout failure propagates.
    fn versions(&self, package: &str) -> Result<RegistryVersions>;
    /// Bypass cached preflight state after an upload.
    fn refresh(&self, package: &str) -> Result<RegistryVersions> {
        self.versions(package)
    }
}

/// One process shares a connection pool and caches successful version lookups.
pub struct CratesIoRegistry {
    base_url: String,
    agent: ureq::Agent,
    cache: RefCell<BTreeMap<String, RegistryVersions>>,
}
#[derive(Deserialize)]
struct Response {
    versions: Vec<PublishedVersion>,
}
#[derive(Deserialize)]
struct PublishedVersion {
    num: String,
    yanked: bool,
}

impl CratesIoRegistry {
    /// HTTPS is required except for loopback HTTP test fixtures.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into();
        let uri: ureq::http::Uri = base_url.parse().context("invalid registry URI")?;
        if uri.scheme_str() != Some("https")
            && !(uri.scheme_str() == Some("http")
                && matches!(uri.host(), Some("127.0.0.1" | "localhost" | "[::1]")))
        {
            bail!("registry API requires HTTPS or a loopback HTTP fixture");
        }
        if uri.host().is_none() || uri.query().is_some() {
            bail!("invalid registry API base URL");
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent("AsterForge forge-release (https://github.com/AsterCommunity/AsterForge)")
            .max_redirects(0)
            .build()
            .into();
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            agent,
            cache: RefCell::new(BTreeMap::new()),
        })
    }
}
impl Registry for CratesIoRegistry {
    fn refresh(&self, package: &str) -> Result<RegistryVersions> {
        self.cache.borrow_mut().remove(package);
        self.versions(package)
    }

    fn versions(&self, package: &str) -> Result<RegistryVersions> {
        if package.is_empty()
            || !package
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            bail!("invalid registry package name");
        }
        if let Some(cached) = self.cache.borrow().get(package) {
            return Ok(cached.clone());
        }
        let url = format!("{}/{}", self.base_url, package);
        let mut response = match self.agent.get(&url).call() {
            Ok(response) if response.status() == 200 => response,
            Ok(response) => bail!(
                "unexpected registry status {} for {package}",
                response.status()
            ),
            Err(ureq::Error::StatusCode(404)) => {
                let empty = RegistryVersions::default();
                self.cache
                    .borrow_mut()
                    .insert(package.to_owned(), empty.clone());
                return Ok(empty);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("registry lookup failed for {package}; no release intent inferred")
                });
            }
        };
        let payload: Response = response
            .body_mut()
            .with_config()
            .limit(8 * 1024 * 1024)
            .read_json()
            .with_context(|| format!("invalid registry response for {package}"))?;
        if payload.versions.is_empty() {
            bail!("registry returned no versions for an existing crate {package}");
        }
        let mut versions = Vec::new();
        let mut yanked = BTreeSet::new();
        for entry in payload.versions {
            let version =
                Version::parse(&entry.num).context("registry returned an invalid SemVer")?;
            if entry.yanked {
                yanked.insert(version.clone());
            }
            versions.push(version);
        }
        let versions = RegistryVersions(versions, yanked);
        self.cache
            .borrow_mut()
            .insert(package.to_owned(), versions.clone());
        Ok(versions)
    }
}
