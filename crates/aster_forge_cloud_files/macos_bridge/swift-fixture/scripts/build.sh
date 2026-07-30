#!/bin/sh
set -eu

fixture_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

"$fixture_root/scripts/generate.sh" "$@"
cmake --build "$fixture_root/build" --config Debug --target AsterForgeCloudFilesFixtureHost
