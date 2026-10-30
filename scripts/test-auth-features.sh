#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
consumer=$(mktemp -d)
trap 'rm -rf "$consumer"' EXIT
mkdir -p "$consumer/src"
cp "$repo_root/scripts/fixtures/auth-backend-consumer.rs" "$consumer/src/main.rs"
cat > "$consumer/Cargo.toml" <<EOF
[workspace]
[package]
name = "auth-backend-consumer"
version = "0.0.0"
edition = "2024"
[features]
aws-lc-rs = ["aster_forge_auth/aws-lc-rs"]
rust-crypto = ["aster_forge_auth/rust-crypto"]
merge = ["jsonwebtoken/aws_lc_rs", "jsonwebtoken/rust_crypto"]
[dependencies]
aster_forge_auth = { path = "$repo_root/crates/aster_forge_auth", default-features = false }
jsonwebtoken = { version = "11", default-features = false }
EOF

# Reuse artifacts while keeping resolution isolated from workspace feature unification.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/target}"
cargo generate-lockfile --manifest-path "$consumer/Cargo.toml"

expect_failure() {
    local features=$1 diagnostic=$2
    if cargo check --locked --manifest-path "$consumer/Cargo.toml" --features "$features" > "$consumer/check.log" 2>&1; then
        printf 'Invalid auth features unexpectedly compiled: %s\n' "${features:-none}" >&2
        exit 1
    fi
    if ! grep -Fq -- "$diagnostic" "$consumer/check.log"; then
        cat "$consumer/check.log" >&2
        exit 1
    fi
    printf 'Rejected invalid auth features: %s\n' "${features:-none}"
}

expect_failure '' 'select exactly one JWT backend feature'
expect_failure 'aws-lc-rs,rust-crypto' 'are mutually exclusive'

for backend in aws-lc-rs rust-crypto; do
    cargo run --locked --manifest-path "$consumer/Cargo.toml" --features "$backend" -- missing
    cargo run --locked --manifest-path "$consumer/Cargo.toml" --features "$backend"
    cargo tree --locked --manifest-path "$consumer/Cargo.toml" --features "$backend" -e features > "$consumer/$backend.tree"
    if test "$backend" = rust-crypto; then
        if grep -Eq -- 'aws-lc-(rs|sys)|feature "(use_pem|aws_lc_rs)"|simple_asn1|pem v' "$consumer/$backend.tree"; then
            cat "$consumer/$backend.tree" >&2
            printf 'RustCrypto JWT consumer pulled in AWS-LC or PEM dependencies\n' >&2
            exit 1
        fi
    fi
    # The other backend can be enabled directly on jsonwebtoken without enabling a second Forge
    # feature. Explicit startup must still select Forge's configured provider, or reject conflict.
    cargo run --locked --manifest-path "$consumer/Cargo.toml" --features "$backend,merge"
    cargo run --locked --manifest-path "$consumer/Cargo.toml" --features "$backend,merge" -- conflict
    cargo run --locked --manifest-path "$consumer/Cargo.toml" --features "$backend,merge" -- missing
done
printf 'Auth backend compile diagnostics, initialization and dependency isolation passed\n'
