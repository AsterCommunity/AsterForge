//! Generic `OAuth2` authorization-code provider driver.
//!
//! This driver supports manually configured authorization, token, and userinfo endpoints. It owns
//! the shared `OAuth2` mechanics reused by fixed-endpoint providers: PKCE generation, token exchange,
//! userinfo retrieval, safe endpoint diagnostics, and JSON claim extraction with dotted-path and
//! JSON-pointer support.

use async_trait::async_trait;
use base64::Engine as _;
use rand::RngExt;
use reqwest::header;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::driver::{
    ExternalAuthAuthorizationStart, ExternalAuthCallback, ExternalAuthProfile,
    ExternalAuthProviderConfig, ExternalAuthProviderDescriptor, ExternalAuthProviderDriver,
    ExternalAuthProviderTestCheck, ExternalAuthProviderTestResult,
};
use crate::outbound_http_user_agent;
use crate::types::{ExternalAuthProtocol, ExternalAuthProviderKind};
use crate::{ExternalAuthError, MapExternalAuthErr, Result};
use aster_forge_utils::url::parse_url;

const OAUTH2_DEFAULT_SCOPES: &str = "openid email profile";
const OAUTH2_NAMESPACE_MAX_LEN: usize = 512;
const OAUTH2_SUBJECT_MAX_LEN: usize = 255;
const OAUTH2_SNAPSHOT_MAX_LEN: usize = 255;
const TOKEN_ENDPOINT_TIMEOUT_SECS: u64 = 15;

#[derive(Clone, Copy)]
enum OAuth2TokenAuthMethod {
    ClientSecretPost,
    PublicClient,
}

#[derive(Clone, Copy)]
struct OAuth2TokenRequest<'a> {
    token_url: &'a str,
    provider: &'a ExternalAuthProviderConfig,
    code: &'a str,
    redirect_uri: &'a str,
    pkce_verifier: &'a str,
    auth_method: OAuth2TokenAuthMethod,
    client_secret: Option<&'a str>,
}

impl OAuth2TokenRequest<'_> {
    fn with_auth_method(self, auth_method: OAuth2TokenAuthMethod) -> Self {
        Self {
            auth_method,
            ..self
        }
    }
}

/// Generic `OAuth2` provider driver with manually configured endpoints.
#[derive(Default)]
pub struct OAuth2ProviderDriver;

#[derive(Debug, Deserialize)]
struct OAuth2TokenResponse {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuth2ErrorResponse {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

impl OAuth2ProviderDriver {
    /// Creates a generic `OAuth2` provider driver.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ExternalAuthProviderDriver for OAuth2ProviderDriver {
    fn kind(&self) -> ExternalAuthProviderKind {
        ExternalAuthProviderKind::GenericOAuth2
    }

    fn descriptor(&self) -> ExternalAuthProviderDescriptor {
        ExternalAuthProviderDescriptor {
            kind: ExternalAuthProviderKind::GenericOAuth2,
            protocol: ExternalAuthProtocol::OAuth2,
            display_name: "Generic OAuth2",
            description: "OAuth2 authorization-code sign-in using manually configured authorization, token and userinfo endpoints.",
            default_scopes: OAUTH2_DEFAULT_SCOPES,
            issuer_url_required: false,
            manual_endpoint_configuration_supported: true,
            authorization_url_required: true,
            token_url_required: true,
            userinfo_url_required: true,
            supports_discovery: false,
            supports_pkce: true,
            supports_email_verified_claim: true,
        }
    }

    async fn start_authorization(
        &self,
        provider: &ExternalAuthProviderConfig,
        redirect_uri: &str,
    ) -> Result<ExternalAuthAuthorizationStart> {
        let authorization_url = require_url(
            provider.authorization_url.as_deref(),
            "authorization_url",
            ExternalAuthError::config_error,
        )?;
        let mut authorization_url = validate_url(
            authorization_url,
            "authorization_url",
            ExternalAuthError::config_error,
        )?;
        let state = format!("oauth2_{}", aster_forge_utils::id::new_short_token());
        let pkce_verifier = build_pkce_verifier();
        let pkce_challenge = build_pkce_challenge(&pkce_verifier);

        {
            let mut query = authorization_url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", &provider.client_id);
            query.append_pair("redirect_uri", redirect_uri);
            query.append_pair("scope", provider.scopes.trim());
            query.append_pair("state", &state);
            query.append_pair("code_challenge", &pkce_challenge);
            query.append_pair("code_challenge_method", "S256");
        }

        Ok(ExternalAuthAuthorizationStart {
            authorization_url: authorization_url.to_string(),
            state,
            nonce: None,
            pkce_verifier: Some(pkce_verifier),
        })
    }

    async fn exchange_callback(
        &self,
        provider: &ExternalAuthProviderConfig,
        callback: ExternalAuthCallback,
    ) -> Result<ExternalAuthProfile> {
        let pkce_verifier = callback.pkce_verifier.ok_or_else(|| {
            ExternalAuthError::database_operation("stored OAuth2 PKCE verifier is missing")
        })?;
        let http_client = oauth2_http_client(provider)?;
        let token = exchange_code_for_token(
            &http_client,
            provider,
            &callback.code,
            &callback.redirect_uri,
            &pkce_verifier,
        )
        .await?;
        let profile_json = fetch_userinfo(&http_client, provider, &token).await?;
        profile_from_userinfo(provider, &profile_json)
    }

    async fn test_provider(
        &self,
        provider: &ExternalAuthProviderConfig,
    ) -> Result<ExternalAuthProviderTestResult> {
        let authorization_url = require_url(
            provider.authorization_url.as_deref(),
            "authorization_url",
            ExternalAuthError::validation_error,
        )?;
        let token_url = require_url(
            provider.token_url.as_deref(),
            "token_url",
            ExternalAuthError::validation_error,
        )?;
        let userinfo_url = require_url(
            provider.userinfo_url.as_deref(),
            "userinfo_url",
            ExternalAuthError::validation_error,
        )?;
        validate_url(
            authorization_url,
            "authorization_url",
            ExternalAuthError::validation_error,
        )?;
        validate_url(token_url, "token_url", ExternalAuthError::validation_error)?;
        validate_url(
            userinfo_url,
            "userinfo_url",
            ExternalAuthError::validation_error,
        )?;
        if provider.client_id.trim().is_empty() {
            return Err(ExternalAuthError::validation_error("client_id is required"));
        }

        Ok(ExternalAuthProviderTestResult {
            provider: self.descriptor().display_name.to_string(),
            issuer: provider.issuer_url.clone(),
            authorization_endpoint: Some(authorization_url.to_string()),
            token_endpoint: Some(token_url.to_string()),
            userinfo_endpoint: Some(userinfo_url.to_string()),
            jwks_key_count: None,
            checks: vec![
                ExternalAuthProviderTestCheck {
                    name: "manual_endpoints".to_string(),
                    success: true,
                    message: "OAuth2 authorization, token and userinfo endpoints are configured"
                        .to_string(),
                },
                ExternalAuthProviderTestCheck {
                    name: "authorization_code".to_string(),
                    success: true,
                    message:
                        "OAuth2 client credentials require a real authorization code to validate"
                            .to_string(),
                },
            ],
        })
    }
}

/// Builds the outbound HTTP client shared by Generic `OAuth2` and specialized
/// OAuth2-backed providers such as GitHub.
///
/// GitHub rejects API calls without a User-Agent header, so keep the project
/// user agent on the shared client instead of setting it per request.
pub(super) fn oauth2_http_client(
    provider: &ExternalAuthProviderConfig,
) -> Result<aster_forge_http::HttpClient> {
    if let Some(client) = &provider.outbound_http_client {
        return Ok(client.clone());
    }
    aster_forge_http::HttpClient::builder()
        .timeout(std::time::Duration::from_secs(TOKEN_ENDPOINT_TIMEOUT_SECS))
        .user_agent(outbound_http_user_agent(provider))
        .build()
        .map_external_auth_err_ctx(
            "failed to build OAuth2 HTTP client",
            ExternalAuthError::internal_error,
        )
}

pub(super) fn require_url<'a>(
    value: Option<&'a str>,
    field: &str,
    error_fn: fn(String) -> ExternalAuthError,
) -> Result<&'a str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error_fn(format!("OAuth2 provider missing {field}")))
}

pub(super) fn validate_url(
    value: &str,
    field: &str,
    error_fn: fn(String) -> ExternalAuthError,
) -> Result<Url> {
    let parsed = parse_url(value, &format!("invalid OAuth2 {field}"))
        .map_err(|error| error_fn(error.to_string()))?;
    if !aster_forge_utils::url::has_http_scheme(&parsed) {
        return Err(error_fn(format!(
            "unsupported URL scheme for OAuth2 {field}, only http/https allowed"
        )));
    }
    Ok(parsed)
}

fn build_pkce_verifier() -> String {
    let mut bytes = [0_u8; 32];
    let mut rng = rand::rng();
    rng.fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn build_pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

pub(super) async fn exchange_code_for_token(
    http_client: &aster_forge_http::HttpClient,
    provider: &ExternalAuthProviderConfig,
    code: &str,
    redirect_uri: &str,
    pkce_verifier: &str,
) -> Result<String> {
    let token_url = require_url(
        provider.token_url.as_deref(),
        "token_url",
        ExternalAuthError::config_error,
    )?;
    validate_url(token_url, "token_url", ExternalAuthError::config_error)?;

    let client_secret = provider
        .client_secret
        .as_deref()
        .map(str::trim)
        .filter(|secret| !secret.is_empty());
    let token_request = OAuth2TokenRequest {
        token_url,
        provider,
        code,
        redirect_uri,
        pkce_verifier,
        auth_method: OAuth2TokenAuthMethod::PublicClient,
        client_secret,
    };
    let response = if client_secret.is_some() {
        send_token_request(
            http_client,
            token_request.with_auth_method(OAuth2TokenAuthMethod::ClientSecretPost),
        )
        .await?
    } else {
        send_token_request(http_client, token_request).await?
    };
    if !response.status().is_success() {
        return Err(oauth2_endpoint_error(response, "OAuth2 token exchange").await);
    }
    let token_response = response
        .json::<OAuth2TokenResponse>()
        .await
        .map_external_auth_err_ctx(
            "OAuth2 token response is invalid",
            ExternalAuthError::auth_invalid_credentials,
        )?;
    if token_response.access_token.trim().is_empty() {
        return Err(ExternalAuthError::auth_invalid_credentials(
            "OAuth2 token response missing access_token",
        ));
    }
    if let Some(token_type) = token_response.token_type.as_deref()
        && !token_type.eq_ignore_ascii_case("bearer")
    {
        return Err(ExternalAuthError::auth_invalid_credentials(
            "OAuth2 token response returned unsupported token_type",
        ));
    }
    Ok(token_response.access_token)
}

async fn send_token_request(
    http_client: &aster_forge_http::HttpClient,
    token_request: OAuth2TokenRequest<'_>,
) -> Result<reqwest::Response> {
    let form = {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("grant_type", "authorization_code");
        serializer.append_pair("code", token_request.code);
        serializer.append_pair("redirect_uri", token_request.redirect_uri);
        serializer.append_pair("code_verifier", token_request.pkce_verifier);
        match token_request.auth_method {
            OAuth2TokenAuthMethod::ClientSecretPost => {
                serializer.append_pair("client_id", &token_request.provider.client_id);
                if let Some(secret) = token_request.client_secret {
                    serializer.append_pair("client_secret", secret);
                }
            }
            OAuth2TokenAuthMethod::PublicClient => {
                serializer.append_pair("client_id", &token_request.provider.client_id);
            }
        }
        serializer.finish()
    };

    let request = http_client
        .post(token_request.token_url)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form);

    request.send().await.map_external_auth_err_ctx(
        "OAuth2 token exchange failed",
        ExternalAuthError::auth_invalid_credentials,
    )
}

pub(super) async fn oauth2_endpoint_error(
    response: reqwest::Response,
    context: &str,
) -> ExternalAuthError {
    let status = response.status();
    let www_authenticate = response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .map(sanitize_error_fragment)
        .filter(|value| !value.is_empty());
    let provider_error = response.json::<OAuth2ErrorResponse>().await.ok();

    let mut details = Vec::new();
    if let Some(error) = provider_error
        .as_ref()
        .and_then(|body| body.error.as_deref())
        .map(sanitize_error_fragment)
        .filter(|error| !error.is_empty())
    {
        details.push(format!("error={error}"));
    }
    if let Some(description) = provider_error
        .as_ref()
        .and_then(|body| body.error_description.as_deref())
        .map(sanitize_error_fragment)
        .filter(|description| !description.is_empty())
    {
        details.push(format!("description={description}"));
    }
    if let Some(www_authenticate) = www_authenticate {
        details.push(format!("www-authenticate={www_authenticate}"));
    }

    if details.is_empty() {
        ExternalAuthError::auth_invalid_credentials(format!("{context} failed ({status})"))
    } else {
        ExternalAuthError::auth_invalid_credentials(format!(
            "{context} failed ({status}; {})",
            details.join("; ")
        ))
    }
}

fn sanitize_error_fragment(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(128)
        .collect::<String>()
        .trim()
        .to_string()
}

pub(super) async fn fetch_userinfo(
    http_client: &aster_forge_http::HttpClient,
    provider: &ExternalAuthProviderConfig,
    access_token: &str,
) -> Result<Value> {
    let userinfo_url = require_url(
        provider.userinfo_url.as_deref(),
        "userinfo_url",
        ExternalAuthError::config_error,
    )?;
    validate_url(
        userinfo_url,
        "userinfo_url",
        ExternalAuthError::config_error,
    )?;
    let response = http_client
        .get(userinfo_url)
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_external_auth_err_ctx(
            "OAuth2 userinfo request failed",
            ExternalAuthError::auth_invalid_credentials,
        )?;
    if !response.status().is_success() {
        return Err(oauth2_endpoint_error(response, "OAuth2 userinfo request").await);
    }
    response.json::<Value>().await.map_external_auth_err_ctx(
        "OAuth2 userinfo response is invalid",
        ExternalAuthError::auth_invalid_credentials,
    )
}

pub(super) fn profile_from_userinfo(
    provider: &ExternalAuthProviderConfig,
    userinfo: &Value,
) -> Result<ExternalAuthProfile> {
    let subject_claim = provider.subject_claim.as_deref().unwrap_or("sub");
    let subject = extract_claim_string(userinfo, subject_claim)
        .or_else(|| {
            if subject_claim == "sub" {
                extract_claim_string(userinfo, "id")
            } else {
                None
            }
        })
        .ok_or_else(|| {
            ExternalAuthError::auth_invalid_credentials("OAuth2 userinfo missing subject")
        })?;
    let subject = validate_required_claim(&subject, "OAuth2 subject", OAUTH2_SUBJECT_MAX_LEN)?;

    let email = extract_claim_string(userinfo, provider.email_claim.as_deref().unwrap_or("email"))
        .map(|email| email.trim().to_string())
        .filter(|email| !email.is_empty());
    if let Some(email) = email.as_deref() {
        aster_forge_validation::email::validate_email(email).map_err(|_| {
            ExternalAuthError::auth_invalid_credentials("OAuth2 email claim is invalid")
        })?;
    }

    Ok(ExternalAuthProfile {
        identity_namespace: identity_namespace(provider)?,
        subject,
        email,
        email_verified: extract_claim_bool(
            userinfo,
            provider
                .email_verified_claim
                .as_deref()
                .unwrap_or("email_verified"),
        )
        .unwrap_or(false),
        display_name: normalize_optional_snapshot(extract_claim_string(
            userinfo,
            provider.display_name_claim.as_deref().unwrap_or("name"),
        )),
        preferred_username: normalize_optional_snapshot(extract_claim_string(
            userinfo,
            provider
                .username_claim
                .as_deref()
                .unwrap_or("preferred_username"),
        )),
    })
}

fn identity_namespace(provider: &ExternalAuthProviderConfig) -> Result<String> {
    if let Some(issuer) = provider
        .issuer_url
        .as_deref()
        .map(str::trim)
        .filter(|issuer| !issuer.is_empty())
    {
        return validate_required_claim(issuer, "OAuth2 issuer", OAUTH2_NAMESPACE_MAX_LEN);
    }
    let authorization_url = require_url(
        provider.authorization_url.as_deref(),
        "authorization_url",
        ExternalAuthError::config_error,
    )?;
    let parsed = validate_url(
        authorization_url,
        "authorization_url",
        ExternalAuthError::config_error,
    )?;
    let origin = parsed.origin().ascii_serialization();
    validate_required_claim(&origin, "OAuth2 origin", OAUTH2_NAMESPACE_MAX_LEN)
}

fn extract_claim_string(value: &Value, claim: &str) -> Option<String> {
    extract_claim_value(value, claim).and_then(|value| match value {
        Value::String(value) => Some(value.trim().to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn extract_claim_bool(value: &Value, claim: &str) -> Option<bool> {
    extract_claim_value(value, claim).and_then(|value| match value {
        Value::Bool(value) => Some(*value),
        Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
        Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    })
}

fn extract_claim_value<'a>(value: &'a Value, claim: &str) -> Option<&'a Value> {
    let claim = claim.trim();
    if claim.is_empty() {
        return None;
    }
    if claim.starts_with('/') {
        return value.pointer(claim);
    }
    if let Some(found) = value.get(claim) {
        return Some(found);
    }
    claim
        .split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

fn validate_required_claim(value: &str, field: &str, max_len: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > max_len || value.chars().any(char::is_control) {
        return Err(ExternalAuthError::auth_invalid_credentials(format!(
            "{field} claim is invalid"
        )));
    }
    Ok(value.to_string())
}

fn truncate_to_utf8_boundary(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    let mut end = max_len;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn normalize_optional_snapshot(value: Option<String>) -> Option<String> {
    value
        .map(|value| {
            value
                .chars()
                .filter(|ch| !ch.is_control())
                .collect::<String>()
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| truncate_to_utf8_boundary(&value, OAUTH2_SNAPSHOT_MAX_LEN))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> ExternalAuthProviderConfig {
        ExternalAuthProviderConfig {
            id: 1,
            key: "generic".to_string(),
            provider_kind: ExternalAuthProviderKind::GenericOAuth2,
            protocol: ExternalAuthProtocol::OAuth2,
            options: crate::types::ExternalAuthProviderOptions::default(),
            issuer_url: None,
            authorization_url: Some("https://id.example.com/oauth/authorize".to_string()),
            token_url: Some("https://id.example.com/oauth/token".to_string()),
            userinfo_url: Some("https://id.example.com/oauth/userinfo".to_string()),
            client_id: "client".to_string(),
            client_secret: None,
            scopes: OAUTH2_DEFAULT_SCOPES.to_string(),
            subject_claim: None,
            username_claim: None,
            display_name_claim: None,
            email_claim: None,
            email_verified_claim: None,
            groups_claim: None,
            avatar_url_claim: None,
            outbound_http_user_agent: None,
            #[cfg(any(feature = "oauth2", feature = "oidc"))]
            outbound_http_client: None,
        }
    }

    #[test]
    fn profile_supports_json_pointer_and_dotted_claims() {
        let mut provider = provider();
        provider.subject_claim = Some("/user/id".to_string());
        provider.username_claim = Some("user.login".to_string());
        provider.email_claim = Some("mail.primary".to_string());
        provider.email_verified_claim = Some("mail.verified".to_string());
        let userinfo = serde_json::json!({
            "user": { "id": 123, "login": "octo" },
            "mail": { "primary": "octo@example.com", "verified": "true" },
            "name": "Octo Cat"
        });

        let profile = profile_from_userinfo(&provider, &userinfo).expect("profile should parse");

        assert_eq!(profile.subject, "123");
        assert_eq!(profile.email.as_deref(), Some("octo@example.com"));
        assert!(profile.email_verified);
        assert_eq!(profile.preferred_username.as_deref(), Some("octo"));
    }

    #[test]
    fn profile_defaults_unverified_when_claim_is_missing() {
        let userinfo = serde_json::json!({
            "id": "github-1",
            "email": "user@example.com"
        });

        let profile = profile_from_userinfo(&provider(), &userinfo).expect("profile should parse");

        assert_eq!(profile.subject, "github-1");
        assert!(!profile.email_verified);
    }

    #[test]
    fn validate_url_rejects_non_http_schemes() {
        let err = validate_url(
            "file:///tmp/token",
            "token_url",
            ExternalAuthError::config_error,
        )
        .expect_err("non-http OAuth2 URL should be rejected");

        assert!(
            err.to_string()
                .contains("unsupported URL scheme for OAuth2 token_url")
        );
    }

    #[test]
    fn pkce_verifier_uses_valid_rfc7636_shape() {
        let verifier = build_pkce_verifier();

        assert!(verifier.len() >= 43);
        assert!(verifier.len() <= 128);
        assert!(
            verifier
                .chars()
                .all(|ch| { ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~') })
        );
    }

    #[actix_web::test]
    async fn oauth2_http_client_uses_default_crate_user_agent_when_app_value_is_missing() {
        let user_agent = observed_user_agent(None).await;

        assert_eq!(user_agent.as_deref(), Some(crate::OUTBOUND_HTTP_USER_AGENT));
    }

    #[actix_web::test]
    async fn oauth2_http_client_uses_application_user_agent() {
        let user_agent = observed_user_agent(Some("AsterYggdrasil/0.1.0-beta.1")).await;

        assert_eq!(user_agent.as_deref(), Some("AsterYggdrasil/0.1.0-beta.1"));
    }

    #[actix_web::test]
    async fn oauth2_http_client_falls_back_for_blank_application_user_agent() {
        let user_agent = observed_user_agent(Some(" \t\n ")).await;

        assert_eq!(user_agent.as_deref(), Some(crate::OUTBOUND_HTTP_USER_AGENT));
    }

    async fn observed_user_agent(configured_user_agent: Option<&str>) -> Option<String> {
        use actix_web::{App, HttpRequest, HttpResponse, HttpServer, web};

        async fn echo_user_agent(req: HttpRequest) -> HttpResponse {
            let user_agent = req
                .headers()
                .get("User-Agent")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            HttpResponse::Ok().json(user_agent)
        }

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("listener should bind");
        let addr = listener
            .local_addr()
            .expect("listener local address should resolve");
        let server = HttpServer::new(|| App::new().route("/", web::get().to(echo_user_agent)))
            .listen(listener)
            .expect("server should listen")
            .run();
        let handle = server.handle();
        actix_web::rt::spawn(server);

        let mut provider = provider();
        provider.outbound_http_user_agent = configured_user_agent.map(str::to_string);
        let http_client = oauth2_http_client(&provider).expect("HTTP client should build");
        let response = http_client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("request should succeed");
        let user_agent = response
            .json::<Option<String>>()
            .await
            .expect("response should decode");

        handle.stop(true).await;
        user_agent
    }

    #[actix_web::test]
    async fn userinfo_error_includes_safe_provider_diagnostics() {
        use actix_web::{App, HttpResponse, HttpServer, web};

        async fn unauthorized_userinfo() -> HttpResponse {
            HttpResponse::Unauthorized()
                .append_header((
                    "WWW-Authenticate",
                    r#"Bearer error="insufficient_scope", error_description="missing openid""#,
                ))
                .json(serde_json::json!({
                    "error": "invalid_token",
                    "error_description": "missing openid scope"
                }))
        }

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("listener should bind");
        let addr = listener
            .local_addr()
            .expect("listener address should exist");
        let server =
            HttpServer::new(|| App::new().route("/userinfo", web::get().to(unauthorized_userinfo)))
                .listen(listener)
                .expect("mock server should listen")
                .run();
        let handle = server.handle();
        tokio::spawn(server);

        let mut provider = provider();
        provider.userinfo_url = Some(format!("http://127.0.0.1:{}/userinfo", addr.port()));
        let http_client = oauth2_http_client(&provider).expect("HTTP client should build");

        let error = fetch_userinfo(&http_client, &provider, "opaque-access-token")
            .await
            .expect_err("userinfo should fail");
        let message = error.to_string();

        assert!(message.contains("OAuth2 userinfo request failed (401 Unauthorized"));
        assert!(message.contains("error=invalid_token"));
        assert!(message.contains("description=missing openid scope"));
        assert!(message.contains("www-authenticate=Bearer"));

        handle.stop(true).await;
    }
}

#[cfg(test)]
mod injected_transport_tests {
    use super::*;
    /// Serve one token response and retain the complete request for protocol assertions.
    async fn token_endpoint(
        status: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let len = socket.read(&mut chunk).await.unwrap();
                assert!(len > 0);
                bytes.extend_from_slice(&chunk[..len]);
                let request = String::from_utf8_lossy(&bytes);
                if let Some(end) = request.find("\r\n\r\n") {
                    let length = request[..end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(bytes).unwrap()
        });
        (url, server)
    }

    #[tokio::test]
    async fn injected_token_exchange_preserves_form_authentication_and_validates_token_response() {
        for (secret, status, body, expected) in [
            (
                true,
                "200 OK",
                r#"{"access_token":"issued-token","token_type":"Bearer"}"#,
                Ok("issued-token"),
            ),
            (
                false,
                "200 OK",
                r#"{"access_token":"public-token"}"#,
                Ok("public-token"),
            ),
            (
                true,
                "200 OK",
                r#"{"access_token":" ","token_type":"Bearer"}"#,
                Err("missing access_token"),
            ),
            (
                true,
                "200 OK",
                r#"{"access_token":"token","token_type":"MAC"}"#,
                Err("unsupported token_type"),
            ),
            (
                true,
                "400 Bad Request",
                r#"{"error":"invalid_grant"}"#,
                Err("400 Bad Request; error=invalid_grant"),
            ),
        ] {
            let (url, server) = token_endpoint(status, body).await;
            let mut provider = crate::registry::tests_for_transport_config();
            provider.token_url = Some(url);
            provider.client_secret = secret.then(|| "request-secret".into());
            provider.outbound_http_client = Some(
                aster_forge_http::HttpClient::builder()
                    .no_proxy()
                    .user_agent("TokenProduct/42")
                    .build()
                    .unwrap(),
            );
            let client = oauth2_http_client(&provider).unwrap();
            let result = exchange_code_for_token(
                &client,
                &provider,
                "a+b &",
                "http://localhost/callback",
                "verifier",
            )
            .await;
            match (result, expected) {
                (Ok(token), Ok(expected)) => assert_eq!(token, expected),
                (Err(ExternalAuthError::InvalidCredentials(message)), Err(expected)) => {
                    assert!(
                        message.contains(expected),
                        "unexpected authentication failure: {message}"
                    );
                }
                other => panic!("unexpected token-exchange result: {other:?}"),
            }
            let request = server.await.unwrap();
            assert!(request.starts_with("POST /token HTTP/1.1"));
            assert!(request.contains("TokenProduct/42"));
            let form = request.split_once("\r\n\r\n").unwrap().1;
            let fields = url::form_urlencoded::parse(form.as_bytes())
                .collect::<std::collections::HashMap<_, _>>();
            assert_eq!(fields.get("code").unwrap(), "a+b &");
            assert_eq!(fields.get("code_verifier").unwrap(), "verifier");
            assert_eq!(fields.get("client_id").unwrap(), "client-id");
            assert_eq!(fields.get("grant_type").unwrap(), "authorization_code");
            assert_eq!(
                fields.get("redirect_uri").unwrap(),
                "http://localhost/callback"
            );
            if secret {
                assert_eq!(fields.get("client_secret").unwrap(), "request-secret");
            }
            assert_eq!(fields.contains_key("client_secret"), secret);
        }
    }

    #[test]
    fn invalid_fallback_ua_is_an_initialization_error_and_debug_redacts_credentials() {
        let mut provider = crate::registry::tests_for_transport_config();
        provider.outbound_http_user_agent = Some("bad\r\nua".into());
        assert!(oauth2_http_client(&provider).is_err());
        let debug = format!("{provider:?}");
        assert!(!debug.contains("client-secret"));
        assert!(debug.contains("REDACTED"));
    }

    #[tokio::test]
    async fn injected_transport_preserves_product_ua_and_redirect() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 2048];
            let len = socket.read(&mut bytes).await.unwrap();
            socket.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/forbidden\r\nContent-Length: 0\r\n\r\n").await.unwrap();
            String::from_utf8_lossy(&bytes[..len]).to_string()
        });
        let mut provider = crate::registry::tests_for_transport_config();
        provider.outbound_http_user_agent = Some("must-not-override-product".into());
        provider.outbound_http_client = Some(
            aster_forge_http::HttpClient::builder()
                .no_proxy()
                .user_agent("Product/42")
                .build()
                .unwrap(),
        );
        let client = oauth2_http_client(&provider).unwrap();
        let response = client.get(url).send().await.unwrap();
        assert_eq!(response.status(), 307);
        assert!(server.await.unwrap().contains("Product/42"));
    }

    /// Verify this provider's injection entrypoint retains the product deadline.
    #[tokio::test]
    async fn injected_transport_preserves_product_timeout() {
        let mut provider = crate::registry::tests_for_transport_config();
        provider.outbound_http_client = Some(
            aster_forge_http::HttpClient::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_millis(50))
                .build()
                .unwrap(),
        );
        let client = oauth2_http_client(&provider).unwrap();
        super::super::test_support::assert_injected_timeout(client).await;
    }
}
