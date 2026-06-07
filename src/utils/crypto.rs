//! Compatibility token hashing helpers.

pub fn sha256_hex(value: &str) -> String {
    crate::utils::hash::sha256_hex(value.as_bytes())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sha256_hex_hashes_utf8_string_bytes() {
        assert_eq!(
            super::sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            super::sha256_hex("AsterForge"),
            crate::utils::hash::sha256_hex("AsterForge".as_bytes())
        );
    }
}
