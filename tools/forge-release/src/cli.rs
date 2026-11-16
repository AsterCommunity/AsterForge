//! CLI dispatch keeps local preparation separate from registry-authorized publication.

use crate::{
    checks::{check_workspace, load_workspace, refresh_lockfile, repository_root},
    manifest::ManifestWorkspace,
    output::{print_plan, print_prepare, print_publish, print_release_issues},
    prepare::{discover, prepare, write_release_manifest},
    publish::{CargoPublisher, publish_release, read_manifest, validate_manifest},
    registry::CratesIoRegistry,
};
use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(about = "Prepare and publish explicit Forge crate versions")]
struct Cli {
    /// Repository root (defaults to the checkout containing this tool).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Debug, Subcommand)]
enum Command {
    /// Check internal dependency pins and local Cargo.lock entries.
    Check,
    /// Sync all local dependency pins and refresh Cargo.lock; never change package versions.
    Sync {
        /// Validate committed lock contents, allowing only existing local version identity edits.
        #[arg(long)]
        locked: bool,
    },
    /// Print validation impact and dependency-first publication order, including versioned dev dependencies.
    Plan {
        #[arg(long, required = true, num_args = 1..)]
        changed: Vec<String>,
        #[arg(long, num_args = 1..)]
        release: Vec<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Discover unpublished versions or select explicit roots, and prepare a release PR.
    #[command(group(ArgGroup::new("selection").required(true).args(["auto", "packages"])))]
    Prepare {
        /// Discover every publishable local version absent from crates.io.
        #[arg(long)]
        auto: bool,
        #[arg(long = "package", num_args = 1..)]
        packages: Vec<String>,
        /// Print changes without changing manifests, locks or release-plan.json.
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Verify a committed release plan without registry or upload credentials.
    Validate,
    /// Verify and publish exactly the committed release set; dry-run builds normalized archives.
    Publish {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
}
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum OutputFormat {
    #[default]
    Text,
    Json,
}

/// Execute one command. No subprocess receives secrets on its command line.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.map_or_else(repository_root, Ok)?.canonicalize()?;
    match cli.command {
        Command::Check => {
            let workspace = checked_workspace(&root)?;
            println!(
                "Cargo release metadata is consistent ({} crates).",
                workspace.package_count()
            );
        }
        Command::Sync { locked } => {
            let mut manifests = ManifestWorkspace::load(&root)?;
            let names = manifests
                .workspace
                .packages
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            manifests.update_pins(&names)?;
            manifests.write_manifests()?;
            if locked {
                crate::lockfile::validate_committed_lock(&root, &manifests.workspace)?;
            } else {
                refresh_lockfile(&root)?;
            }
        }
        Command::Plan {
            changed,
            release,
            format,
        } => {
            let workspace = load_workspace(&root)?;
            let plan = workspace.plan(&changed, &release)?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&plan)?),
                OutputFormat::Text => print_plan(&plan),
            }
        }
        Command::Prepare {
            auto,
            packages,
            dry_run,
            format,
        } => {
            let mut manifests = ManifestWorkspace::load(&root)?;
            let registry = CratesIoRegistry::new("https://crates.io/api/v1/crates")?;
            let names = if auto {
                discover(&manifests.workspace, &registry)?
            } else {
                packages
            };
            if names.is_empty() {
                match format {
                    OutputFormat::Text => println!("No unpublished crate versions."),
                    OutputFormat::Json => {
                        println!("{}", serde_json::json!({"pin_updates": [], "plan": null}))
                    }
                }
                return Ok(());
            }
            let result = prepare(&mut manifests, &names, &registry, dry_run)?;
            if !dry_run {
                refresh_lockfile(&root).context(
                    "dependency edits are present; fix Cargo resolution and rerun prepare",
                )?;
                write_release_manifest(&root.join("release-plan.json"), &result.plan)?;
                let description = format!(
                    "## Forge release\n\nOnly manually versioned crates are published. Downstream package versions stay unchanged.\n\n{}\n\nCI verifies all affected consumers. Security release scope is maintainer-owned.\n",
                    result
                        .plan
                        .publish_order
                        .iter()
                        .map(|p| format!("- {} {}", p.name, p.version))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                let generated = root.join("tools/forge-release/.generated");
                std::fs::create_dir_all(&generated)?;
                std::fs::write(generated.join("release-pr.md"), description)?;
            }
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                OutputFormat::Text => print_prepare(&result),
            }
        }
        Command::Validate => {
            let workspace = checked_workspace(&root)?;
            validate_manifest(&workspace, &read_manifest(&root.join("release-plan.json"))?)?;
        }
        Command::Publish { dry_run, format } => {
            let workspace = checked_workspace(&root)?;
            let manifest = read_manifest(&root.join("release-plan.json"))?;
            let registry = CratesIoRegistry::new("https://crates.io/api/v1/crates")?;
            let mut publisher = CargoPublisher::new(&root);
            let outcomes =
                publish_release(&workspace, &manifest, &registry, &mut publisher, !dry_run)?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&outcomes)?),
                OutputFormat::Text => print_publish(&outcomes),
            }
        }
    }
    Ok(())
}
fn checked_workspace(root: &std::path::Path) -> Result<crate::model::Workspace> {
    let workspace = load_workspace(root)?;
    let issues = check_workspace(&workspace, root)?;
    if !issues.is_empty() {
        print_release_issues(&issues);
        bail!("found {} release metadata issue(s)", issues.len());
    }
    Ok(workspace)
}
