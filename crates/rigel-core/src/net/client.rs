//! Reusable HTTP clients with bounded keep-alive pools and named phase deadlines.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use thiserror::Error;

/// Per-phase deadlines applied by callers around network operations.
#[derive(Debug, Clone, Copy)]
pub struct ClientConfig {
    pub pool_max_idle_per_host: usize,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub body_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            pool_max_idle_per_host: 8,
            connect_timeout: Duration::from_secs(10),
            response_timeout: Duration::from_secs(30),
            body_timeout: Duration::from_secs(60),
        }
    }
}

/// A network phase that can independently expire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpPhase {
    Connect,
    Response,
    Body,
}

impl std::fmt::Display for HttpPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Connect => "connect",
            Self::Response => "response",
            Self::Body => "body",
        })
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP {phase} phase timed out")]
    Timeout { phase: HttpPhase },
    #[error("failed to build HTTP client: {0}")]
    Build(#[from] reqwest::Error),
}

/// A provider's reusable reqwest client.
#[derive(Debug, Clone)]
pub struct ProviderClient {
    client: reqwest::Client,
    config: ClientConfig,
}

impl ProviderClient {
    fn build(config: ClientConfig) -> Result<Self, ClientError> {
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .pool_idle_timeout(Duration::from_secs(90))
            .connect_timeout(config.connect_timeout)
            .build()?;
        Ok(Self { client, config })
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn config(&self) -> ClientConfig {
        self.config
    }

    pub async fn run_phase<T, F>(&self, phase: HttpPhase, work: F) -> Result<T, ClientError>
    where
        F: Future<Output = T>,
    {
        let timeout = match phase {
            HttpPhase::Connect => self.config.connect_timeout,
            HttpPhase::Response => self.config.response_timeout,
            HttpPhase::Body => self.config.body_timeout,
        };
        tokio::time::timeout(timeout, work)
            .await
            .map_err(|_| ClientError::Timeout { phase })
    }
}

/// Caches exactly one configured HTTP client for each provider key.
#[derive(Debug, Default)]
pub struct ClientRegistry {
    clients: Mutex<HashMap<String, Arc<ProviderClient>>>,
}

impl ClientRegistry {
    pub fn get(&self, provider: &str) -> Result<Arc<ProviderClient>, ClientError> {
        self.get_or_configure(provider, ClientConfig::default())
    }

    pub fn get_or_configure(
        &self,
        provider: &str,
        config: ClientConfig,
    ) -> Result<Arc<ProviderClient>, ClientError> {
        let mut clients = self
            .clients
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(client) = clients.get(provider) {
            return Ok(Arc::clone(client));
        }
        let client = Arc::new(ProviderClient::build(config)?);
        clients.insert(provider.to_owned(), Arc::clone(&client));
        Ok(client)
    }
}

/// Process-wide registry used by provider adapters to share their connection pools.
pub fn shared_provider_client(provider: &str) -> Result<Arc<ProviderClient>, ClientError> {
    static REGISTRY: OnceLock<ClientRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ClientRegistry::default).get(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn registry_reuses_client_and_http_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepted_server = Arc::clone(&accepted);
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            accepted_server.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            for _ in 0..2 {
                let mut request = Vec::new();
                let mut byte = [0; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            }
        });

        let registry = ClientRegistry::default();
        let first = registry.get("test-provider").unwrap();
        let second = registry.get("test-provider").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        for _ in 0..2 {
            let response = first
                .client()
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.bytes().await.unwrap().as_ref(), b"ok");
        }
        server.await.unwrap();
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_error_names_the_expired_phase() {
        let registry = ClientRegistry::default();
        let client = registry
            .get_or_configure(
                "slow",
                ClientConfig {
                    response_timeout: Duration::from_millis(1),
                    ..ClientConfig::default()
                },
            )
            .unwrap();

        let error = client
            .run_phase(HttpPhase::Response, std::future::pending::<()>())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "HTTP response phase timed out");
    }
}
