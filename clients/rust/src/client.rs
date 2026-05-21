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
        let url = format!("{}/api/v2/collections/{}", self.inner.config.url, name);
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
        let url = format!("{}/api/v2/collections", self.inner.config.url);
        let response: ListCollectionsResponse = self.get(&url).await?;
        Ok(response.collections)
    }

    // Internal HTTP methods

    #[cfg(feature = "client")]
    pub(crate) async fn get<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        let mut request = self.inner.http_client.get(url);

        if let Some(ref api_key) = self.inner.config.api_key {
            request = request.header("Authorization", format!("Bearer {api_key}"));
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
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }

        let response = request.send().await?;
        self.handle_response(response).await
    }

    #[cfg(feature = "client")]
    pub(crate) async fn delete<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        let mut request = self.inner.http_client.delete(url);

        if let Some(ref api_key) = self.inner.config.api_key {
            request = request.header("Authorization", format!("Bearer {api_key}"));
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

#[cfg(all(test, feature = "client"))]
impl ProximaClient {
    pub(crate) fn for_tests(url: &str) -> Self {
        let config = ClientConfig {
            url: url.to_string(),
            ..ClientConfig::default()
        };
        let http_client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(config.timeout_ms))
            .pool_max_idle_per_host(config.max_idle_connections)
            .build()
            .expect("test client should build without system proxy lookup");

        Self {
            inner: Arc::new(ProximaClientInner {
                config,
                http_client,
            }),
        }
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
    /// Collection ID
    #[serde(default)]
    pub collection_id: Option<String>,
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Number of vectors
    #[serde(default, alias = "record_count")]
    pub vector_count: u64,
    /// Storage engine type
    #[serde(default)]
    pub engine: Option<String>,
    /// Nested v2 collection statistics.
    #[serde(default)]
    pub stats: Option<CollectionStats>,
}

/// v2 collection statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStats {
    /// Total number of records.
    #[serde(default)]
    pub record_count: u64,
    /// Total storage size in bytes.
    #[serde(default)]
    pub storage_size_bytes: u64,
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
    use serde_json::json;

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
        let client = if std::env::var_os("LLVM_PROFILE_FILE").is_some() {
            ProximaClient::for_tests("http://localhost:5678")
        } else {
            ProximaClient::connect("http://localhost:5678").unwrap()
        };
        assert_eq!(client.url(), "http://localhost:5678");
    }

    #[test]
    fn test_invalid_url() {
        let result = ProximaClient::connect("not-a-valid-url");
        assert!(result.is_err());
    }

    #[test]
    fn default_client_config_targets_local_server_with_pooling() {
        let config = ClientConfig::default();

        assert_eq!(config.url, "http://localhost:5678");
        assert_eq!(config.timeout_ms, 30000);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.api_key, None);
        assert!(config.pool_connections);
        assert_eq!(config.max_idle_connections, 10);
    }

    #[test]
    fn builder_applies_pooling_and_retry_options_to_client_config() {
        let client = ProximaClient::builder()
            .url("https://db.example.com")
            .timeout_ms(1500)
            .max_retries(7)
            .api_key("secret")
            .pool_connections(false)
            .max_idle_connections(2)
            .connect()
            .unwrap();

        assert_eq!(client.url(), "https://db.example.com");
        assert_eq!(client.config().timeout_ms, 1500);
        assert_eq!(client.config().max_retries, 7);
        assert_eq!(client.config().api_key.as_deref(), Some("secret"));
        assert!(!client.config().pool_connections);
        assert_eq!(client.config().max_idle_connections, 2);
    }

    #[test]
    fn invalid_url_returns_config_error_with_url_field() {
        let result = ProximaClient::with_config(ClientConfig {
            url: "not a url".to_string(),
            ..ClientConfig::default()
        });

        assert!(matches!(
            result.unwrap_err(),
            ProximaError::Config(ConfigError::InvalidValue { field, .. }) if field == "url"
        ));
    }

    #[test]
    fn debug_output_exposes_url_and_timeout_only() {
        let client = ProximaClient::builder()
            .url("https://db.example.com")
            .timeout_ms(2500)
            .api_key("secret")
            .build()
            .unwrap();

        let debug = format!("{client:?}");

        assert!(debug.contains("https://db.example.com"));
        assert!(debug.contains("2500"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn health_status_deserializes_optional_fields() {
        let status: HealthStatus = serde_json::from_value(json!({"status": "ok"})).unwrap();

        assert_eq!(status.status, "ok");
        assert_eq!(status.version, None);
        assert_eq!(status.uptime_seconds, None);

        let status: HealthStatus =
            serde_json::from_value(json!({"status": "ok", "version": "1.0", "uptime_seconds": 42}))
                .unwrap();
        assert_eq!(status.version.as_deref(), Some("1.0"));
        assert_eq!(status.uptime_seconds, Some(42));
    }

    #[test]
    fn collection_info_accepts_record_count_alias_and_nested_stats() {
        let info: CollectionInfo = serde_json::from_value(json!({
            "collection_id": "uuid-1",
            "name": "items",
            "dimension": 384,
            "record_count": 17,
            "engine": "sst",
            "stats": {
                "record_count": 19,
                "storage_size_bytes": 1024
            }
        }))
        .unwrap();

        assert_eq!(info.collection_id.as_deref(), Some("uuid-1"));
        assert_eq!(info.name, "items");
        assert_eq!(info.dimension, 384);
        assert_eq!(info.vector_count, 17);
        assert_eq!(info.engine.as_deref(), Some("sst"));
        assert_eq!(info.stats.as_ref().unwrap().record_count, 19);
        assert_eq!(info.stats.as_ref().unwrap().storage_size_bytes, 1024);
    }

    #[test]
    fn list_response_dtos_deserialize_wrapped_collections_and_graphs() {
        let collections: ListCollectionsResponse = serde_json::from_value(json!({
            "collections": [
                {"name": "items", "dimension": 128, "vector_count": 3}
            ]
        }))
        .unwrap();
        assert_eq!(collections.collections.len(), 1);
        assert_eq!(collections.collections[0].name, "items");

        let graphs: ListGraphsResponse = serde_json::from_value(json!({
            "graphs": [
                {"name": "kg", "node_count": 2, "edge_count": 1}
            ]
        }))
        .unwrap();
        assert_eq!(graphs.graphs.len(), 1);
        assert_eq!(graphs.graphs[0].name, "kg");
        assert_eq!(graphs.graphs[0].node_count, 2);
        assert_eq!(graphs.graphs[0].edge_count, 1);
    }
}
