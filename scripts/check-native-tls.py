#!/usr/bin/env python3
"""Keep the default Forge workspace dependency graph on rustls."""

from __future__ import annotations

import subprocess
import sys


def main() -> int:
    result = subprocess.run(
        ["cargo", "tree", "--workspace", "--all-targets", "--locked", "-e", "normal"],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if result.returncode:
        print(result.stdout, end="")
        return result.returncode
    native = [line for line in result.stdout.splitlines() if "native-tls" in line]
    if native:
        print("default workspace graph unexpectedly enables native-tls:", file=sys.stderr)
        print("\n".join(native), file=sys.stderr)
        return 1
    print("PASS default workspace graph: native-tls absent; rustls remains the default")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
