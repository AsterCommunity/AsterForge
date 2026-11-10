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

mod builder;
mod client;
mod reqwest_client;
mod response_body;

pub use builder::HttpClientBuilder;
#[cfg(any(feature = "rustls", feature = "native-tls"))]
pub use builder::TlsBackend;
pub use client::{HttpClient, HttpObservation, HttpObserver, HttpOutcome, HttpRequestBuilder};
pub use reqwest_client::{BufferedHttpError, execute_reqwest_buffered_limited};
pub use response_body::read_reqwest_body_limited;

#[cfg(test)]
mod test_support {
    /// Initialize the provider at the test application boundary before constructing any client.
    /// Keep provider selection in sync with tests/support/mod.rs; integration tests are separate crates.
    pub fn initialize_tls() {
        #[cfg(feature = "rustls")]
        {
            #[cfg(feature = "rustls-ring")]
            let provider = tokio_rustls::rustls::crypto::ring::default_provider();
            #[cfg(not(feature = "rustls-ring"))]
            let provider = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider();
            let _ = provider.install_default();
        }
    }
}
