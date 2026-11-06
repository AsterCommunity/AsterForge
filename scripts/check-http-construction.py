#!/usr/bin/env python3
"""Reject raw reqwest constructors outside the Forge transport factory, including fixtures."""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALLOWED = "crates/aster_forge_http/src/builder.rs"
# Match full or aliased module paths, without confusing RemoteStorageClient/SDK builders.
CONSTRUCTOR = re.compile(r"\b(?:Client|ClientBuilder)\s*::\s*(?:builder|new|default)\s*\(")
IMPORT = re.compile(r"\buse\s+reqwest\b[^;]*\b(?:Client|ClientBuilder)\b", re.S)

def code_only(source):
    """Strip comments/string literals so examples and diagnostics do not become false positives."""
    pattern = r'//[^\n]*|/\*.*?\*/|r(\#*)".*?"\1|"(?:\\.|[^"\\])*"'
    return re.sub(pattern, lambda match: " " + "\n" * match.group().count("\n"), source, flags=re.S)

def violations(source):
    source = code_only(source)
    return [(source.count("\n",0,match.start())+1,match.group().strip()) for pattern in (CONSTRUCTOR,IMPORT) for match in pattern.finditer(source)]

def main():
    if "--self-test" in sys.argv:
        for source in ["reqwest::Client::new()", "reqwest :: blocking :: Client :: builder()", "use reqwest::{Client as Pool}; Pool::new()", "use reqwest::ClientBuilder; ClientBuilder::new()"]:
            assert violations(source), source
        for source in ['// reqwest::Client::new()', 'let message = "reqwest::Client::new()";', 'HttpClient::builder()', 'RemoteStorageClient::new()', 'ClientConfig::new()']:
            assert not violations(source), source
    count = 0
    for folder in ("src", "crates", "tests", "examples", "templates"):
        base = ROOT / folder
        if not base.exists():
            continue
        for path in sorted(base.rglob("*.rs")):
            if any(part in {"target", "node_modules", ".git"} for part in path.parts):
                continue
            relative = path.relative_to(ROOT).as_posix()
            if relative == ALLOWED:
                continue
            for line, expression in violations(path.read_text()):
                print(f"{relative}:{line}: use aster_forge_http factory: {expression}", file=sys.stderr)
                count += 1
    if count:
        return 1
    print("HTTP construction inventory: no raw constructors outside Forge factory (including tests).")
    return 0

if __name__ == "__main__":
    sys.exit(main())
