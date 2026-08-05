//! Product-neutral outbound HTTP client mechanics for Aster services.
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

mod reqwest_client;
mod response_body;

pub use reqwest_client::{BufferedHttpError, execute_reqwest_buffered_limited};
pub use response_body::read_reqwest_body_limited;
