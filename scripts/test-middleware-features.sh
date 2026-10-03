#!/usr/bin/env bash
set -euo pipefail

for features in '' actix axum actix,axum actix,metrics axum,metrics actix,axum,metrics; do
    printf 'Middleware features: %s\n' "${features:-none}"
    cargo test --locked -p aster_forge_middleware --no-default-features --features "$features" --all-targets
done

dependency_tree=$(cargo tree --locked -p aster_forge_middleware --no-default-features --features axum,metrics -e normal)
if rg -q 'actix-(web|governor)' <<< "$dependency_tree"; then
    printf 'Axum-only middleware must not depend on Actix\n' >&2
    exit 1
fi
printf 'Axum-only dependency isolation passed\n'
