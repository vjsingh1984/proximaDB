/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Connection Pool Manager for Inter-Node Communication
//!
//! This module provides connection pooling for gRPC channels to cluster nodes.
//! Features include:
//! - Lazy connection establishment
//! - Connection health checking
//! - Health caching with TTL
//! - Thread-safe using DashMap
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                      ConnectionManager                                   │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  ┌──────────────────────────────────────────────────────────────────┐  │
//! │  │                DashMap<NodeEndpoint, ChannelPool>                 │  │
//! │  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
//! │  │  │ node-1 pool  │  │ node-2 pool  │  │ node-3 pool  │  ...       │  │
//! │  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
//! │  └──────────────────────────────────────────────────────────────────┘  │
//! │                                                                         │
//! │  ┌──────────────────────────────────────────────────────────────────┐  │
//! │  │             RwLock<HashMap<NodeEndpoint, CachedHealth>>           │  │
//! │  │  (Health status cache with TTL for avoiding redundant checks)     │  │
//! │  └──────────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tonic::transport::{Channel, Endpoint};

use super::error::{RpcError, RpcErrorKind, RpcResult};
use super::types::{NodeEndpoint, ServingStatus};

// ============================================================================
// CONFIGURATION
// ============================================================================

/// Configuration for connection pool behavior
#[derive(Debug, Clone)]
pub struct ConnectionPoolConfig {
    /// Maximum number of connections per node
    pub max_connections_per_node: usize,

    /// Idle timeout for connections (after which they may be closed)
    pub idle_timeout: Duration,

    /// Connection timeout for establishing new connections
    pub connect_timeout: Duration,

    /// Request timeout for individual RPC calls
    pub request_timeout: Duration,

    /// Health check interval
    pub health_check_interval: Duration,

    /// Health cache TTL (how long to cache health status)
    pub health_cache_ttl: Duration,

    /// Whether to use TLS for connections
    pub use_tls: bool,

    /// TCP keepalive interval
    pub tcp_keepalive: Option<Duration>,

    /// HTTP/2 keep-alive interval
    pub http2_keepalive_interval: Option<Duration>,

    /// HTTP/2 keep-alive timeout
    pub http2_keepalive_timeout: Option<Duration>,
}

impl Default for ConnectionPoolConfig {
    fn default() -> Self {
        Self {
            max_connections_per_node: 10,
            idle_timeout: Duration::from_secs(300), // 5 minutes
            connect_timeout: Duration::from_secs(5), // 5 seconds
            request_timeout: Duration::from_secs(30), // 30 seconds
            health_check_interval: Duration::from_secs(10), // 10 seconds
            health_cache_ttl: Duration::from_secs(5), // 5 seconds
            use_tls: false,
            tcp_keepalive: Some(Duration::from_secs(60)),
            http2_keepalive_interval: Some(Duration::from_secs(30)),
            http2_keepalive_timeout: Some(Duration::from_secs(10)),
        }
    }
}

impl ConnectionPoolConfig {
    /// Create a new configuration with custom settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum connections per node
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections_per_node = max;
        self
    }

    /// Set idle timeout
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Set connection timeout
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set request timeout
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Enable TLS
    pub fn with_tls(mut self, use_tls: bool) -> Self {
        self.use_tls = use_tls;
        self
    }
}

// ============================================================================
// CACHED HEALTH
// ============================================================================

/// Cached health status for a node
#[derive(Debug, Clone)]
pub struct CachedHealth {
    /// The health status
    pub status: ServingStatus,

    /// When this status was last checked
    pub last_checked: Instant,

    /// Number of consecutive failures
    pub consecutive_failures: u32,

    /// Last error message (if any)
    pub last_error: Option<String>,
}

impl CachedHealth {
    /// Create a new healthy cache entry
    pub fn healthy() -> Self {
        Self {
            status: ServingStatus::Serving,
            last_checked: Instant::now(),
            consecutive_failures: 0,
            last_error: None,
        }
    }

    /// Create a new unhealthy cache entry
    pub fn unhealthy(error: impl Into<String>) -> Self {
        Self {
            status: ServingStatus::NotServing,
            last_checked: Instant::now(),
            consecutive_failures: 1,
            last_error: Some(error.into()),
        }
    }

    /// Check if this cache entry has expired
    pub fn is_expired(&self, ttl: Duration) -> bool {
        self.last_checked.elapsed() > ttl
    }

    /// Record a failure
    pub fn record_failure(&mut self, error: impl Into<String>) {
        self.status = ServingStatus::NotServing;
        self.last_checked = Instant::now();
        self.consecutive_failures += 1;
        self.last_error = Some(error.into());
    }

    /// Record a success
    pub fn record_success(&mut self) {
        self.status = ServingStatus::Serving;
        self.last_checked = Instant::now();
        self.consecutive_failures = 0;
        self.last_error = None;
    }
}

// ============================================================================
// CHANNEL POOL
// ============================================================================

/// Pool of gRPC channels to a single node
///
/// Manages multiple connections to the same endpoint for load distribution
/// and fault tolerance.
#[derive(Debug)]
pub struct ChannelPool {
    /// The target endpoint
    endpoint: NodeEndpoint,

    /// Pool of available channels
    channels: Vec<Channel>,

    /// Configuration for this pool
    config: ConnectionPoolConfig,

    /// Round-robin counter for channel selection
    next_channel: AtomicU64,

    /// When this pool was created
    created_at: Instant,

    /// Last time a channel was used
    last_used: AtomicU64,
}

impl ChannelPool {
    /// Create a new channel pool (lazy - no connections established yet)
    pub fn new(endpoint: NodeEndpoint, config: ConnectionPoolConfig) -> Self {
        Self {
            endpoint,
            channels: Vec::new(),
            config,
            next_channel: AtomicU64::new(0),
            created_at: Instant::now(),
            last_used: AtomicU64::new(0),
        }
    }

    /// Get a channel from the pool, establishing a new connection if needed
    pub async fn get_channel(&mut self) -> RpcResult<Channel> {
        // Update last used time
        self.last_used.store(
            Instant::now().duration_since(self.created_at).as_secs(),
            Ordering::Relaxed,
        );

        // If we have no channels or need more, create one
        if self.channels.is_empty() {
            let channel = self.create_channel().await?;
            self.channels.push(channel.clone());
            return Ok(channel);
        }

        // Round-robin selection
        let idx = self.next_channel.fetch_add(1, Ordering::Relaxed) as usize % self.channels.len();
        Ok(self.channels[idx].clone())
    }

    /// Create a new gRPC channel to the endpoint
    async fn create_channel(&self) -> RpcResult<Channel> {
        let scheme = if self.config.use_tls || self.endpoint.tls {
            "https"
        } else {
            "http"
        };

        let uri = format!("{}://{}", scheme, self.endpoint.address);

        let mut endpoint = Endpoint::from_shared(uri).map_err(|e| {
            RpcError::new(
                RpcErrorKind::Connection,
                format!("Invalid endpoint URI: {}", e),
            )
        })?;

        // Apply configuration
        endpoint = endpoint
            .connect_timeout(self.config.connect_timeout)
            .timeout(self.config.request_timeout);

        if let Some(keepalive) = self.config.tcp_keepalive {
            endpoint = endpoint.tcp_keepalive(Some(keepalive));
        }

        if let Some(interval) = self.config.http2_keepalive_interval {
            endpoint = endpoint.http2_keep_alive_interval(interval);
        }

        if let Some(timeout) = self.config.http2_keepalive_timeout {
            endpoint = endpoint.keep_alive_timeout(timeout);
        }

        // Connect (lazy connection)
        let channel = endpoint.connect_lazy();

        Ok(channel)
    }

    /// Get the number of channels in the pool
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Check if the pool has been idle for longer than the timeout
    pub fn is_idle(&self) -> bool {
        let last_used_secs = self.last_used.load(Ordering::Relaxed);
        let now_secs = Instant::now().duration_since(self.created_at).as_secs();
        let idle_secs = now_secs.saturating_sub(last_used_secs);
        Duration::from_secs(idle_secs) > self.config.idle_timeout
    }

    /// Get the endpoint this pool connects to
    pub fn endpoint(&self) -> &NodeEndpoint {
        &self.endpoint
    }
}

// ============================================================================
// CONNECTION MANAGER
// ============================================================================

/// Manages connections to all cluster nodes
///
/// Provides connection pooling, health caching, and lazy connection establishment.
/// Thread-safe using DashMap and RwLock.
///
/// # Example
///
/// ```ignore
/// let config = ConnectionPoolConfig::default();
/// let manager = ConnectionManager::new(config);
///
/// // Get a channel to a node
/// let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
/// let channel = manager.get_channel(&endpoint).await?;
///
/// // Mark node as unhealthy
/// manager.mark_unhealthy(&endpoint, "connection refused").await;
///
/// // Check if node is healthy
/// let healthy = manager.is_healthy(&endpoint).await;
/// ```
pub struct ConnectionManager {
    /// Configuration for connection pools
    config: ConnectionPoolConfig,

    /// Channel pools per node endpoint
    channels: DashMap<String, ChannelPool>,

    /// Health status cache per node
    health_cache: RwLock<HashMap<String, CachedHealth>>,

    /// Metrics: total connections established
    total_connections: AtomicU64,

    /// Metrics: total connection failures
    total_failures: AtomicU64,
}

impl ConnectionManager {
    /// Create a new connection manager
    pub fn new(config: ConnectionPoolConfig) -> Self {
        Self {
            config,
            channels: DashMap::new(),
            health_cache: RwLock::new(HashMap::new()),
            total_connections: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
        }
    }

    /// Get a gRPC channel to the specified endpoint
    ///
    /// This method:
    /// 1. Checks health cache - if unhealthy and not expired, returns error
    /// 2. Gets or creates a channel pool for the endpoint
    /// 3. Returns a channel from the pool
    pub async fn get_channel(&self, endpoint: &NodeEndpoint) -> RpcResult<Channel> {
        let key = endpoint_key(endpoint);

        // Check health cache
        {
            let cache = self.health_cache.read().await;
            if let Some(health) = cache.get(&key)
                && !health.is_expired(self.config.health_cache_ttl)
                    && health.status != ServingStatus::Serving
                {
                    return Err(RpcError::new(
                        RpcErrorKind::Connection,
                        format!(
                            "Node {} is unhealthy: {}",
                            endpoint,
                            health.last_error.as_deref().unwrap_or("unknown")
                        ),
                    ));
                }
        }

        // Get or create channel pool
        let channel = {
            let mut pool = self
                .channels
                .entry(key.clone())
                .or_insert_with(|| ChannelPool::new(endpoint.clone(), self.config.clone()));

            match pool.get_channel().await {
                Ok(ch) => {
                    self.total_connections.fetch_add(1, Ordering::Relaxed);
                    ch
                }
                Err(e) => {
                    self.total_failures.fetch_add(1, Ordering::Relaxed);
                    // Update health cache on failure
                    self.mark_unhealthy(endpoint, e.message()).await;
                    return Err(e);
                }
            }
        };

        Ok(channel)
    }

    /// Mark a node as unhealthy
    pub async fn mark_unhealthy(&self, endpoint: &NodeEndpoint, error: impl Into<String>) {
        let key = endpoint_key(endpoint);
        let error_msg = error.into();

        let mut cache = self.health_cache.write().await;
        cache
            .entry(key)
            .and_modify(|h| h.record_failure(&error_msg))
            .or_insert_with(|| CachedHealth::unhealthy(&error_msg));
    }

    /// Mark a node as healthy
    pub async fn mark_healthy(&self, endpoint: &NodeEndpoint) {
        let key = endpoint_key(endpoint);

        let mut cache = self.health_cache.write().await;
        cache
            .entry(key)
            .and_modify(|h| h.record_success())
            .or_insert_with(CachedHealth::healthy);
    }

    /// Check if a node is considered healthy
    pub async fn is_healthy(&self, endpoint: &NodeEndpoint) -> bool {
        let key = endpoint_key(endpoint);

        let cache = self.health_cache.read().await;
        match cache.get(&key) {
            Some(health) => {
                // If expired, consider healthy (will be re-checked)
                if health.is_expired(self.config.health_cache_ttl) {
                    true
                } else {
                    health.status == ServingStatus::Serving
                }
            }
            // No cached status means we haven't checked yet - consider healthy
            None => true,
        }
    }

    /// Get cached health status for a node
    pub async fn get_health(&self, endpoint: &NodeEndpoint) -> Option<CachedHealth> {
        let key = endpoint_key(endpoint);
        let cache = self.health_cache.read().await;
        cache.get(&key).cloned()
    }

    /// Get all endpoints with active connections
    pub fn active_endpoints(&self) -> Vec<NodeEndpoint> {
        self.channels
            .iter()
            .map(|entry| entry.value().endpoint().clone())
            .collect()
    }

    /// Get all healthy endpoints
    pub async fn healthy_endpoints(&self) -> Vec<NodeEndpoint> {
        let cache = self.health_cache.read().await;
        self.channels
            .iter()
            .filter(|entry| {
                let key = endpoint_key(entry.value().endpoint());
                match cache.get(&key) {
                    Some(health) => {
                        health.is_expired(self.config.health_cache_ttl)
                            || health.status == ServingStatus::Serving
                    }
                    None => true,
                }
            })
            .map(|entry| entry.value().endpoint().clone())
            .collect()
    }

    /// Remove idle connections
    pub fn cleanup_idle(&self) {
        self.channels.retain(|_, pool| !pool.is_idle());
    }

    /// Get connection statistics
    pub fn stats(&self) -> ConnectionStats {
        let active_pools = self.channels.len();
        let total_channels: usize = self
            .channels
            .iter()
            .map(|e| e.value().channel_count())
            .sum();

        ConnectionStats {
            active_pools,
            total_channels,
            total_connections: self.total_connections.load(Ordering::Relaxed),
            total_failures: self.total_failures.load(Ordering::Relaxed),
        }
    }

    /// Close all connections to a specific endpoint
    pub fn close_endpoint(&self, endpoint: &NodeEndpoint) {
        let key = endpoint_key(endpoint);
        self.channels.remove(&key);
    }

    /// Close all connections
    pub fn close_all(&self) {
        self.channels.clear();
    }
}

/// Statistics about the connection manager
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    /// Number of active connection pools
    pub active_pools: usize,

    /// Total number of channels across all pools
    pub total_channels: usize,

    /// Total connections ever established
    pub total_connections: u64,

    /// Total connection failures
    pub total_failures: u64,
}

// ============================================================================
// HELPERS
// ============================================================================

/// Create a unique key for an endpoint
fn endpoint_key(endpoint: &NodeEndpoint) -> String {
    format!("{}:{}", endpoint.node_id, endpoint.address)
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_pool_config_default() {
        let config = ConnectionPoolConfig::default();
        assert_eq!(config.max_connections_per_node, 10);
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert!(!config.use_tls);
    }

    #[test]
    fn test_connection_pool_config_builder() {
        let config = ConnectionPoolConfig::new()
            .with_max_connections(20)
            .with_connect_timeout(Duration::from_secs(10))
            .with_tls(true);

        assert_eq!(config.max_connections_per_node, 20);
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert!(config.use_tls);
    }

    #[test]
    fn test_cached_health() {
        let mut health = CachedHealth::healthy();
        assert_eq!(health.status, ServingStatus::Serving);
        assert_eq!(health.consecutive_failures, 0);

        health.record_failure("connection refused");
        assert_eq!(health.status, ServingStatus::NotServing);
        assert_eq!(health.consecutive_failures, 1);
        assert!(health.last_error.is_some());

        health.record_success();
        assert_eq!(health.status, ServingStatus::Serving);
        assert_eq!(health.consecutive_failures, 0);
        assert!(health.last_error.is_none());
    }

    #[test]
    fn test_cached_health_expiry() {
        let health = CachedHealth::healthy();

        // Should not be expired immediately
        assert!(!health.is_expired(Duration::from_secs(1)));

        // Would be expired after TTL (can't easily test time passage)
    }

    #[test]
    fn test_channel_pool_creation() {
        let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
        let config = ConnectionPoolConfig::default();
        let pool = ChannelPool::new(endpoint.clone(), config);

        assert_eq!(pool.endpoint().node_id, "node-1");
        assert_eq!(pool.channel_count(), 0); // Lazy, no channels yet
    }

    #[tokio::test]
    async fn test_connection_manager_creation() {
        let config = ConnectionPoolConfig::default();
        let manager = ConnectionManager::new(config);

        assert!(manager.active_endpoints().is_empty());
        assert_eq!(manager.stats().active_pools, 0);
    }

    #[tokio::test]
    async fn test_connection_manager_health_tracking() {
        let config = ConnectionPoolConfig::default();
        let manager = ConnectionManager::new(config);

        let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        // Initially healthy (no cached status)
        assert!(manager.is_healthy(&endpoint).await);

        // Mark as unhealthy
        manager
            .mark_unhealthy(&endpoint, "connection refused")
            .await;
        assert!(!manager.is_healthy(&endpoint).await);

        // Mark as healthy again
        manager.mark_healthy(&endpoint).await;
        assert!(manager.is_healthy(&endpoint).await);
    }

    #[tokio::test]
    async fn test_connection_manager_get_health() {
        let config = ConnectionPoolConfig::default();
        let manager = ConnectionManager::new(config);

        let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        // No cached health initially
        assert!(manager.get_health(&endpoint).await.is_none());

        // Mark unhealthy
        manager.mark_unhealthy(&endpoint, "test error").await;

        let health = manager.get_health(&endpoint).await;
        assert!(health.is_some());
        let health = health.unwrap();
        assert_eq!(health.status, ServingStatus::NotServing);
        assert_eq!(health.last_error.as_deref(), Some("test error"));
    }

    #[test]
    fn test_endpoint_key() {
        let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
        let key = endpoint_key(&endpoint);
        assert_eq!(key, "node-1:127.0.0.1:5679");
    }

    #[test]
    fn test_connection_stats() {
        let config = ConnectionPoolConfig::default();
        let manager = ConnectionManager::new(config);

        let stats = manager.stats();
        assert_eq!(stats.active_pools, 0);
        assert_eq!(stats.total_channels, 0);
        assert_eq!(stats.total_connections, 0);
        assert_eq!(stats.total_failures, 0);
    }
}
