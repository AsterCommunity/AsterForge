//! Shared runtime CORS policy, preflight validation and response headers.
/// Origin list accepted by a runtime CORS policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsAllowedOrigins {
    /// No active CORS policy: requests pass through without adding CORS headers.
    None,
    /// Every origin is accepted.
    Any,
    /// Only the listed normalized origins are accepted.
    List(Vec<String>),
}

/// Product-neutral runtime CORS policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCorsPolicy {
    /// Whether CORS processing is enabled.
    pub enabled: bool,
    /// Origins accepted when CORS processing is enabled.
    pub allowed_origins: CorsAllowedOrigins,
    /// Whether credentials are allowed.
    pub allow_credentials: bool,
    /// Browser preflight cache duration.
    pub max_age_secs: u64,
}

impl RuntimeCorsPolicy {
    /// Returns whether requests should be actively checked.
    #[must_use]
    pub fn enforces_requests(&self) -> bool {
        self.enabled && !matches!(self.allowed_origins, CorsAllowedOrigins::None)
    }

    /// Returns whether a normalized origin is allowed.
    #[must_use]
    pub fn allows_origin(&self, origin: &str) -> bool {
        match &self.allowed_origins {
            CorsAllowedOrigins::None => false,
            CorsAllowedOrigins::Any => true,
            CorsAllowedOrigins::List(origins) => origins.iter().any(|allowed| allowed == origin),
        }
    }

    /// Returns whether responses should use `Access-Control-Allow-Origin: *`.
    #[must_use]
    pub fn sends_wildcard_origin(&self) -> bool {
        matches!(self.allowed_origins, CorsAllowedOrigins::Any) && !self.allow_credentials
    }
}

/// CORS middleware failure category for product error mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsMiddlewareErrorKind {
    /// The incoming request contains an invalid origin or preflight header.
    InvalidRequest,
    /// A response header produced or inherited by the middleware is invalid.
    InvalidResponse,
}

/// Product-neutral CORS middleware error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CorsMiddlewareError {
    kind: CorsMiddlewareErrorKind,
    message: String,
}

impl CorsMiddlewareError {
    #[cfg(any(feature = "actix", feature = "axum"))]
    pub(crate) fn new(kind: CorsMiddlewareErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the failure category.
    #[must_use]
    pub const fn kind(&self) -> CorsMiddlewareErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(any(feature = "actix", feature = "axum"))]
mod engine {
    use super::{CorsMiddlewareError, CorsMiddlewareErrorKind, RuntimeCorsPolicy};
    use http::{HeaderMap, HeaderValue, header};
    use std::collections::BTreeSet;
    /// Static CORS method/header lists; runtime origin policy is resolved per request.
    #[derive(Clone, Default)]
    pub(crate) struct CorsSettings {
        pub allowed_methods: Vec<&'static str>,
        pub allowed_headers: Vec<&'static str>,
        pub exposed_headers: Vec<&'static str>,
        pub additional_origin_schemes: Vec<&'static str>,
    }

    pub(crate) enum CorsDecision {
        Pass,
        Reject,
        Preflight(HeaderMap),
        Actual(String),
    }

    pub(crate) fn evaluate(
        method: &str,
        headers: &HeaderMap,
        request_origin: Option<&str>,
        policy: &RuntimeCorsPolicy,
        config: &CorsSettings,
    ) -> Result<CorsDecision, CorsMiddlewareError> {
        let Some(origin) = headers.get(header::ORIGIN) else {
            return Ok(CorsDecision::Pass);
        };
        if !policy.enforces_requests() {
            return Ok(CorsDecision::Pass);
        }
        let origin = origin.to_str().map_err(|_| {
            CorsMiddlewareError::new(
                CorsMiddlewareErrorKind::InvalidRequest,
                "invalid Origin header",
            )
        })?;
        let origin = aster_forge_utils::url::normalize_origin_with_additional_schemes(
            origin,
            false,
            &config.additional_origin_schemes,
        )
        .map_err(|error| {
            CorsMiddlewareError::new(CorsMiddlewareErrorKind::InvalidRequest, error.to_string())
        })?;
        if request_origin == Some(origin.as_str()) {
            return Ok(CorsDecision::Pass);
        }
        if !policy.allows_origin(&origin) {
            return Ok(CorsDecision::Reject);
        }
        if method == "OPTIONS" && headers.contains_key(header::ACCESS_CONTROL_REQUEST_METHOD) {
            let allowed_method = headers
                .get(header::ACCESS_CONTROL_REQUEST_METHOD)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|method| config.allowed_methods.contains(&method));
            if !allowed_method || !requested_headers_are_allowed(headers, config)? {
                return Ok(CorsDecision::Reject);
            }
            let mut response = HeaderMap::new();
            apply_origin_headers(&mut response, policy, &origin)?;
            apply_preflight_headers(&mut response, policy, config)?;
            return Ok(CorsDecision::Preflight(response));
        }
        Ok(CorsDecision::Actual(origin))
    }
    fn requested_headers_are_allowed(
        headers: &HeaderMap,
        config: &CorsSettings,
    ) -> Result<bool, CorsMiddlewareError> {
        let Some(request_headers) = headers.get(header::ACCESS_CONTROL_REQUEST_HEADERS) else {
            return Ok(true);
        };

        let request_headers = request_headers.to_str().map_err(|_| {
            CorsMiddlewareError::new(
                CorsMiddlewareErrorKind::InvalidRequest,
                "invalid Access-Control-Request-Headers",
            )
        })?;

        // Requested header names are lowercased before comparison (browsers send
        // them that way), so the configured names must be normalized too —
        // otherwise a configured "Content-Type" would never match a preflight.
        let allowed_headers = config
            .allowed_headers
            .iter()
            .map(|header| header.to_ascii_lowercase())
            .collect::<BTreeSet<String>>();

        for requested in request_headers.split(',') {
            let requested = requested.trim().to_ascii_lowercase();
            if requested.is_empty() {
                continue;
            }

            let parsed: Result<header::HeaderName, _> = requested.parse();
            if parsed.is_err() {
                return Err(CorsMiddlewareError::new(
                    CorsMiddlewareErrorKind::InvalidRequest,
                    "invalid Access-Control-Request-Headers",
                ));
            }

            if !allowed_headers.contains(requested.as_str()) {
                return Ok(false);
            }
        }

        Ok(true)
    }

    pub(crate) fn apply_origin_headers(
        headers: &mut HeaderMap,
        policy: &RuntimeCorsPolicy,
        origin: &str,
    ) -> Result<(), CorsMiddlewareError> {
        if !headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN) {
            let value = if policy.sends_wildcard_origin() {
                HeaderValue::from_static("*")
            } else {
                header_value(origin, "failed to serialize Access-Control-Allow-Origin")?
            };

            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        }

        if policy.allow_credentials
            && !headers.contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
        {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }

        ensure_vary(headers, "Origin")?;
        Ok(())
    }

    fn apply_preflight_headers(
        headers: &mut HeaderMap,
        policy: &RuntimeCorsPolicy,
        config: &CorsSettings,
    ) -> Result<(), CorsMiddlewareError> {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            header_value(
                &config.allowed_methods.join(", "),
                "failed to serialize Access-Control-Allow-Methods",
            )?,
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            header_value(
                &config.allowed_headers.join(", "),
                "failed to serialize Access-Control-Allow-Headers",
            )?,
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            header_value(
                &policy.max_age_secs.to_string(),
                "failed to serialize Access-Control-Max-Age",
            )?,
        );
        ensure_vary(headers, "Access-Control-Request-Method")?;
        ensure_vary(headers, "Access-Control-Request-Headers")?;
        Ok(())
    }

    pub(crate) fn apply_actual_headers(
        headers: &mut HeaderMap,
        config: &CorsSettings,
    ) -> Result<(), CorsMiddlewareError> {
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            header_value(
                &config.exposed_headers.join(", "),
                "failed to serialize Access-Control-Expose-Headers",
            )?,
        );
        Ok(())
    }

    fn ensure_vary(headers: &mut HeaderMap, value: &str) -> Result<(), CorsMiddlewareError> {
        let mut values = std::collections::BTreeMap::new();
        for existing in headers.get_all(header::VARY) {
            let existing = existing.to_str().map_err(|_| {
                CorsMiddlewareError::new(
                    CorsMiddlewareErrorKind::InvalidResponse,
                    "invalid Vary header",
                )
            })?;
            for item in existing
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                values
                    .entry(item.to_ascii_lowercase())
                    .or_insert_with(|| item.to_string());
            }
        }
        if values.contains_key("*") {
            return Ok(());
        }
        values
            .entry(value.to_ascii_lowercase())
            .or_insert_with(|| value.to_string());
        let joined = values.into_values().collect::<Vec<_>>().join(", ");
        headers.insert(
            header::VARY,
            header_value(&joined, "failed to serialize Vary header")?,
        );
        Ok(())
    }

    fn header_value(
        value: &str,
        error_message: &'static str,
    ) -> Result<HeaderValue, CorsMiddlewareError> {
        HeaderValue::from_str(value).map_err(|_| {
            CorsMiddlewareError::new(CorsMiddlewareErrorKind::InvalidResponse, error_message)
        })
    }
}

#[cfg(any(feature = "actix", feature = "axum"))]
pub(crate) use engine::{
    CorsDecision, CorsSettings, apply_actual_headers, apply_origin_headers, evaluate,
};
