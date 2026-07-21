//! Product-neutral normalization helpers for external authentication settings.
//!
//! These helpers cover small validation rules that recur in Aster services: provider keys, scopes,
//! claim names, provider URLs, allow-listed email domains, flow tokens, return paths, and hashed
//! login-flow secrets. Product crates still own callback URL construction, local email validation,
//! persisted provider rows, API error codes, and account-linking policy.

use crate::{ExternalAuthError, ExternalAuthProtocol, Result};

/// Default scopes used when a provider or request does not specify its own scope list.
pub const DEFAULT_SCOPES: &str = "openid email profile";

/// Default maximum length for external-auth provider URLs.
pub const DEFAULT_EXTERNAL_AUTH_URL_MAX_LEN: usize = 2048;

/// Default maximum length for provider identity namespace values such as issuer URLs.
pub const DEFAULT_EXTERNAL_AUTH_IDENTITY_NAMESPACE_MAX_LEN: usize = 512;

/// Normalizes an application-owned provider key.
pub fn normalize_provider_key(value: &str) -> Result<String> {
    let key = value.trim().to_ascii_lowercase();
    if key.len() < 2 || key.len() > 64 {
        return Err(ExternalAuthError::validation_error(
            "external auth provider key must be 2-64 characters",
        ));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ExternalAuthError::validation_error(
            "external auth provider key may only contain lowercase letters, numbers and hyphens",
        ));
    }
    if key.starts_with('-') || key.ends_with('-') {
        return Err(ExternalAuthError::validation_error(
            "external auth provider key cannot start or end with '-'",
        ));
    }
    Ok(key)
}

/// Normalizes a required string field with a byte-length limit.
pub fn normalize_required_field(value: &str, field: &str, max_len: usize) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ExternalAuthError::validation_error(format!(
            "{field} is required"
        )));
    }
    if trimmed.len() > max_len {
        return Err(ExternalAuthError::validation_error(format!(
            "{field} exceeds {max_len} bytes"
        )));
    }
    Ok(trimmed.to_string())
}

/// Normalizes an optional provider claim name.
pub fn normalize_optional_claim(value: Option<String>, field: &str) -> Result<Option<String>> {
    match value {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.len() > 128 {
                Err(ExternalAuthError::validation_error(format!(
                    "{field} exceeds 128 bytes"
                )))
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        None => Ok(None),
    }
}

/// Normalizes scopes with a caller-provided default.
pub fn normalize_scopes_with_default(
    value: Option<&str>,
    default_scopes: &str,
    protocol: ExternalAuthProtocol,
) -> Result<String> {
    let raw = value.unwrap_or(default_scopes);
    let mut scopes = Vec::new();
    for scope in raw.split_whitespace() {
        let scope = scope.trim();
        if scope.is_empty() || scopes.iter().any(|existing| existing == scope) {
            continue;
        }
        if scope.chars().any(char::is_control) || scope.len() > 128 {
            return Err(ExternalAuthError::validation_error(
                "invalid external auth scope",
            ));
        }
        scopes.push(scope.to_string());
    }
    if protocol == ExternalAuthProtocol::Oidc && !scopes.iter().any(|scope| scope == "openid") {
        scopes.insert(0, "openid".to_string());
    }
    Ok(scopes.join(" "))
}

/// Normalizes scopes with Forge's default `openid email profile` value.
pub fn normalize_scopes(value: Option<&str>, protocol: ExternalAuthProtocol) -> Result<String> {
    normalize_scopes_with_default(value, DEFAULT_SCOPES, protocol)
}

fn parse_external_auth_url(value: &str, context: &str) -> Result<url::Url> {
    aster_forge_utils::url::parse_url(value, context)
        .map_err(|error| ExternalAuthError::validation_error(error.to_string()))
}

fn normalize_optional_url(
    value: Option<String>,
    field: &str,
    max_len: usize,
) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > max_len {
        return Err(ExternalAuthError::validation_error(format!(
            "{field} exceeds {max_len} bytes"
        )));
    }
    let parse_context = format!("invalid external auth {field}");
    let parsed = parse_external_auth_url(trimmed, &parse_context)?;
    if !aster_forge_utils::url::is_https_or_loopback_http(&parsed) {
        return Err(ExternalAuthError::validation_error(format!(
            "external auth {field} must use HTTPS, except localhost"
        )));
    }
    if parsed.fragment().is_some() {
        return Err(ExternalAuthError::validation_error(format!(
            "external auth {field} cannot include fragment"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

/// Normalizes an icon URL that may be a root-relative path or HTTPS URL.
pub fn normalize_icon_url_input(value: Option<String>, max_len: usize) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > max_len {
        return Err(ExternalAuthError::validation_error(format!(
            "icon_url exceeds {max_len} bytes"
        )));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(ExternalAuthError::validation_error(
            "external auth icon_url cannot contain whitespace",
        ));
    }
    if trimmed.starts_with('/') && !trimmed.starts_with("//") {
        return Ok(Some(trimmed.to_string()));
    }
    let parsed = parse_external_auth_url(trimmed, "invalid external auth icon_url")?;
    if !aster_forge_utils::url::is_https_or_loopback_http(&parsed) {
        return Err(ExternalAuthError::validation_error(
            "external auth icon_url must be a root-relative path or HTTPS URL, except localhost",
        ));
    }
    if parsed.fragment().is_some() {
        return Err(ExternalAuthError::validation_error(
            "external auth icon_url cannot include fragment",
        ));
    }
    Ok(Some(trimmed.to_string()))
}

/// Normalizes an issuer URL.
pub fn normalize_issuer_url_input(
    value: Option<String>,
    required: bool,
    max_len: usize,
) -> Result<Option<String>> {
    let Some(issuer) = normalize_optional_url(value, "issuer_url", max_len)? else {
        if required {
            return Err(ExternalAuthError::validation_error(
                "issuer_url is required",
            ));
        }
        return Ok(None);
    };
    let parsed = parse_external_auth_url(&issuer, "invalid external auth issuer_url")?;
    if parsed.query().is_some() {
        return Err(ExternalAuthError::validation_error(
            "external auth issuer_url cannot include query or fragment",
        ));
    }
    Ok(Some(issuer.trim_end_matches('/').to_string()))
}

/// Normalizes a manually configured provider endpoint.
pub fn normalize_manual_endpoint_input(
    value: Option<String>,
    field: &str,
    required: bool,
    supported: bool,
    max_len: usize,
) -> Result<Option<String>> {
    let endpoint = normalize_optional_url(value, field, max_len)?;
    if endpoint.is_some() && !supported {
        return Err(ExternalAuthError::validation_error(format!(
            "{field} is not supported for this external auth provider kind"
        )));
    }
    if endpoint.is_none() && required {
        return Err(ExternalAuthError::validation_error(format!(
            "{field} is required"
        )));
    }
    Ok(endpoint)
}

/// Normalizes an allow-list of email domains into a stable JSON string.
pub fn normalize_allowed_domains(value: Option<Vec<String>>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let mut domains = Vec::new();
    for raw in value {
        let domain = raw.trim().trim_start_matches('@').to_ascii_lowercase();
        if domain.is_empty() {
            continue;
        }
        if domain.len() > 253
            || !domain.contains('.')
            || domain
                .chars()
                .any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'))
        {
            return Err(ExternalAuthError::validation_error(format!(
                "invalid external auth allowed domain '{raw}'"
            )));
        }
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
    if domains.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&domains).map(Some).map_err(|error| {
        ExternalAuthError::internal_error(format!(
            "failed to serialize external auth allowed domains: {error}"
        ))
    })
}

/// Parses a stored allow-list JSON string into domain entries.
pub fn parse_allowed_domains(raw: Option<&str>) -> Result<Vec<String>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<String>>(trimmed).map_err(|error| {
        ExternalAuthError::state_error(format!(
            "failed to parse external auth allowed domains: {error}"
        ))
    })
}

/// Returns whether an email address is allowed by a stored allow-list JSON string.
pub fn email_domain_allowed(allowed_domains: Option<&str>, email: &str) -> Result<bool> {
    let domains = parse_allowed_domains(allowed_domains)?;
    if domains.is_empty() {
        return Ok(true);
    }
    let Some((_, domain)) = email.rsplit_once('@') else {
        return Ok(false);
    };
    let domain = domain.to_ascii_lowercase();
    Ok(domains.iter().any(|allowed| allowed == &domain))
}

/// Hashes an OAuth/OIDC state value before persistence.
pub fn state_hash(state: &str) -> String {
    aster_forge_crypto::sha256_hex(state.as_bytes())
}

/// Hashes an external-auth flow token before persistence.
pub fn token_hash(token: &str) -> String {
    aster_forge_crypto::sha256_hex(token.as_bytes())
}

/// Normalizes a post-login return path.
pub fn normalize_return_path(value: Option<&str>, max_len: usize) -> Result<String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok("/".to_string());
    };
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(ExternalAuthError::validation_error(
            "invalid external auth return_path",
        ));
    }
    if value.len() > max_len {
        return Err(ExternalAuthError::validation_error(
            "external auth return_path is too long",
        ));
    }
    Ok(value.to_string())
}

/// Normalizes an external-auth flow token supplied by a client.
pub fn normalize_flow_token(value: &str, max_len: usize) -> Result<String> {
    let token = value.trim();
    if token.is_empty() {
        return Err(ExternalAuthError::validation_error(
            "external auth flow_token is required",
        ));
    }
    if token.len() > max_len || token.chars().any(char::is_whitespace) {
        return Err(ExternalAuthError::validation_error(
            "invalid external auth flow_token",
        ));
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_key_is_lowercase_hyphenated_identifier() {
        assert_eq!(normalize_provider_key(" GitHub ").unwrap(), "github");
        assert_eq!(
            normalize_provider_key("microsoft-work").unwrap(),
            "microsoft-work"
        );
        assert!(normalize_provider_key("-bad").is_err());
        assert!(normalize_provider_key("bad_underscore").is_err());
    }

    #[test]
    fn scopes_are_deduplicated_and_oidc_scopes_include_openid() {
        assert_eq!(
            normalize_scopes_with_default(
                Some("email profile email"),
                "",
                ExternalAuthProtocol::OAuth2
            )
            .unwrap(),
            "email profile"
        );
        assert_eq!(
            normalize_scopes_with_default(Some("email"), "", ExternalAuthProtocol::Oidc).unwrap(),
            "openid email"
        );
    }

    #[test]
    fn icon_url_allows_root_relative_and_https_or_loopback_http() {
        assert_eq!(
            normalize_icon_url_input(Some("/assets/icon.svg".to_string()), 2048).unwrap(),
            Some("/assets/icon.svg".to_string())
        );
        assert!(
            normalize_icon_url_input(Some("http://localhost/icon.svg".to_string()), 2048).is_ok()
        );
        assert!(
            normalize_icon_url_input(Some("//cdn.example.com/icon.svg".to_string()), 2048).is_err()
        );
        assert!(
            normalize_icon_url_input(Some("http://example.com/icon.svg".to_string()), 2048)
                .is_err()
        );
    }

    #[test]
    fn issuer_url_rejects_query_and_strips_trailing_slash() {
        assert_eq!(
            normalize_issuer_url_input(Some("https://id.example.com/".to_string()), true, 512)
                .unwrap(),
            Some("https://id.example.com".to_string())
        );
        assert!(
            normalize_issuer_url_input(Some("https://id.example.com/?x=1".to_string()), true, 512)
                .is_err()
        );
        assert!(normalize_issuer_url_input(None, true, 512).is_err());
    }

    #[test]
    fn allowed_domains_are_normalized_and_deduplicated() {
        assert_eq!(
            normalize_allowed_domains(Some(vec![
                " Example.COM ".to_string(),
                "@example.com".to_string(),
                "sub.example.com".to_string(),
            ]))
            .unwrap()
            .as_deref(),
            Some(r#"["example.com","sub.example.com"]"#)
        );
        assert!(email_domain_allowed(Some(r#"["example.com"]"#), "user@example.com").unwrap());
        assert!(!email_domain_allowed(Some(r#"["example.com"]"#), "user@test.com").unwrap());
    }

    #[test]
    fn return_path_and_flow_token_reject_unsafe_values() {
        assert_eq!(normalize_return_path(None, 2048).unwrap(), "/");
        assert_eq!(
            normalize_return_path(Some("/dashboard?tab=auth"), 2048).unwrap(),
            "/dashboard?tab=auth"
        );
        assert!(normalize_return_path(Some("//evil.example.com"), 2048).is_err());
        assert!(normalize_return_path(Some("/bad\\path"), 2048).is_err());
        // Control characters (CR/LF, TAB, NUL) must not pass: products may emit
        // the stored path into redirects or logs where they become injection
        // primitives. The scopes/claims/provider-key normalizers already reject
        // `char::is_control`; return_path must match.
        assert!(normalize_return_path(Some("/ok\r\nhttps://evil.example.com"), 2048).is_err());
        assert!(normalize_return_path(Some("/tab\ttab"), 2048).is_err());
        assert!(normalize_return_path(Some("/nul\0byte"), 2048).is_err());

        assert_eq!(normalize_flow_token(" token ", 128).unwrap(), "token");
        assert!(normalize_flow_token("bad token", 128).is_err());
    }

    #[test]
    fn state_and_token_hash_use_sha256_hex() {
        assert_eq!(state_hash("state").len(), 64);
        assert_eq!(state_hash("state"), token_hash("state"));
    }
}
