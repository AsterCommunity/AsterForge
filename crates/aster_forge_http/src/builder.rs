//! The only reqwest pool construction point. No arbitrary redirect/TLS bypass hooks.
use crate::{HttpClient, HttpObserver};
use std::{net::SocketAddr, sync::Arc, time::Duration};

/// Product-neutral client assembly. Redirects and automatic retries are always disabled.
/// Transport retries and protocol redirects must be explicitly implemented by the caller.
#[must_use]
pub struct HttpClientBuilder {
    user_agent: String,
    connect_timeout: Duration,
    read_timeout: Option<Duration>,
    timeout: Option<Duration>,
    idle_timeout: Duration,
    idle_per_host: usize,
    proxy: ProxyPolicy,
    roots: Vec<reqwest::Certificate>,
    pinned: Vec<(String, Vec<SocketAddr>)>,
    observer: Option<Arc<dyn HttpObserver>>,
}
enum ProxyPolicy {
    System,
    Disabled,
    Explicit(Box<reqwest::Proxy>),
}
impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            user_agent: concat!("aster_forge_http/", env!("CARGO_PKG_VERSION")).into(),
            connect_timeout: Duration::from_secs(5),
            read_timeout: Some(Duration::from_secs(30)),
            timeout: Some(Duration::from_secs(30)),
            idle_timeout: Duration::from_secs(90),
            idle_per_host: 8,
            proxy: ProxyPolicy::System,
            roots: Vec::new(),
            pinned: Vec::new(),
            observer: None,
        }
    }
}
impl HttpClientBuilder {
    pub fn user_agent(mut self, value: impl Into<String>) -> Self {
        self.user_agent = value.into();
        self
    }
    pub fn connect_timeout(mut self, value: Duration) -> Self {
        self.connect_timeout = value;
        self
    }
    pub fn read_timeout(mut self, value: Duration) -> Self {
        self.read_timeout = Some(value);
        self
    }
    pub fn timeout(mut self, value: Duration) -> Self {
        self.timeout = Some(value);
        self
    }
    /// Stream owners must supply their own cancellation/progress boundary when disabling a timer.
    pub fn without_total_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }
    pub fn without_read_timeout(mut self) -> Self {
        self.read_timeout = None;
        self
    }
    pub fn pool_idle_timeout(mut self, value: Duration) -> Self {
        self.idle_timeout = value;
        self
    }
    pub fn pool_max_idle_per_host(mut self, value: usize) -> Self {
        self.idle_per_host = value;
        self
    }
    pub fn no_proxy(mut self) -> Self {
        self.proxy = ProxyPolicy::Disabled;
        self
    }
    /// Explicit proxy ignores environment proxy variables.
    pub fn proxy(mut self, value: reqwest::Proxy) -> Self {
        self.proxy = ProxyPolicy::Explicit(Box::new(value));
        self
    }
    pub fn add_root_certificate(mut self, value: reqwest::Certificate) -> Self {
        self.roots.push(value);
        self
    }
    /// A proxy would bypass local DNS pinning, so pinned destinations always use direct connections.
    pub fn resolve_to_addrs(mut self, host: &str, addrs: &[SocketAddr]) -> Self {
        self.pinned.push((host.to_owned(), addrs.to_vec()));
        self.proxy = ProxyPolicy::Disabled;
        self
    }
    pub fn observer(mut self, value: Arc<dyn HttpObserver>) -> Self {
        self.observer = Some(value);
        self
    }

    /// # Errors
    /// Returns a reqwest configuration error before any request is sent.
    pub fn build(self) -> Result<HttpClient, reqwest::Error> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(self.user_agent)
            .connect_timeout(self.connect_timeout)
            .pool_idle_timeout(self.idle_timeout)
            .pool_max_idle_per_host(self.idle_per_host);
        if let Some(value) = self.read_timeout {
            builder = builder.read_timeout(value);
        }
        if let Some(value) = self.timeout {
            builder = builder.timeout(value);
        }
        builder = match self.proxy {
            ProxyPolicy::System => builder,
            ProxyPolicy::Disabled => builder.no_proxy(),
            ProxyPolicy::Explicit(proxy) => builder.no_proxy().proxy(*proxy),
        };
        if !self.pinned.is_empty() {
            builder = builder.no_proxy();
        }
        for certificate in self.roots {
            builder = builder.add_root_certificate(certificate);
        }
        for (host, addrs) in self.pinned {
            builder = builder.resolve_to_addrs(&host, &addrs);
        }
        let client = builder.build()?;
        Ok(match self.observer {
            Some(observer) => HttpClient::observed(client, observer),
            None => client.into(),
        })
    }

    /// Blocking transport for synchronous SDK exporters. Construct/drop outside an async runtime.
    /// It shares UA/redirect/proxy/TLS/pool policy. The blocking SDK owns observation and body I/O;
    /// async observers and idle-read timers are not installed on this boundary.
    /// # Errors
    /// Returns a reqwest configuration error before any request is sent.
    #[cfg(feature = "blocking")]
    pub fn build_blocking(self) -> Result<reqwest::blocking::Client, reqwest::Error> {
        let mut builder = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(self.user_agent)
            .connect_timeout(self.connect_timeout)
            .pool_idle_timeout(self.idle_timeout)
            .pool_max_idle_per_host(self.idle_per_host);
        if let Some(value) = self.timeout {
            builder = builder.timeout(value);
        }
        builder = match self.proxy {
            ProxyPolicy::System => builder,
            ProxyPolicy::Disabled => builder.no_proxy(),
            ProxyPolicy::Explicit(proxy) => builder.no_proxy().proxy(*proxy),
        };
        if !self.pinned.is_empty() {
            builder = builder.no_proxy();
        }
        for certificate in self.roots {
            builder = builder.add_root_certificate(certificate);
        }
        for (host, addrs) in self.pinned {
            builder = builder.resolve_to_addrs(&host, &addrs);
        }
        builder.build()
    }
}
