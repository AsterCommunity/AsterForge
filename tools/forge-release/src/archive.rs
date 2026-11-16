//! Compile the exact normalized package archives using only other reviewed batch archives as patches.
//!
//! Cargo 1.95's multi-package verifier can fail when the temporary registry lock lacks a checksum.
//! Package creation and archive compilation are separate so no new dependency must be uploaded first.

use crate::model::CrateVersion;
use anyhow::{Context, Result, bail};
use std::{fs, path::Path, process::Command};

/// Stage archives without verification, then compile their normalized contents in isolation.
pub fn verify_batch(root: &Path, packages: &[CrateVersion]) -> Result<()> {
    let staging = tempfile::tempdir()?;
    let target = staging.path().join("artifacts");
    let extracted = staging.path().join("archives");
    fs::create_dir(&extracted)?;
    let mut command = Command::new("cargo");
    command
        .args([
            "package",
            "--locked",
            "--no-verify",
            "--registry",
            "crates-io",
            "--target-dir",
        ])
        .arg(&target)
        .current_dir(root);
    for package in packages {
        command.args(["--package", &package.name]);
    }
    require_success(&mut command, "creating release archives")?;
    for package in packages {
        let archive = target
            .join("package")
            .join(format!("{}-{}.crate", package.name, package.version));
        let input =
            fs::File::open(&archive).with_context(|| format!("missing {}", archive.display()))?;
        // Archives originate from Cargo, but still use tar's traversal-safe extraction.
        tar::Archive::new(flate2::read::GzDecoder::new(input)).unpack(&extracted)?;
    }
    let patch_file = staging.path().join("patches.toml");
    let mut config = String::from("[patch.crates-io]\n");
    for package in packages {
        let path = extracted.join(format!("{}-{}", package.name, package.version));
        config.push_str(&format!(
            "{} = {{ path = {} }}\n",
            package.name,
            serde_json::to_string(&path)?
        ));
    }
    fs::write(&patch_file, config)?;
    for package in packages {
        let manifest = extracted
            .join(format!("{}-{}", package.name, package.version))
            .join("Cargo.toml");
        let mut command = Command::new("cargo");
        command
            .arg("check")
            .arg("--all-targets")
            .arg("--manifest-path")
            .arg(manifest)
            .arg("--config")
            .arg(&patch_file)
            .arg("--target-dir")
            .arg(staging.path().join("verified"))
            .current_dir(root);
        // These disposable archive lockfiles may change from registry identities to batch paths.
        require_success(
            &mut command,
            &format!("verifying archived {} {}", package.name, package.version),
        )?;
    }
    Ok(())
}

fn require_success(command: &mut Command, action: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed while {action}"))?;
    if !status.success() {
        bail!("failed while {action}: {status}");
    }
    Ok(())
}
