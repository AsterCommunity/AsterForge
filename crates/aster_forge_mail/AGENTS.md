# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_mail`. See [`../../docs/crates/aster_forge_mail.md`](../../docs/crates/aster_forge_mail.md) for the complete configuration, sender, outbox, template, and runtime-component contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, and the target module. For persistence, inspect `aster_forge_db::mail_outbox`; for lifecycle, inspect `aster_forge_runtime` and `aster_forge_tasks`.
- Separate three outcomes before coding: SMTP accepted the message, the DB marked it sent, and the product audit callback completed. Do not flatten distributed side effects into one vague `Result`.

## Ownership Boundaries

- This crate owns SMTP runtime settings and normalizers, messages and senders, the generic template catalog and renderer, outbox dispatch, retry and drain, and the mail runtime component.
- Shared outbox schemas and stores belong to `aster_forge_db`.
- Products own configuration keys, template codes, payloads and text, business URLs, localization, recipient and user context, audit, and concrete repository adapters.

## Change Constraints

- If SMTP succeeds and `mark_sent` fails, use only the existing best-effort persistence retry. Never resend and create a duplicate message.
- Claim, sent, retry, and failed transitions retain their fences. Permanent failure clears sensitive payloads according to the contract.
- Retry policy, error truncation, and drain deadlines need explicit caps, and truncation must be UTF-8-safe.
- Placeholder replacement remains single-pass. Unknown or unclosed tokens, HTML escaping, and text fallback must not introduce recursive expansion or double decoding.
- Template code and variable uniqueness are build-time errors; later registrations must not silently overwrite earlier ones.
- Component shutdown-dependency changes require checking the audit, DB, and task graphs together.

## Validation

```bash
cargo test -p aster_forge_mail
cargo check -p aster_forge_mail --no-default-features
cargo test -p aster_forge_mail --all-features
cargo clippy -p aster_forge_mail --all-targets --all-features -- -D warnings
```

Cover SMTP timeout and temporary/permanent failure, mark-sent retry, claim races, drain deadlines, sensitive-field cleanup, duplicate templates, unknown tokens, nested HTML, and feature combinations.
