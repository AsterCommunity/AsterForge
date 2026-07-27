#!/bin/sh
set -eu

fixture_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
signing_file="$fixture_root/LocalSigning.cmake"

if [ ! -f "$signing_file" ]; then
  printf '%s\n' "Missing $signing_file; start from LocalSigning.cmake.example." >&2
  exit 2
fi

identity_count=$(
  security find-identity -v -p codesigning 2>/dev/null \
    | awk '/valid identities found/ { print $1 }'
)
if [ "${identity_count:-0}" -eq 0 ]; then
  printf '%s\n' "No valid Apple code-signing identity is available in the current keychain." >&2
  exit 2
fi

"$fixture_root/scripts/build.sh" "$@"

app="$fixture_root/build/Debug/AsterForgeCloudFilesFixtureHost.app"
executable="$app/Contents/MacOS/AsterForgeCloudFilesFixtureHost"
extension="$app/Contents/PlugIns/AsterForgeCloudFilesFixtureExtension.appex"

if [ ! -x "$executable" ] || [ ! -d "$extension" ]; then
  printf '%s\n' "Signed fixture products are missing from $app." >&2
  exit 2
fi

codesign --verify --strict --verbose=2 "$extension"
codesign --verify --strict --verbose=2 "$app"
host_team=$(codesign -dv "$app" 2>&1 | awk -F= '/^TeamIdentifier=/ { print $2 }')
extension_team=$(codesign -dv "$extension" 2>&1 | awk -F= '/^TeamIdentifier=/ { print $2 }')
if [ -z "$host_team" ] || [ "$host_team" != "$extension_team" ]; then
  printf '%s\n' "Host and extension code-signing Team IDs do not match." >&2
  exit 2
fi
pluginkit -a "$extension"

cleanup() {
  "$executable" --system-test cleanup >/dev/null 2>&1 || true
  pluginkit -r "$extension" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

probe_report=$("$executable" --system-test probe)
printf '%s\n' "$probe_report"

set +e
baseline_report=$("$executable" --system-test baseline)
baseline_status=$?
set -e
printf '%s\n' "$baseline_report"
if [ "$baseline_status" -ne 0 ]; then
  exit "$baseline_status"
fi
root_url=$(printf '%s' "$baseline_report" | plutil -extract rootURL raw -o - -- -)

extension_executable="$extension/Contents/MacOS/AsterForgeCloudFilesFixtureExtension"
if pkill -f "$extension_executable" 2>/dev/null; then
  printf '%s\n' "Terminated the live File Provider extension process."
else
  printf '%s\n' "The system had already reclaimed the File Provider extension process."
fi
sleep 1

expected='AsterForge in-memory File Provider fixture.'
actual=$(/bin/cat "$root_url/README.txt")
if [ "$actual" != "$expected" ]; then
  printf '%s\n' "Hostless README.txt hydration returned unexpected bytes." >&2
  exit 1
fi
printf '%s\n' "Hostless README.txt hydration passed."

set +e
recovery_report=$("$executable" --system-test recovery)
recovery_status=$?
set -e
printf '%s\n' "$recovery_report"
if [ "$recovery_status" -ne 0 ]; then
  exit "$recovery_status"
fi

trap - EXIT INT TERM
pluginkit -r "$extension" >/dev/null 2>&1 || true
printf '%s\n' "Signed File Provider system test passed."
