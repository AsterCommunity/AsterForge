#!/usr/bin/env python3
"""Reject failed checks and unexpected skips of selected CI suites."""

import json
import os


SUITES = {
    "quality": "has_rust",
    "affected-crates": "has_generic",
    "panic-contract": "has_panic",
    "process-contract": "has_process",
    "auth-contract": "has_auth",
    "http-crypto-contract": "has_http_crypto",
    "cloud-files-contract": "has_cloud_files",
    "xml-fuzz": "has_xml",
    "template": "has_template",
}


def failures(needs: dict) -> list[str]:
    if needs.get("changes", {}).get("result") != "success":
        return ["Change detection did not succeed"]
    selected = needs["changes"].get("outputs", {})
    errors = []
    for job, flag in SUITES.items():
        if selected.get(flag) not in {"true", "false"}:
            errors.append(f"Missing or invalid selection: {flag}")
            continue
        allowed = {"success"} if selected[flag] == "true" else {"success", "skipped"}
        result = needs.get(job, {}).get("result")
        if result not in allowed:
            errors.append(f"{job}: {result} (selected={selected[flag]})")
    return errors


if __name__ == "__main__":
    problems = failures(json.loads(os.environ["NEEDS_JSON"]))
    for problem in problems:
        print(problem)
    raise SystemExit(bool(problems))
