//! Email allow/block list normalization and matching helpers.
//!
//! Product crates still decide which configuration keys contain allowlists or
//! blocklists, which API error codes to return, and whether an empty allowlist
//! means "allow everyone" or "deny everyone". This module only owns the shared
//! mechanics for parsing policy entries, normalizing them, deduplicating them in
//! a stable order, and testing exact email/domain matches.

use std::collections::BTreeSet;

use crate::email::{email_domain, normalize_email};
use crate::{Result, ValidationError};

/// Normalized email policy list split into exact email and exact domain sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmailPolicyList {
    emails: BTreeSet<String>,
    domains: BTreeSet<String>,
}

impl EmailPolicyList {
    /// Builds a policy list from raw entries.
    ///
    /// Blank entries are ignored. Non-blank invalid entries fail the whole
    /// normalization pass so configuration writes do not silently persist typos.
    pub fn from_items<I, S>(items: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut list = Self::default();
        for item in items {
            let item = item.as_ref().trim();
            if item.is_empty() {
                continue;
            }
            list.insert(parse_email_policy_item(item)?);
        }
        Ok(list)
    }

    /// Builds a best-effort policy list from raw entries.
    ///
    /// Invalid entries are skipped and passed to `on_invalid`, which lets
    /// runtime readers preserve fail-open startup behavior while still logging
    /// the ignored item.
    pub fn from_items_lossy<I, S, F>(items: I, mut on_invalid: F) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        F: FnMut(&str, &ValidationError),
    {
        let mut list = Self::default();
        for item in items {
            let item = item.as_ref().trim();
            if item.is_empty() {
                continue;
            }
            match parse_email_policy_item(item) {
                Ok(entry) => list.insert(entry),
                Err(error) => on_invalid(item, &error),
            }
        }
        list
    }

    /// Returns whether no emails or domains are configured.
    pub fn is_empty(&self) -> bool {
        self.emails.is_empty() && self.domains.is_empty()
    }

    /// Returns whether `email` or `domain` exactly matches this list.
    pub fn matches(&self, email: &str, domain: &str) -> bool {
        self.emails.contains(email) || self.domains.contains(domain)
    }

    /// Returns normalized entries as a stable sorted vector.
    pub fn entries(&self) -> Vec<String> {
        self.emails
            .iter()
            .chain(self.domains.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn insert(&mut self, item: EmailPolicyEntry) {
        match item {
            EmailPolicyEntry::Email(value) => {
                self.emails.insert(value);
            }
            EmailPolicyEntry::Domain(value) => {
                self.domains.insert(value);
            }
        }
    }
}

/// Normalized policy entry classified as either an exact email or an exact domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailPolicyEntry {
    /// Exact normalized email address.
    Email(String),
    /// Exact normalized email domain.
    Domain(String),
}

/// Normalizes and deduplicates raw email policy entries into a stable vector.
pub fn normalize_email_policy_items<I, S>(items: I) -> Result<Vec<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    Ok(EmailPolicyList::from_items(items)?.entries())
}

/// Parses one raw policy entry.
///
/// Entries containing `@` are treated as exact email addresses unless they start
/// with a single leading `@`, in which case they are treated as domains. Entries
/// without `@` are treated as exact domains.
pub fn parse_email_policy_item(item: &str) -> Result<EmailPolicyEntry> {
    if let Some(domain) = item.strip_prefix('@')
        && !domain.contains('@')
    {
        return normalize_email_policy_domain(domain).map(EmailPolicyEntry::Domain);
    }

    if item.contains('@') {
        return normalize_email_policy_email(item).map(EmailPolicyEntry::Email);
    }

    normalize_email_policy_domain(item).map(EmailPolicyEntry::Domain)
}

/// Normalizes an exact email policy entry.
pub fn normalize_email_policy_email(email: &str) -> Result<String> {
    let normalized = normalize_email(email)?;
    Ok(normalized.to_ascii_lowercase())
}

/// Normalizes an exact email domain policy entry.
pub fn normalize_email_policy_domain(domain: &str) -> Result<String> {
    let normalized = domain.trim().trim_start_matches('@').to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 253
        || normalized.contains('@')
        || !normalized.contains('.')
        || normalized.starts_with('.')
        || normalized.ends_with('.')
        || normalized.contains("..")
    {
        return Err(ValidationError::new(format!(
            "invalid email policy domain '{domain}'"
        )));
    }

    if !normalized.split('.').all(|label| {
        !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) {
        return Err(ValidationError::new(format!(
            "invalid email policy domain '{domain}'"
        )));
    }

    Ok(normalized)
}

/// Normalizes an email and returns its exact-match domain.
pub fn normalized_email_and_domain(email: &str) -> Result<(String, String)> {
    let normalized = normalize_email_policy_email(email)?;
    let domain = email_domain(&normalized)?;
    Ok((normalized, domain))
}

#[cfg(test)]
mod tests {
    use super::{
        EmailPolicyEntry, EmailPolicyList, normalize_email_policy_domain,
        normalize_email_policy_items, normalized_email_and_domain, parse_email_policy_item,
    };

    #[test]
    fn policy_items_are_trimmed_lowercased_deduplicated_and_sorted() {
        let normalized = normalize_email_policy_items([
            " Example.COM ",
            "alice@Example.com",
            "example.com",
            " ALICE@example.COM ",
            "@Team.Example",
        ])
        .unwrap();

        assert_eq!(
            normalized,
            vec![
                "alice@example.com".to_string(),
                "example.com".to_string(),
                "team.example".to_string(),
            ]
        );
    }

    #[test]
    fn policy_item_parser_classifies_emails_and_domains() {
        assert_eq!(
            parse_email_policy_item("alice@example.com").unwrap(),
            EmailPolicyEntry::Email("alice@example.com".to_string())
        );
        assert_eq!(
            parse_email_policy_item("@example.com").unwrap(),
            EmailPolicyEntry::Domain("example.com".to_string())
        );
        assert_eq!(
            parse_email_policy_item("example.com").unwrap(),
            EmailPolicyEntry::Domain("example.com".to_string())
        );
    }

    #[test]
    fn invalid_domains_are_rejected() {
        assert!(normalize_email_policy_domain("localhost").is_err());
        assert!(normalize_email_policy_domain("用户.中国").is_err());
        assert_eq!(
            normalize_email_policy_domain("xn--fiq228c.xn--fiqs8s").unwrap(),
            "xn--fiq228c.xn--fiqs8s"
        );
    }

    #[test]
    fn policy_list_matches_exact_emails_and_domains() {
        let list = EmailPolicyList::from_items(["example.com", "alice@other.test", "blocked.test"])
            .unwrap();

        assert!(list.matches("bob@example.com", "example.com"));
        assert!(list.matches("alice@other.test", "other.test"));
        assert!(!list.matches("bob@sub.example.com", "sub.example.com"));
    }

    #[test]
    fn lossy_policy_list_skips_invalid_items() {
        let mut invalid = Vec::new();
        let list =
            EmailPolicyList::from_items_lossy(["example.com", "localhost"], |item, error| {
                invalid.push((item.to_string(), error.to_string()));
            });

        assert!(list.matches("alice@example.com", "example.com"));
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "localhost");
    }

    #[test]
    fn normalized_email_and_domain_returns_exact_match_parts() {
        assert_eq!(
            normalized_email_and_domain(" Alice@Example.COM ").unwrap(),
            ("alice@example.com".to_string(), "example.com".to_string())
        );
    }
}
