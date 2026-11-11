/// Initialize the provider at the test application boundary before constructing any client.
/// Keep provider selection in sync with `src/lib.rs::test_support`; integration tests cannot import cfg(test) modules.
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
