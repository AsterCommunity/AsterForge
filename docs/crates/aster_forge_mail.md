# aster_forge_mail

`aster_forge_mail` contains product-neutral mail infrastructure helpers. It does not own SMTP config keys, templates, recipients, user context, audit records, or database entities.

The current crate focuses on mail outbox dispatch mechanics that repeat across services:

- dispatch counters;
- retry delay policy;
- delivery error truncation;
- best-effort retry when persisting `sent` after SMTP success.

## Use Cases

- A product has its own `mail_outbox` table and wants shared dispatch counters.
- A product wants consistent retry delay selection after temporary delivery failure.
- A product wants UTF-8-safe truncation for stored delivery errors.
- A product wants a shared helper for retrying `mark_sent` after SMTP already accepted a message.

Do not put product templates, template codes, audit actions, user IDs, runtime config keys, or concrete SeaORM repositories in this crate.

## Cargo

```toml
[dependencies]
aster_forge_mail = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Outbox

Module: `aster_forge_mail::outbox`

Main types and functions:

- `DispatchStats`
- `MailOutboxRetryPolicy`
- `DEFAULT_ERROR_MAX_LEN`
- `DEFAULT_MARK_SENT_RETRY_DELAYS_MS`
- `retry_delay_secs(attempt_count)`
- `truncate_error(error, max_len)`
- `retry_mark_sent(id, retry_delays_ms, mark_sent)`

`DispatchStats` is a small counter type:

```rust
use aster_forge_mail::DispatchStats;

let mut total = DispatchStats::default();
total.merge(DispatchStats {
    claimed: 1,
    sent: 1,
    retried: 0,
    failed: 0,
});
```

`MailOutboxRetryPolicy` captures product-neutral retry decisions:

```rust
use aster_forge_mail::{DEFAULT_ERROR_MAX_LEN, MailOutboxRetryPolicy};

let policy = MailOutboxRetryPolicy::new(6, DEFAULT_ERROR_MAX_LEN);

if policy.should_permanently_fail(attempt_count) {
    // Product code marks the row failed and records product audit details.
} else {
    let retry_after_secs = policy.retry_delay_secs(attempt_count);
    // Product code writes next_attempt_at using its own clock/database layer.
}
```

`retry_mark_sent` narrows the duplicate-delivery window where SMTP succeeds but the database row still says `Processing`:

```rust
use aster_forge_mail::{DEFAULT_MARK_SENT_RETRY_DELAYS_MS, retry_mark_sent};

let updated = retry_mark_sent(
    outbox_id,
    DEFAULT_MARK_SENT_RETRY_DELAYS_MS,
    |id, _attempt| async move {
        product_mail_outbox_repo::mark_sent(db, id, now()).await
    },
)
.await?;
```

The product callback owns timestamps, repository calls, error types, transactions, and logging context. Forge only performs the retry loop and delay scheduling.

## Error Boundary

`retry_mark_sent` returns the product callback's error type. This keeps database and API error mapping in the product crate.

## Testing

Forge tests cover:

- dispatch counter merging;
- default retry delay policy;
- UTF-8-safe truncation;
- retry-until-success and final-failure behavior for `retry_mark_sent`.

Product tests should still cover:

- repository claim fences;
- stale processing reclaim;
- template rendering;
- audit logging;
- SMTP sender configuration;
- transaction behavior.

## Reference Projects

- AsterYggdrasil: keeps templates, audit, SeaORM repositories, and `MailSender` in product code, while using Forge for dispatch stats, retry policy, truncation, and `mark_sent` retry mechanics.
