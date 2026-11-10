fn main() -> Result<(), Box<dyn std::error::Error>> {
    use aster_forge_crypto::bytes_to_hex;
    assert_eq!(bytes_to_hex(&[0, 255]), "00ff");
    #[cfg(feature = "sha256")]
    assert_eq!(
        aster_forge_crypto::sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    #[cfg(feature = "password")]
    {
        use aster_forge_crypto::{
            PasswordHashPolicy, PasswordHashVerificationLimits, PasswordHashWorkFactor,
        };
        let policy = PasswordHashPolicy::new(
            PasswordHashWorkFactor::new(8, 1, 1, 32)?,
            PasswordHashVerificationLimits::new(8, 1, 1, 32)?,
        )?;
        let hash = aster_forge_crypto::hash_password_with_policy("test", &policy)?;
        assert!(aster_forge_crypto::verify_password_with_policy("test", &hash, &policy)?.is_valid);
    }
    #[cfg(feature = "hmac-sha256")]
    assert_eq!(
        aster_forge_crypto::hmac_sha256_hex(&[0x0b; 20], b"Hi There")?,
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    #[cfg(feature = "secret-envelope")]
    {
        let envelope = aster_forge_crypto::encrypt_secret(
            b"fixture master key",
            b"fixture:v1",
            b"aad",
            b"test",
        )?;
        assert_eq!(
            aster_forge_crypto::decrypt_secret(
                b"fixture master key",
                b"fixture:v1",
                b"aad",
                &envelope
            )?,
            b"test"
        );
    }
    Ok(())
}
