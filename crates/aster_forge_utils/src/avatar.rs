//! Avatar presentation helpers.
//!
//! This module keeps the product-neutral parts of Gravatar presentation: the
//! normalized email hash and the conventional URL shape used by Aster services.
//! Product crates still own avatar source policy, upload handling, route paths,
//! cache headers, and which sizes they expose.

use md5::{Digest, Md5};

/// Returns the lowercase MD5 hash used by Gravatar for `email`.
pub fn gravatar_hash(email: &str) -> String {
    let normalized = email.trim().to_lowercase();
    let mut hasher = Md5::new();
    hasher.update(normalized.as_bytes());
    hex_lower(&hasher.finalize())
}

/// Builds a Gravatar URL with Aster's default public query parameters.
pub fn gravatar_url(email: &str, size: u32, base_url: &str) -> String {
    let hash = gravatar_hash(email);
    let base = base_url.trim_end_matches('/');
    format!("{base}/{hash}?d=identicon&s={size}&r=g")
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{gravatar_hash, gravatar_url};

    #[test]
    fn gravatar_hash_trims_and_lowercases_email() {
        assert_eq!(
            gravatar_hash("  MyEmailAddress@example.com "),
            "0bc83cb571cd1c50ba6f3e8a78ef1346"
        );
    }

    #[test]
    fn gravatar_url_uses_default_query_parameters_and_trims_base_slashes() {
        assert_eq!(
            gravatar_url("user@example.com", 512, "https://www.gravatar.com/avatar"),
            "https://www.gravatar.com/avatar/b58996c504c5638798eb6b511e6f49af?d=identicon&s=512&r=g"
        );
        assert_eq!(
            gravatar_url("user@example.com", 1024, "https://mirror.example/avatar/"),
            "https://mirror.example/avatar/b58996c504c5638798eb6b511e6f49af?d=identicon&s=1024&r=g"
        );
    }
}
