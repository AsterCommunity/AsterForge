#!/usr/bin/env python3
"""Compile real isolated consumers; reject accidental feature/dependency leakage."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
HTTP = ["json", "form", "query", "multipart", "stream", "blocking", "http2", "charset", "system-proxy", "rustls", "rustls-aws-lc-rs", "rustls-ring", "native-tls", "native-tls-vendored"]
CRYPTO = ["password", "sha256", "hmac-sha256", "secret-envelope"]


def cargo(path, args, features=(), failure=None):
    cmd = ["cargo", *args, "--manifest-path", str(path / "Cargo.toml")]
    if os.environ.get("FORGE_FEATURE_TEST_OFFLINE"):
        cmd += ["--offline"]
    if features:
        cmd += ["--features", ",".join(features)]
    result = subprocess.run(cmd, env=ENV, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if failure:
        assert result.returncode and failure in result.stderr, result.stdout + result.stderr
    elif result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return result.stdout


def fixture(path, kind, features):
    path.mkdir()
    (path / "src").mkdir()
    crate = f"aster_forge_{kind}"
    manifest = f'''[workspace]
[package]
name = "{kind}-feature-consumer"
version = "0.0.0"
edition = "2024"
[features]
'''
    for feature in features:
        forwards = [f"{crate}/{feature}"]
        if feature == "stream":
            forwards.append("dep:futures-util")
        if feature in ("rustls", "rustls-ring"):
            forwards.append("dep:rustls")
        manifest += f'{feature} = {json.dumps(forwards)}\n'
    manifest += f'''[dependencies]
{crate} = {{ path = {json.dumps(str(ROOT / "crates" / crate))}, default-features = false }}
'''
    if kind == "http":
        manifest += '''reqwest = { version = "0.13", default-features = false }
rustls = { version = "0.23", default-features = false, features = ["std", "ring"], optional = true }
futures-util = { version = "0.3", default-features = false, features = ["std"], optional = true }
'''
    (path / "Cargo.toml").write_text(manifest)
    source = (ROOT / "scripts" / "fixtures" / f"{kind}-feature-consumer.rs").read_text()
    (path / "src/main.rs").write_text(source)
    return source


def resolved(path, features):
    metadata = json.loads(cargo(path, ["metadata", "--format-version", "1"], features))
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    return {p["name"]: set(nodes[p["id"]]["features"]) for p in metadata["packages"] if p["id"] in nodes}


def negative(path, source, expression, diagnostic):
    (path / "src/main.rs").write_text(f"fn main() {{ {expression}; }}\n")
    cargo(path, ["check"], failure=diagnostic)
    (path / "src/main.rs").write_text(source)


def main():
    with tempfile.TemporaryDirectory(prefix="forge-feature-consumers-") as directory:
        base = Path(directory)
        http, crypto = base / "http", base / "crypto"
        http_source = fixture(http, "http", HTTP)
        crypto_source = fixture(crypto, "crypto", CRYPTO)
        for path, features in [(http, HTTP), (crypto, CRYPTO)]:
            for selected in [[], *[[f] for f in features], features]:
                cargo(path, ["run"], selected)
                print(f"PASS {path.name}: {','.join(selected) or 'none'}", flush=True)
        for mask in range(1 << len(CRYPTO)):
            selected = [feature for index, feature in enumerate(CRYPTO) if mask & (1 << index)]
            cargo(crypto, ["run"], selected)
        print(f"PASS crypto: all {1 << len(CRYPTO)} feature combinations", flush=True)
        for selected in [
            ["json", "form"],
            ["form", "query"],
            ["multipart", "stream"],
            ["blocking", "rustls-aws-lc-rs"],
            ["http2", "rustls-aws-lc-rs"],
            ["charset", "json"],
            ["system-proxy", "rustls"],
            ["rustls-aws-lc-rs", "native-tls"],
            ["rustls-ring", "native-tls"],
        ]:
            cargo(http, ["run"], selected)
        print("PASS http: 9 cross-feature interactions", flush=True)
        minimal = resolved(http, ["json", "rustls-aws-lc-rs"])
        forbidden = {"form", "query", "multipart", "stream", "charset", "http2", "system-proxy", "default-tls", "default", "native-tls"}
        assert not minimal["reqwest"] & forbidden, minimal["reqwest"]
        assert not {"serde_urlencoded", "mime_guess", "encoding_rs", "native-tls", "openssl", "futures-executor"} & minimal.keys(), minimal.keys()
        ring = resolved(http, ["json", "rustls-ring"])
        assert "ring" in ring and not {"aws-lc-rs", "aws-lc-sys", "native-tls", "openssl"} & ring.keys()
        native = resolved(http, ["native-tls"])
        assert not {"aws-lc-rs", "aws-lc-sys", "ring", "rustls"} & native.keys()
        vendored = resolved(http, ["native-tls-vendored"])
        assert {"openssl", "openssl-sys", "openssl-src"} <= vendored.keys(), vendored.keys()
        system_proxy = resolved(http, ["system-proxy"])
        assert "client-proxy-system" in system_proxy.get("hyper-util", set())
        http2 = resolved(http, ["http2"])
        assert {"reqwest", "h2", "hyper", "hyper-util"} <= http2.keys()
        assert "http2" in http2["reqwest"]
        assert "http2" in http2["hyper"] and "http2" in http2["hyper-util"]
        charset = resolved(http, ["charset"])
        assert "encoding_rs" in charset
        for feature, absent in [("password", {"aes-gcm", "hkdf", "hmac", "sha2"}), ("sha256", {"argon2", "aes-gcm", "hkdf", "hmac"}), ("hmac-sha256", {"argon2", "aes-gcm", "hkdf"}), ("secret-envelope", {"argon2"})]:
            deps = resolved(crypto, [feature])
            assert not absent & deps.keys(), (feature, absent & deps.keys())
        for method, argument in [("json", "&[1]"), ("form", '&[("a", "b")]'), ("query", '&[("a", "b")]')]:
            negative(http, http_source, f'let _ = aster_forge_http::HttpClient::builder().build().unwrap().get("http://example.invalid/").{method}({argument})', f"no method named `{method}`")
        negative(http, http_source, 'let _ = aster_forge_http::HttpClient::builder().build().unwrap().get("http://example.invalid/").multipart(())', 'no method named `multipart`')
        negative(http, http_source, 'let _ = |r: reqwest::Response| r.bytes_stream()', 'no method named `bytes_stream`')
        negative(http, http_source, 'let _ = aster_forge_http::HttpClient::builder().build_blocking()', 'no method named `build_blocking`')
        negative(http, http_source, 'let _ = aster_forge_http::HttpClient::builder().add_root_certificate(())', 'no method named `add_root_certificate`')
        negative(http, http_source, 'let _ = aster_forge_http::HttpClient::builder().tls_backend(aster_forge_http::TlsBackend::Rustls)', 'could not find `TlsBackend`')
        for function, args in [("sha256_hex", 'b"test"'), ("hash_password", '"test"'), ("hmac_sha256_hex", 'b"key", b"test"'), ("encrypt_secret", 'b"key", b"context", b"aad", b"test"')]:
            negative(crypto, crypto_source, f"let _ = aster_forge_crypto::{function}({args})", f"cannot find function `{function}`")
        external = base / "external-auth"
        external.mkdir()
        (external / "src").mkdir()
        (external / "src/main.rs").write_text("fn main() {}\n")
        (external / "Cargo.toml").write_text(f'''[workspace]
[package]
name = "external-auth-feature-consumer"
version = "0.0.0"
edition = "2024"
[dependencies]
aster_forge_external_auth = {{ path = {json.dumps(str(ROOT / "crates/aster_forge_external_auth"))} }}
''')
        cargo(external, ["check"])
        external_deps = resolved(external, [])
        assert not {"form", "query", "multipart", "stream", "charset", "http2", "system-proxy"} & external_deps["reqwest"], external_deps["reqwest"]
        assert external_deps["aster_forge_crypto"] <= {"sha256", "default"} and "sha256" in external_deps["aster_forge_crypto"], external_deps["aster_forge_crypto"]
        print("PASS isolated dependency trees, downstream external auth and disabled APIs", flush=True)


if __name__ == "__main__":
    main()
