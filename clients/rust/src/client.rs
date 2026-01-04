//! HTTP/REST client for ProximaDB
//!
//! This module provides the `ProximaClient` for connecting to a remote
//! ProximaDB server over HTTP/REST.

use crate::collection::CollectionHandle;
use crate::error::{ConfigError, NetworkError, ProximaError, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the ProximaDB client
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Server URL (e.g., "http://localhost:5678")
    pub url: String,
    /// Request timeout in milliseconds
    pub timeout_ms: u64,
    /// Number of retries for failed requests
    pub max_retries: u32,
    /// Optional API key for authentication
    pub api_key: Option<String>,
    /// Enable connection pooling
    pub pool_connections: bool,
    /// Maximum idle connections in pool
    pub max_idle_connections: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:5678".to_string(),
            timeout_ms: 30000,
            max_retries: 3,
            api_key: None,
            pool_connections: true,
            max_idle_connections: 10,
        }
    }
}

/// Builder for creating a ProximaClient
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    config: ClientConfig,
}

impl ClientBuilder {
    /// Create a new client builder with default configuration
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default(),
        }
    }

    /// Set the server URL
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.config.url = url.into();
        self
    }

    /// Set the request timeout in milliseconds
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.config.timeout_ms = timeout_ms;
        self
    }

    /// Set the maximum number of retries
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.config.max_retries = max_retries;
        self
    }

    /// Set the API key for authentication
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.config.api_key = Some(api_key.into());
        self
    }

    /// Enable or disable connection pooling
    pub fn pool_connections(mut self, enable: bool) -> Self {
        self.config.pool_connections = enable;
        self
    }

    /// Set maximum idle connections in pool
    pub fn max_idle_connections(mut self, max: usize) -> Self {
        self.config.max_idle_connections = max;
        self
    }

    /// Build the ProximaClient
    pub fn build(self) -> Result<ProximaClient> {
        ProximaClient::with_config(self.config)
    }

    /// Connect to the server (alias for build)
    pub fn connect(self) -> Result<ProximaClient> {
        self.build()
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// HTTP/REST client for ProximaDB
///
/// Provides methods for connecting to a remote ProximaDB server and
/// performing vector database operations.
///
/// # Example
///
/// ```rust,ignore
/// use proximadb_sdk::ProximaClient;
///
/// let client = ProximaClient::connect("http://localhost:5678")?;
///
/// // Create a collection
/// client.create_collection("embeddings")
///     .dimension(768)
///     .engine("sst")
///     .execute()?;
///
/// // Search with fluent API
/// let results = client.collection("embeddings")
///     .search()
///     .vector(&query_vector)
///     .top_k(10)
///     .execute()?;
/// ```
#[derive(Clone)]
pub struct ProximaClient {
    inner: Arc<ProximaClientInner>,
}

struct ProximaClientInner {
    config: ClientConfig,
    #[cfg(feature = "client")]
    http_client: reqwest::Client,
}

impl ProximaClient {
    /// Connect to a ProximaDB server at the given URL
    pub fn connect(url: impl Into<String>) -> Result<Self> {
        ClientBuilder::new().url(url).build()
    }

    /// Create a client with custom configuration
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        // Validate URL
        let _parsed_url = url::Url::parse(&config.url).map_err(|_| {
            ProximaError::Config(ConfigError::InvalidValue {
                field: "url".to_string(),
                reason: format!("Invalid URL: {}", config.url),
            })
        })?;

        #[cfg(feature = "client")]
        let http_client = {
            let mut builder = reqwest::Client::builder()
                .timeout(Duration::from_millis(config.timeout_ms))
                .pool_max_idle_per_host(config.max_idle_connections);

            if !config.pool_connections {
                builder = builder.pool_max_idle_per_host(0);
            }

            builder.build().map_err(|e| {
                ProximaError::Network(NetworkError::ConnectionFailed {
                    url: config.url.clone(),
                    reason: e.to_string(),
                })
            })?
        };

        Ok(Self {
            inner: Arc::new(ProximaClientInner {
                config,
                #[cfg(feature = "client")]
                http_client,
            }),
        })
    }

    /// Get a builder for creating a client
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Get a handle to a collection for fluent operations
    pub fn collection(&self, name: &str) -> CollectionHandle<'_> {
        CollectionHandle::new(self, name)
    }

    /// Get the server URL
    pub fn url(&self) -> &str {
        &self.inner.config.url
    }

    /// Get the client configuration
    pub fn config(&self) -> &ClientConfig {
        &self.inner.config
    }

    /// Check if the server is healthy
    #[cfg(feature = "client")]
    pub async fn health(&self) -> Result<HealthStatus> {
        let url = format!("{}/health", self.inner.config.url);
        let response = self.get(&url).await?;
        Ok(response)
    }

    /// Create a collection builder
    pub fn create_collection(&self, name: &str) -> crate::collection::CollectionBuilder<'_> {
        crate::collection::CollectionBuilder::new(self, name)
    }

    /// Delete a collection
    #[cfg(feature = "client")]
    pub async fn delete_collection(&self, name: &str) -> Result<()> {
        let url = format!("{}/api/v1/collections/{}", self.inner.config.url, name);
        self.delete::<serde_json::Value>(&url).await?;
        Ok(())
    }

    /// Get a handle to a graph for fluent operations
    #[cfg(feature = "client")]
    pub fn graph(&self, name: &str) -> crate::graph::GraphHandle<'_> {
        crate::graph::GraphHandle::new(self, name)
    }

    /// Create a graph builder
    #[cfg(feature = "client")]
    pub fn create_graph(&self, name: &str) -> crate::graph::GraphBuilder<'_> {
        crate::graph::GraphBuilder::new(self, name)
    }

    /// Delete a graph
    #[cfg(feature = "client")]
    pub async fn delete_graph(&self, name: &str) -> Result<()> {
        let url = format!("{}/api/v1/graphs/{}", self.inner.config.url, name);
        self.delete::<serde_json::Value>(&url).await?;
        Ok(())
    }

    /// List all graphs
    #[cfg(feature = "client")]
    pub async fn list_graphs(&self) -> Result<Vec<GraphInfo>> {
        let url = format!("{}/api/v1/graphs", self.inner.config.url);
        let response: ListGraphsResponse = self.get(&url).await?;
        Ok(response.graphs)
    }

    /// List all collections
    #[cfg(feature = "client")]
    pub async fn list_collections(&self) -> Result<Vec<CollectionInfo>> {
        let url = format!("{}/api/v1/collections", self.inner.config.url);
        let response: ListCollectionsResponse = self.get(&url).await?;
        Ok(response.collections)
    }

    // Internal HTTP methods

    #[cfg(feature = "client")]
    pub(crate) async fn get<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        let mut request = self.inner.http_client.get(url);

        if let Some(ref api_key) = self.inner.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.send().await?;
        self.handle_response(response).await
    }

    #[cfg(feature = "client")]
    pub(crate) async fn post<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        let mut request = self.inner.http_client.post(url).json(body);

        if let Some(ref api_key) = self.inner.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.send().await?;
        self.handle_response(response).await
    }

    #[cfg(feature = "client")]
    pub(crate) async fn delete<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        let mut request = self.inner.http_client.delete(url);

        if let Some(ref api_key) = self.inner.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.send().await?;
        self.handle_response(response).await
    }

    #[cfg(feature = "client")]
    async fn handle_response<T: for<'de> Deserialize<'de>>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();

        if status.is_success() {
            let body = response.json::<T>().await.map_err(|e| {
                ProximaError::Network(NetworkError::Deserialization {
                    reason: e.to_string(),
                })
            })?;
            Ok(body)
        } else {
            let status_code = status.as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            match status_code {
                401 => Err(ProximaError::Network(NetworkError::AuthenticationFailed {
                    reason: message,
                })),
                429 => Err(ProximaError::Network(NetworkError::RateLimited {
                    retry_after_ms: 1000,
                })),
                _ => Err(ProximaError::Network(NetworkError::HttpError {
                    status: status_code,
                    message,
                })),
            }
        }
    }
}

impl std::fmt::Debug for ProximaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProximaClient")
            .field("url", &self.inner.config.url)
            .field("timeout_ms", &self.inner.config.timeout_ms)
            .finish()
    }
}

/// Health status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Server status
    pub status: String,
    /// Server version
    #[serde(default)]
    pub version: Option<String>,
    /// Uptime in seconds
    #[serde(default)]
    pub uptime_seconds: Option<u64>,
}

/// Collection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionInfo {
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Number of vectors
    #[serde(default)]
    pub vector_count: u64,
    /// Storage engine type
    #[serde(default)]
    pub engine: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListCollectionsResponse {
    collections: Vec<CollectionInfo>,
}

/// Graph information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphInfo {
    /// Graph name
    pub name: String,
    /// Number of nodes
    #[serde(default)]
    pub node_count: u64,
    /// Number of edges
    #[serde(default)]
    pub edge_count: u64,
    /// Graph description
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListGraphsResponse {
    graphs: Vec<GraphInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_builder() {
        let builder = ClientBuilder::new()
            .url("http://localhost:5678")
            .timeout_ms(5000)
            .max_retries(5)
            .api_key("test-key");

        assert_eq!(builder.config.url, "http://localhost:5678");
        assert_eq!(builder.config.timeout_ms, 5000);
        assert_eq!(builder.config.max_retries, 5);
        assert_eq!(builder.config.api_key, Some("test-key".to_string()));
    }

    #[test]
    fn test_client_connect() {
        let client = ProximaClient::connect("http://localhost:5678");
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.url(), "http://localhost:5678");
    }

    #[test]
    fn test_invalid_url() {
        let result = ProximaClient::connect("not-a-valid-url");
        assert!(result.is_err());
    }
}
