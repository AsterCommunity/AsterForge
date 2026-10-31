#!/usr/bin/env python3
"""Print a Cargo config that patches Forge Git dependencies to this checkout.

Usage from a consumer: python3 ../AsterForge/scripts/local-consumer-patch.py > /tmp/forge-local.toml
Then append --config /tmp/forge-local.toml to Cargo commands. Do not commit a
consumer lockfile resolved with this patch as its published dependency pin.
"""

import json
from pathlib import Path
import tomllib


def main():
    root = Path(__file__).resolve().parents[1]
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    print('[patch."https://github.com/AsterCommunity/AsterForge"]')
    for member in workspace["workspace"]["members"]:
        for directory in sorted(root.glob(member)):
            manifest = tomllib.loads((directory / "Cargo.toml").read_text())
            name = manifest["package"]["name"]
            if name.startswith("aster_forge_"):
                print(f"{name} = {{ path = {json.dumps(str(directory), ensure_ascii=False)} }}")


if __name__ == "__main__":
    main()
