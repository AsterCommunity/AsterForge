//! Execution and observation mechanics; the product owns client policy.
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Completion of the request exchange through response headers, not body EOF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpOutcome {
    Response,
    Timeout,
    Connect,
    Transport,
    Cancelled,
}

/// Deliberately excludes URL, headers, credentials and response bodies.
#[derive(Clone, Debug)]
pub struct HttpObservation {
    pub method: reqwest::Method,
    pub status: Option<u16>,
    pub outcome: HttpOutcome,
    pub duration: Duration,
}

/// Product-owned, non-blocking telemetry sink. Implementations must not panic.
pub trait HttpObserver: Send + Sync {
    fn observe(&self, observation: HttpObservation);
}

/// A shared connection pool with one execution/observation boundary.
/// No deref or raw-client accessor: request builders cannot bypass execution.
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    observer: Option<Arc<dyn HttpObserver>>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient").finish_non_exhaustive()
    }
}

impl From<reqwest::Client> for HttpClient {
    fn from(client: reqwest::Client) -> Self {
        Self {
            client,
            observer: None,
        }
    }
}

impl HttpClient {
    /// Safe shared defaults; product overrides are explicit.
    pub fn builder() -> super::HttpClientBuilder {
        super::HttpClientBuilder::default()
    }
    /// The caller supplies an already configured pool and its policy-scoped observer.
    pub fn observed(client: reqwest::Client, observer: Arc<dyn HttpObserver>) -> Self {
        Self {
            client,
            observer: Some(observer),
        }
    }

    pub fn request(
        &self,
        method: reqwest::Method,
        url: impl reqwest::IntoUrl,
    ) -> HttpRequestBuilder {
        HttpRequestBuilder {
            client: self.clone(),
            builder: self.client.request(method, url),
        }
    }

    pub fn get(&self, url: impl reqwest::IntoUrl) -> HttpRequestBuilder {
        self.request(reqwest::Method::GET, url)
    }
    pub fn post(&self, url: impl reqwest::IntoUrl) -> HttpRequestBuilder {
        self.request(reqwest::Method::POST, url)
    }
    pub fn put(&self, url: impl reqwest::IntoUrl) -> HttpRequestBuilder {
        self.request(reqwest::Method::PUT, url)
    }
    pub fn delete(&self, url: impl reqwest::IntoUrl) -> HttpRequestBuilder {
        self.request(reqwest::Method::DELETE, url)
    }
    pub fn head(&self, url: impl reqwest::IntoUrl) -> HttpRequestBuilder {
        self.request(reqwest::Method::HEAD, url)
    }
    pub fn patch(&self, url: impl reqwest::IntoUrl) -> HttpRequestBuilder {
        self.request(reqwest::Method::PATCH, url)
    }

    /// Executes the exact request, including caller-signed headers and streaming body.
    ///
    /// # Errors
    /// Returns the underlying transport error without reserializing the request.
    pub async fn execute(
        &self,
        request: reqwest::Request,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let mut guard = ObservationGuard {
            observer: self.observer.clone(),
            observation: HttpObservation {
                method: request.method().clone(),
                status: None,
                outcome: HttpOutcome::Cancelled,
                duration: Duration::ZERO,
            },
            started: Instant::now(),
        };
        let result = self.client.execute(request).await;
        match &result {
            Ok(response) => {
                guard.observation.status = Some(response.status().as_u16());
                guard.observation.outcome = HttpOutcome::Response;
            }
            Err(error) => {
                guard.observation.outcome = if error.is_timeout() {
                    HttpOutcome::Timeout
                } else if error.is_connect() {
                    HttpOutcome::Connect
                } else {
                    HttpOutcome::Transport
                };
            }
        }
        result
    }
}

struct ObservationGuard {
    observer: Option<Arc<dyn HttpObserver>>,
    observation: HttpObservation,
    started: Instant,
}
impl Drop for ObservationGuard {
    fn drop(&mut self) {
        if let Some(observer) = &self.observer {
            self.observation.duration = self.started.elapsed();
            observer.observe(self.observation.clone());
        }
    }
}

/// Request preparation stays compatible with reqwest; sending always returns to `HttpClient`.
#[derive(Debug)]
#[must_use]
pub struct HttpRequestBuilder {
    client: HttpClient,
    builder: reqwest::RequestBuilder,
}
impl HttpRequestBuilder {
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        reqwest::header::HeaderName: TryFrom<K>,
        <reqwest::header::HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        reqwest::header::HeaderValue: TryFrom<V>,
        <reqwest::header::HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.builder = self.builder.header(key, value);
        self
    }
    pub fn headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.builder = self.builder.headers(headers);
        self
    }
    pub fn body(mut self, body: impl Into<reqwest::Body>) -> Self {
        self.builder = self.builder.body(body);
        self
    }
    pub fn json<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
        self.builder = self.builder.json(value);
        self
    }
    pub fn form<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
        self.builder = self.builder.form(value);
        self
    }
    pub fn query<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
        self.builder = self.builder.query(value);
        self
    }
    pub fn multipart(mut self, form: reqwest::multipart::Form) -> Self {
        self.builder = self.builder.multipart(form);
        self
    }
    pub fn bearer_auth(mut self, token: impl std::fmt::Display) -> Self {
        self.builder = self.builder.bearer_auth(token);
        self
    }
    pub fn basic_auth<U: std::fmt::Display, P: std::fmt::Display>(
        mut self,
        username: U,
        password: Option<P>,
    ) -> Self {
        self.builder = self.builder.basic_auth(username, password);
        self
    }
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.builder = self.builder.timeout(timeout);
        self
    }
    pub fn version(mut self, version: http::Version) -> Self {
        self.builder = self.builder.version(version);
        self
    }
    pub fn try_clone(&self) -> Option<Self> {
        Some(Self {
            client: self.client.clone(),
            builder: self.builder.try_clone()?,
        })
    }

    /// Exposes the prepared request for protocol signing, never the underlying client.
    /// # Errors
    /// Returns reqwest's request preparation error.
    pub fn build(self) -> Result<reqwest::Request, reqwest::Error> {
        self.builder.build()
    }

    /// # Errors
    /// Returns request preparation or transport errors.
    pub async fn send(self) -> Result<reqwest::Response, reqwest::Error> {
        self.client.execute(self.builder.build()?).await
    }
}
