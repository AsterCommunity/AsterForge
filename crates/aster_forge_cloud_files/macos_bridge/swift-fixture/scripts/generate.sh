#!/bin/sh
set -eu

fixture_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

cmake -S "$fixture_root" -B "$fixture_root/build" -G Xcode "$@"
