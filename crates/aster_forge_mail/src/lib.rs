//! Shared mail infrastructure helpers for Aster services.
//!
//! This crate intentionally does not own product templates, recipients, audit
//! records, user context, SMTP configuration keys, or database entities. It only
//! provides small mechanics that recur around mail outbox dispatch: dispatch
//! counters, retry delay selection, error truncation, and best-effort retry when
//! persisting a successful SMTP delivery.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

pub mod outbox;

pub use outbox::{
    DEFAULT_ERROR_MAX_LEN, DEFAULT_MARK_SENT_RETRY_DELAYS_MS, DispatchStats, MailOutboxRetryPolicy,
    retry_mark_sent, truncate_error,
};
