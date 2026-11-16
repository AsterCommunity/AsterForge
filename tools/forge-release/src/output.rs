//! Human-readable output for release plans and consistency findings.

use crate::{
    model::{CrateVersion, ReleaseIssue, ReleasePlan},
    prepare::PrepareResult,
    publish::PublishOutcome,
};

/// Print a plan with validation impact separate from the selected release set.
pub fn print_plan(plan: &ReleasePlan) {
    print_crates("Changed", &plan.changed);
    print_crates(
        "Revalidate: normal/build dependents",
        &plan.runtime_or_build_impact,
    );
    print_crates("Revalidate: dev-only dependents", &plan.dev_only_impact);
    print_crates("Publish order", &plan.publish_order);
    println!("Impact lists are validation guidance; only Publish order is selected for release.");
}

/// Print all consistency issues without hiding subsequent findings.
pub fn print_release_issues(issues: &[ReleaseIssue]) {
    for issue in issues {
        eprintln!("- {issue}");
    }
}

/// Print preparation changes and the resulting release plan.
pub fn print_prepare(result: &PrepareResult) {
    print_crates("Release roots", &result.plan.changed);
    println!("Workspace pin updates:");
    if result.pin_updates.is_empty() {
        println!("  (none)");
    } else {
        for update in &result.pin_updates {
            println!(
                "  {}: {} -> {}",
                update.package, update.previous, update.current
            );
        }
    }
    print_plan(&result.plan);
}

/// Print idempotent publication outcomes.
pub fn print_publish(outcomes: &[PublishOutcome]) {
    for outcome in outcomes {
        println!(
            "{} {}: {}",
            outcome.package.name,
            outcome.package.version,
            match outcome.status {
                crate::publish::PublishStatus::Published => "published",
                crate::publish::PublishStatus::AlreadyPublished => "already published",
                crate::publish::PublishStatus::DryRun => "package checked",
            }
        );
    }
}

fn print_crates(label: &str, crates: &[CrateVersion]) {
    println!("{label}:");
    if crates.is_empty() {
        println!("  (none)");
    } else {
        for package in crates {
            println!("  {} {}", package.name, package.version);
        }
    }
}
