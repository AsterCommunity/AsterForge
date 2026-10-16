#!/usr/bin/env bash
set -euo pipefail

for features in '' backend-prometheus otlp backend-prometheus,otlp backend-prometheus,otlp,runtime-health,allocator-metrics; do
    printf 'Metrics features: %s\n' "${features:-none}"
    cargo test --locked -p aster_forge_metrics --no-default-features --features "$features" --lib
done
for features in '' actix axum actix,axum actix,prometheus axum,prometheus actix,axum,prometheus; do
    printf 'Endpoint features: %s\n' "${features:-none}"
    cargo test --locked -p aster_forge_observability --no-default-features --features "$features" --all-targets
done
metrics_tree=$(cargo tree --locked -p aster_forge_metrics --no-default-features --features otlp -e normal)
if rg -q '(prometheus|opentelemetry-prometheus) v' <<< "$metrics_tree"; then
    printf 'OTLP-only metrics must not compile a Prometheus exporter\n' >&2
    exit 1
fi
endpoint_tree=$(cargo tree --locked -p aster_forge_observability --no-default-features --features axum,prometheus -e normal)
if rg -q 'actix-web v' <<< "$endpoint_tree"; then
    printf 'Axum-only exposition must not compile Actix\n' >&2
    exit 1
fi
