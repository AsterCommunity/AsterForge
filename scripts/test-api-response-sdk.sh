#!/usr/bin/env bash
set -euo pipefail

validation_root=$(mktemp -d "${TMPDIR:-/tmp}/asterforge-response-sdk.XXXXXX")
printf 'Response SDK validation artifacts: %s\n' "$validation_root"

ASTER_FORGE_RESPONSE_OPENAPI_OUT="$validation_root/openapi.json" \
    cargo test --locked -p aster_forge_api --features openapi generic_openapi_names_references_and_typed_codes_are_distinct
bunx --package 'typescript@npm:@typescript/typescript6@6.0.2' \
    --package 'openapi-typescript@7.13.0' openapi-typescript \
    "$validation_root/openapi.json" -o "$validation_root/api.generated.ts"
cp scripts/fixtures/api-response-sdk.ts "$validation_root/check.ts"
bunx --package 'typescript@npm:@typescript/typescript6@6.0.2' \
    tsc --ignoreConfig --strict --noEmit --skipLibCheck "$validation_root/check.ts"
