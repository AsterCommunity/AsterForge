# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_utils`. See [`../../docs/crates/aster_forge_utils.md`](../../docs/crates/aster_forge_utils.md) for the module inventory and behavior contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the target module, the crate documentation, and module-local tests.
- Search the workspace for a more precise owner first: configuration normalizers belong in `aster_forge_config`, filenames in `aster_forge_validation`, object keys in `aster_forge_storage_core`, and pagination in `aster_forge_api`. Do not turn utils into a junk drawer.

## Ownership Boundaries

- This crate contains only small, dependency-light, product-neutral primitives with explicit boundaries: backoff, bool-like parsing, HTTP ranges and validators, networking and IP handling, numeric conversions, paths, URLs, text, RAII, and similar mechanics.
- Do not add product configuration structures, business defaults, state machines, repositories, API responses, or convenience wrappers used by only one caller.

## Change Constraints

- A new module must explain why no domain crate is a better owner and why multiple subsystems can reuse its API naturally.
- Helpers for untrusted input limit length, count, or depth before parsing or allocation, and return structured `UtilsError` values.
- Numeric conversions remain checked. Never use `as` to hide truncation or sign loss.
- URL and origin handling, trusted proxies, and HTTP conditional/range logic are security contracts. Test ambiguity, case handling, proxy chains, and RFC precedence.
- RAII cleanup never panics in `Drop`; explicit cleanup may return errors and remains idempotent.
- Path helpers account for Unix, Windows, and configuration-relative paths rather than pretending string replacement is cross-platform support.

## Validation

```bash
cargo test -p aster_forge_utils
cargo clippy -p aster_forge_utils --all-targets -- -D warnings
```

Add property or boundary tests for pure functions. Security parsers must cover empty and over-limit values, malformed UTF-8 or encodings, platform paths, proxy chains, HTTP range/ETag/date boundaries, and panic freedom.
