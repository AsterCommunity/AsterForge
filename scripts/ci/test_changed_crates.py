"""Regression tests for CI selection; no Rust compilation or dependency resolution."""

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "changed_crates", Path(__file__).with_name("changed-crates.py")
)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


def package(name, dependencies=()):
    return {
        "id": name,
        "name": name,
        "manifest_path": str(ci.ROOT / "crates" / name / "Cargo.toml"),
        "dependencies": [
            {"name": dependency, "optional": True, "kind": "dev", "target": "cfg(windows)"}
            for dependency in dependencies
        ],
    }


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.packages = [
            package("foundation"),
            package("consumer", ["foundation"]),
            package("indirect", ["consumer"]),
            package("unrelated"),
        ]
        self.metadata = {
            "packages": self.packages,
            "workspace_members": [item["id"] for item in self.packages],
        }

    def test_optional_target_dev_dependencies_are_transitive(self):
        self.assertEqual(
            ci.affected_packages(["crates/foundation/src/lib.rs"], self.metadata),
            {"foundation", "consumer", "indirect"},
        )

    def test_dependency_cycles_terminate(self):
        self.packages[0]["dependencies"] = [{"name": "indirect"}]
        self.assertEqual(
            ci.affected_packages(["crates/foundation/src/lib.rs"], self.metadata),
            {"foundation", "consumer", "indirect"},
        )

    def test_shared_inputs_select_all_crates(self):
        for path in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml",
                     "scripts/ci/changed-crates.py", "scripts/fixtures/consumer.rs",
                     "scripts/check-lint-attributes.pl",
                     ".github/workflows/rust-crate.yml"]:
            with self.subTest(path=path):
                self.assertEqual(
                    ci.affected_packages([path], self.metadata),
                    set(self.metadata["workspace_members"]),
                )

    def test_documentation_workflow_does_not_select_crates(self):
        self.assertEqual(
            ci.affected_packages([".github/workflows/vitepress.yml"], self.metadata),
            set(),
        )

    def test_script_changes_select_their_contract(self):
        names = set().union(*ci.SCRIPT_PACKAGES.values())
        metadata = {
            "packages": [
                {
                    "id": name,
                    "name": name,
                    "manifest_path": str(ci.ROOT / "crates" / name / "Cargo.toml"),
                    "dependencies": [],
                }
                for name in sorted(names)
            ],
            "workspace_members": sorted(names),
        }
        scenarios = {
            "scripts/test-api-response-sdk.sh": {"aster_forge_api"},
            "scripts/test-auth-features.sh": {"aster_forge_auth"},
            "scripts/test-http-crypto-features.py": {"aster_forge_http", "aster_forge_crypto"},
            "scripts/test-metrics-features.sh": {"aster_forge_metrics"},
            "scripts/test-middleware-features.sh": {"aster_forge_middleware"},
        }
        for path, expected in scenarios.items():
            with self.subTest(path=path):
                self.assertEqual(ci.affected_packages([path], metadata), expected)
        outputs = ci.make_outputs(set(), ["scripts/local-consumer-patch.py"], metadata)
        self.assertEqual(outputs["has_template"], "true")
        self.assertEqual(ci.affected_packages(["scripts/coverage-summary.mjs"], metadata), set())

    def test_unknown_script_falls_back_to_all_crates(self):
        self.assertEqual(
            ci.affected_packages(["scripts/new-check.py"], self.metadata),
            set(self.metadata["workspace_members"]),
        )

    def test_documentation_does_not_select_rust(self):
        names = ci.affected_packages(["docs/guide/index.md", "README.md"], self.metadata)
        outputs = ci.make_outputs(names, ["docs/guide/index.md"], self.metadata)
        self.assertEqual(names, set())
        self.assertEqual(outputs["has_rust"], "false")
        self.assertEqual(outputs["has_template"], "false")

    def test_removed_crate_falls_back_to_all(self):
        self.assertEqual(
            ci.affected_packages(["crates/removed/src/lib.rs"], self.metadata),
            set(self.metadata["workspace_members"]),
        )

    def test_directory_prefix_does_not_match_another_crate(self):
        self.assertEqual(
            ci.affected_packages(["crates/consumer/src/lib.rs"], self.metadata),
            {"consumer", "indirect"},
        )

    def test_panic_does_not_select_unrelated_suites(self):
        outputs = ci.make_outputs({"aster_forge_panic"}, [], self.metadata)
        for suite in ["cloud_files", "process", "auth", "http_crypto", "generic", "xml"]:
            self.assertEqual(outputs[f"has_{suite}"], "false")
        self.assertEqual(outputs["has_panic"], "true")
        self.assertEqual(outputs["has_template"], "true")

    def test_dedicated_crates_are_not_duplicated_in_generic_matrix(self):
        outputs = ci.make_outputs(
            {"aster_forge_cloud_files_linux", "aster_forge_utils"}, [], self.metadata
        )
        self.assertEqual(outputs["has_cloud_files"], "true")
        self.assertEqual(json.loads(outputs["generic_matrix"]), [{"package": "aster_forge_utils"}])
        self.assertEqual(json.loads(outputs["cloud_packages"]), ["aster_forge_cloud_files_linux"])

    def test_template_only_does_not_select_rust_crates(self):
        outputs = ci.make_outputs(set(), ["templates/aster-service-actix/build.rs"], self.metadata)
        self.assertEqual(outputs["has_template"], "true")
        self.assertEqual(outputs["has_rust"], "false")

    def test_transitive_template_dependency_selects_generated_service(self):
        self.packages[0]["name"] = "aster_forge_metrics"
        self.packages[0]["dependencies"] = [{"name": "aster_forge_http"}]
        self.packages[1]["name"] = "aster_forge_http"
        self.packages[1]["dependencies"] = []
        self.packages[2]["name"] = "unrelated"
        self.metadata["workspace_members"] = [item["id"] for item in self.packages]
        outputs = ci.make_outputs({"aster_forge_http"}, [], self.metadata)
        self.assertEqual(outputs["has_template"], "true")

    def test_initial_push_selects_everything(self):
        self.assertEqual(ci.changed_paths("0" * 40, "HEAD"), ["Cargo.lock"])


class GitDiffTests(unittest.TestCase):
    def test_rename_checks_both_owners(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return subprocess.run(
                    ["git", *args], cwd=root, check=True, capture_output=True, text=True
                ).stdout.strip()

            git("init")
            git("config", "user.name", "CI selector test")
            git("config", "user.email", "ci@example.invalid")
            (root / "old").write_text("shared fixture\n")
            git("add", "old")
            git("-c", "commit.gpgsign=false", "commit", "-m", "before")
            base = git("rev-parse", "HEAD")
            (root / "old").rename(root / "new")
            git("add", "-A")
            git("-c", "commit.gpgsign=false", "commit", "-m", "after")
            with patch.object(ci, "ROOT", root):
                self.assertEqual(set(ci.changed_paths(base, "HEAD")), {"old", "new"})
