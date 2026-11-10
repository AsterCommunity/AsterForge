/// Initialize the provider at the test application boundary before constructing any client.
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
