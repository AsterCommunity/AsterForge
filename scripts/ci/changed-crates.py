#!/usr/bin/env python3
"""Select workspace crates affected by a Git change range."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CLOUD_PACKAGES = {
    "aster_forge_cloud_files_core",
    "aster_forge_cloud_files_linux",
    "aster_forge_cloud_files_macos_bridge",
    "aster_forge_cloud_files_windows",
}
DEDICATED_PACKAGES = {
    "aster_forge_auth",
    "aster_forge_panic",
    "aster_forge_process",
} | CLOUD_PACKAGES
GLOBAL_PATHS = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
    "rustfmt.toml",
    ".rustfmt.toml",
    "clippy.toml",
    ".clippy.toml",
    ".cargo/config.toml",
    "scripts/check-http-construction.py",
    "scripts/check-lint-attributes.pl",
}
RUST_WORKFLOWS = {
    "cloud-files.yml",
    "template.yml",
    "rust.yml",
    "rust-auth.yml",
    "rust-crate.yml",
    "rust-full.yml",
    "rust-http-crypto.yml",
    "rust-panic.yml",
    "rust-process.yml",
    "rust-xml.yml",
}
SCRIPT_PACKAGES = {
    "scripts/test-api-response-sdk.sh": {"aster_forge_api"},
    "scripts/test-auth-features.sh": {"aster_forge_auth"},
    "scripts/test-http-crypto-features.py": {"aster_forge_http", "aster_forge_crypto"},
    "scripts/test-metrics-features.sh": {"aster_forge_metrics"},
    "scripts/test-middleware-features.sh": {"aster_forge_middleware"},
    "scripts/fixtures/api-response-sdk.ts": {"aster_forge_api"},
    "scripts/fixtures/auth-backend-consumer.rs": {"aster_forge_auth"},
    "scripts/fixtures/crypto-feature-consumer.rs": {"aster_forge_crypto"},
    "scripts/fixtures/http-feature-consumer.rs": {"aster_forge_http"},
}
NON_RUST_SCRIPTS = {
    "scripts/coverage-summary.mjs",
    "scripts/local-consumer-patch.py",
}


def affected_packages(changed_paths: list[str], metadata: dict) -> set[str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    workspace_ids = set(metadata["workspace_members"])
    package_dirs = {
        package_id: Path(package["manifest_path"]).parent.relative_to(ROOT).as_posix()
        for package_id, package in packages.items()
        if package_id in workspace_ids
    }

    if any(
        path in GLOBAL_PATHS
        or path.startswith(".cargo/")
        or (
            path.startswith(".github/workflows/")
            and Path(path).name in RUST_WORKFLOWS
        )
        or path.startswith("scripts/ci/")
        for path in changed_paths
    ):
        return {packages[package_id]["name"] for package_id in workspace_ids}

    unknown_scripts = {
        path
        for path in changed_paths
        if path.startswith("scripts/")
        and path not in SCRIPT_PACKAGES
        and path not in NON_RUST_SCRIPTS
    }
    if unknown_scripts:
        return {packages[package_id]["name"] for package_id in workspace_ids}

    # Removed/unknown crate directories must not silently select zero checks.
    if any(
        path.startswith("crates/")
        and not any(path.startswith(f"{directory}/") for directory in package_dirs.values())
        for path in changed_paths
    ):
        return {packages[package_id]["name"] for package_id in workspace_ids}

    directly_changed = {
        package_id
        for path in changed_paths
        for package_id, package_dir in package_dirs.items()
        if path == package_dir or path.startswith(f"{package_dir}/")
    }
    script_packages = {
        name for path in changed_paths for name in SCRIPT_PACKAGES.get(path, set())
    }
    directly_changed.update(
        package_id
        for package_id in workspace_ids
        if packages[package_id]["name"] in script_packages
    )
    if not directly_changed:
        return set()

    ids_by_name = {
        package["name"]: package_id
        for package_id, package in packages.items()
        if package_id in workspace_ids
    }
    reverse_dependencies = {package_id: set() for package_id in workspace_ids}
    for source_id in workspace_ids:
        for dependency in packages[source_id].get("dependencies", []):
            target_id = ids_by_name.get(dependency["name"])
            if target_id is not None:
                reverse_dependencies[target_id].add(source_id)

    affected = set(directly_changed)
    pending = list(directly_changed)
    while pending:
        changed_dependency = pending.pop()
        for dependent in reverse_dependencies[changed_dependency] - affected:
            affected.add(dependent)
            pending.append(dependent)

    return {packages[package_id]["name"] for package_id in affected}


def make_outputs(names: set[str], changed_paths: list[str], metadata: dict) -> dict[str, str]:
    generic = sorted(names - DEDICATED_PACKAGES)
    packages = {
        package["name"]: package for package in metadata["packages"]
    }
    template = tomllib.loads(
        re.sub(
            r"\{\{[^{}]+\}\}",
            "ci",
            (ROOT / "templates/aster-service-actix/Cargo.toml.liquid").read_text(),
        )
    )
    template_dependencies = {
        dependency.get("package", name)
        for name, dependency in template["workspace"]["dependencies"].items()
        if isinstance(dependency, dict) and "git" in dependency
    }
    pending = list(template_dependencies)
    while pending:
        package = packages.get(pending.pop())
        if package is None:
            continue
        for dependency in package.get("dependencies", []):
            name = dependency["name"]
            if name.startswith("aster_forge_") and name not in template_dependencies:
                template_dependencies.add(name)
                pending.append(name)
    suites = {
        "generic": bool(generic),
        "rust": bool(names),
        "panic": "aster_forge_panic" in names,
        "process": "aster_forge_process" in names,
        "auth": "aster_forge_auth" in names,
        "cloud_files": any(name.startswith("aster_forge_cloud_files_") for name in names),
        "http_crypto": bool(names & {"aster_forge_http", "aster_forge_crypto"}),
        "xml": "aster_forge_xml" in names,
        "template": bool(names & template_dependencies)
        or any(path.startswith("templates/") for path in changed_paths)
        or "scripts/local-consumer-patch.py" in changed_paths,
    }
    return {
        "cloud_packages": json.dumps(sorted(names & CLOUD_PACKAGES)),
        "generic_matrix": json.dumps(
            [{"package": name} for name in generic] or [{"package": "__none__"}]
        ),
        **{f"has_{suite}": str(enabled).lower() for suite, enabled in suites.items()},
    }


def self_test() -> None:
    sample_packages = [
        {
            "id": "core",
            "name": "core",
            "manifest_path": str(ROOT / "crates/core/Cargo.toml"),
            "dependencies": [],
        },
        {
            "id": "consumer",
            "name": "consumer",
            "manifest_path": str(ROOT / "crates/consumer/Cargo.toml"),
            "dependencies": [
                {"name": "core", "optional": True, "kind": "dev", "target": "cfg(windows)"}
            ],
        },
        {
            "id": "unrelated",
            "name": "unrelated",
            "manifest_path": str(ROOT / "crates/unrelated/Cargo.toml"),
            "dependencies": [],
        },
    ]
    metadata = {
        "packages": sample_packages,
        "workspace_members": [package["id"] for package in sample_packages],
    }
    assert affected_packages(["crates/core/src/lib.rs"], metadata) == {"core", "consumer"}
    assert affected_packages(["Cargo.lock"], metadata) == {"core", "consumer", "unrelated"}
    assert affected_packages(["docs/guide/index.md"], metadata) == set()
    outputs = make_outputs({"aster_forge_panic"}, [], metadata)
    assert outputs["has_panic"] == "true" and outputs["has_cloud_files"] == "false"
    print("changed-crates self-test passed")


def changed_paths(base: str, head: str) -> list[str]:
    # A new branch has no before commit: conservatively validate the workspace.
    if set(base) == {"0"}:
        return ["Cargo.lock"]
    # The caller supplies the merge base for PRs and the exact before SHA for pushes.
    # Disable rename detection so a move checks both the old and new owners.
    changed = subprocess.run(
        ["git", "diff", "--no-renames", "--name-only", "-z", base, head],
        cwd=ROOT,
        check=True,
        capture_output=True,
    ).stdout.decode().split("\0")
    return [path for path in changed if path]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--base")
    parser.add_argument("--head")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0
    if not args.base or not args.head:
        parser.error("--base and --head are required")

    paths = changed_paths(args.base, args.head)
    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    outputs = make_outputs(affected_packages(paths, metadata), paths, metadata)
    for key, value in outputs.items():
        print(f"{key}={value}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.CalledProcessError as error:
        print(error.stderr or error, file=sys.stderr)
        raise SystemExit(error.returncode)
