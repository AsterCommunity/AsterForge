//! Email validation and normalization helpers.
//!
//! The helpers implement Aster's lightweight email contract: trim and lowercase values, reject
//! obviously malformed addresses, and keep the behavior independent from any account or identity
//! provider model.

use crate::{Result, ValidationError};

/// Validates a normalized email address using Aster's lightweight email rules.
pub fn validate_email(email: &str) -> Result<()> {
    if email.len() > 254 {
        return Err(ValidationError::new("email is too long"));
    }
    if email.matches('@').count() != 1 {
        return Err(ValidationError::new("invalid email format"));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(ValidationError::new("invalid email format"));
    };
    if local.is_empty() || domain.is_empty() {
        return Err(ValidationError::new("invalid email format"));
    }
    if !domain.contains('.') {
        return Err(ValidationError::new("invalid email format"));
    }
    Ok(())
}

/// Trims and lowercases an email address, then validates it.
pub fn normalize_email(email: &str) -> Result<String> {
    let normalized = email.trim().to_ascii_lowercase();
    validate_email(&normalized)?;
    Ok(normalized)
}

/// Returns the lowercased domain portion of an email address.
pub fn email_domain(email: &str) -> Result<String> {
    let normalized = normalize_email(email)?;
    normalized
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_ascii_lowercase())
        .ok_or_else(|| ValidationError::new("invalid email format"))
}

#[cfg(test)]
mod tests {
    use super::{email_domain, normalize_email, validate_email};

    #[test]
    fn validate_email_requires_exactly_one_at_separator() {
        assert!(validate_email("alice@example.com").is_ok());
        assert!(validate_email("alice@@example.com").is_err());
        assert!(validate_email("alice@example@com").is_err());
        assert!(validate_email("alice.example.com").is_err());
        assert!(validate_email("@example.com").is_err());
        assert!(validate_email("alice@").is_err());
    }

    #[test]
    fn email_helpers_keep_existing_normalization_contract() {
        assert_eq!(
            normalize_email(" Alice@Example.COM ").unwrap(),
            "alice@example.com"
        );
        assert_eq!(email_domain("alice@Example.COM").unwrap(), "example.com");
    }

    #[test]
    fn validate_email_rejects_missing_domain_dot_and_overlong_values() {
        assert!(validate_email("alice@example").is_err());
        assert!(validate_email("alice@.").is_ok());

        let long_local = "a".repeat(245);
        let too_long = format!("{long_local}@example.com");
        assert!(too_long.len() > 254);
        assert!(validate_email(&too_long).is_err());
    }
}
