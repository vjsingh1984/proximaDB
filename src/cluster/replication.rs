//! Engine Replication Module
//!
//! Provides WAL-based replication for write operations to shard replicas.
//! Implements async replication with configurable consistency levels and
//! automatic failover handling.
//!
//! ## Replication Flow
//!
//! 1. Write arrives at primary node
//! 2. Write is appended to local WAL
//! 3. WAL entry is replicated to replica nodes
//! 4. Acknowledgments collected based on consistency level
//! 5. Response sent to client
//!
//! ## Consistency Guarantees
//!
//! - **Quorum**: Majority of replicas must acknowledge
//! - **One**: Only primary acknowledgment required
//! - **All**: All replicas must acknowledge

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, warn};

use super::distributed_ops::ConsistencyLevel;
use super::node_registry::NodeRegistry;
use super::rpc::{
    ConnectionManager, NodeEndpoint, ReplicateRequest, ReplicateResponse, ReplicationSink,
};
use super::shard::{Shard, ShardId};

/// Configuration for replication
#[derive(Debug, Clone)]
pub struct ReplicationConfig {
    /// Maximum replication lag allowed in milliseconds
    pub max_lag_ms: u64,
    /// Replication timeout in milliseconds
    pub replication_timeout_ms: u64,
    /// Batch size for replication
    pub batch_size: usize,
    /// Enable async replication (vs sync)
    pub async_replication: bool,
    /// Buffer size for replication queue
    pub queue_buffer_size: usize,
    /// Enable compression for replication
    pub enable_compression: bool,
    /// Retry configuration
    pub retry_config: ReplicationRetryConfig,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            max_lag_ms: 1000,
            replication_timeout_ms: 5000,
            batch_size: 100,
            async_replication: false,
            queue_buffer_size: 10000,
            enable_compression: true,
            retry_config: ReplicationRetryConfig::default(),
        }
    }
}

/// Retry configuration for failed replications
#[derive(Debug, Clone)]
pub struct ReplicationRetryConfig {
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Initial backoff in milliseconds
    pub initial_backoff_ms: u64,
    /// Maximum backoff in milliseconds
    pub max_backoff_ms: u64,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
}

impl Default for ReplicationRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 2000,
            backoff_multiplier: 2.0,
        }
    }
}

/// A replication entry representing a write operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationEntry {
    /// Unique entry ID
    pub entry_id: u64,
    /// Shard this entry belongs to
    pub shard_id: String,
    /// Log sequence number
    pub lsn: u64,
    /// Timestamp of the write
    pub timestamp: i64,
    /// Operation type
    pub operation: ReplicationOperation,
    /// Serialized data
    pub data: Vec<u8>,
    /// Checksum for integrity verification
    pub checksum: u32,
}

/// Types of replication operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplicationOperation {
    /// Insert new records
    Insert { count: usize },
    /// Update existing records
    Update { ids: Vec<String> },
    /// Delete records
    Delete { ids: Vec<String> },
    /// Flush memtable to disk
    Flush,
    /// Compaction operation
    Compact,
    /// Schema change
    SchemaChange,
}

/// Acknowledgment from a replica
#[derive(Debug, Clone)]
pub struct ReplicationAck {
    /// Node that acknowledged
    pub node_id: String,
    /// LSN that was acknowledged
    pub lsn: u64,
    /// Acknowledgment timestamp
    pub timestamp: i64,
    /// Success status
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// State of a replica
#[derive(Debug, Clone)]
pub struct ReplicaState {
    /// Node ID
    pub node_id: String,
    /// Last acknowledged LSN
    pub last_ack_lsn: u64,
    /// Lag in milliseconds
    pub lag_ms: u64,
    /// Whether replica is healthy
    pub healthy: bool,
    /// Last heartbeat timestamp
    pub last_heartbeat: i64,
    /// Pending entries count
    pub pending_entries: usize,
}

/// Engine replication service
pub struct EngineReplication {
    config: ReplicationConfig,
    /// Current LSN counter
    current_lsn: Arc<RwLock<u64>>,
    /// Entry ID counter
    entry_id_counter: Arc<RwLock<u64>>,
    /// Pending replication entries by shard
    pending_entries: Arc<RwLock<HashMap<ShardId, Vec<ReplicationEntry>>>>,
    /// Replica states by shard
    replica_states: Arc<RwLock<HashMap<String, ReplicaState>>>,
    /// Replication semaphore for limiting concurrent replications
    replication_semaphore: Arc<Semaphore>,
    /// Local node ID
    local_node_id: String,
    /// Statistics
    stats: Arc<RwLock<ReplicationStats>>,
    /// RPC sink for replication (None for local-only mode)
    sink: Option<Arc<dyn ReplicationSink>>,
    /// Connection manager for resilient connections
    connection_manager: Option<Arc<ConnectionManager>>,
    /// Node registry for node lookups
    node_registry: Option<Arc<NodeRegistry>>,
}

/// Replication statistics
#[derive(Debug, Default)]
struct ReplicationStats {
    total_entries_replicated: u64,
    total_bytes_replicated: u64,
    successful_replications: u64,
    failed_replications: u64,
    total_latency_ms: u64,
    retry_count: u64,
}

impl EngineReplication {
    /// Create a new engine replication service (local-only mode, no RPC)
    pub fn new(config: ReplicationConfig, local_node_id: String) -> Self {
        let max_concurrent = config.batch_size * 2;

        Self {
            config,
            current_lsn: Arc::new(RwLock::new(0)),
            entry_id_counter: Arc::new(RwLock::new(0)),
            pending_entries: Arc::new(RwLock::new(HashMap::new())),
            replica_states: Arc::new(RwLock::new(HashMap::new())),
            replication_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            local_node_id,
            stats: Arc::new(RwLock::new(ReplicationStats::default())),
            sink: None,
            connection_manager: None,
            node_registry: None,
        }
    }

    /// Create a new engine replication service with RPC support
    ///
    /// This constructor wires the replication to use actual RPC calls
    /// for distributed replication.
    ///
    /// # Arguments
    ///
    /// * `config` - Replication configuration
    /// * `local_node_id` - The local node's identifier
    /// * `sink` - The RPC sink for replication calls
    /// * `connection_manager` - Connection manager for resilient connections
    /// * `node_registry` - Registry for looking up node addresses
    pub fn with_rpc(
        config: ReplicationConfig,
        local_node_id: String,
        sink: Arc<dyn ReplicationSink>,
        connection_manager: Arc<ConnectionManager>,
        node_registry: Arc<NodeRegistry>,
    ) -> Self {
        let max_concurrent = config.batch_size * 2;

        Self {
            config,
            current_lsn: Arc::new(RwLock::new(0)),
            entry_id_counter: Arc::new(RwLock::new(0)),
            pending_entries: Arc::new(RwLock::new(HashMap::new())),
            replica_states: Arc::new(RwLock::new(HashMap::new())),
            replication_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            local_node_id,
            stats: Arc::new(RwLock::new(ReplicationStats::default())),
            sink: Some(sink),
            connection_manager: Some(connection_manager),
            node_registry: Some(node_registry),
        }
    }

    /// Check if RPC is enabled for this replication service
    pub fn has_rpc(&self) -> bool {
        self.sink.is_some() && self.node_registry.is_some()
    }

    /// Get the replication sink (if configured)
    pub fn sink(&self) -> Option<&Arc<dyn ReplicationSink>> {
        self.sink.as_ref()
    }

    /// Get the connection manager (if configured)
    pub fn connection_manager(&self) -> Option<&Arc<ConnectionManager>> {
        self.connection_manager.as_ref()
    }

    /// Get the node registry (if configured)
    pub fn node_registry(&self) -> Option<&Arc<NodeRegistry>> {
        self.node_registry.as_ref()
    }

    /// Convert a ReplicationEntry to a ReplicateRequest for RPC
    fn to_replicate_request(
        &self,
        entry: &ReplicationEntry,
        consistency: ConsistencyLevel,
    ) -> ReplicateRequest {
        use super::rpc::ConsistencyLevel as RpcConsistencyLevel;
        use super::rpc::ReplicationOperation as RpcReplicationOperation;

        // Convert local ConsistencyLevel to RPC ConsistencyLevel
        let rpc_consistency = match consistency {
            ConsistencyLevel::One => RpcConsistencyLevel::One,
            ConsistencyLevel::Quorum => RpcConsistencyLevel::Quorum,
            ConsistencyLevel::All => RpcConsistencyLevel::All,
            ConsistencyLevel::LocalQuorum => RpcConsistencyLevel::LocalQuorum,
        };

        // Convert local ReplicationOperation to RPC ReplicationOperation
        let rpc_operation = match &entry.operation {
            ReplicationOperation::Insert { .. } => RpcReplicationOperation::Insert,
            ReplicationOperation::Update { .. } => RpcReplicationOperation::Update,
            ReplicationOperation::Delete { .. } => RpcReplicationOperation::Delete,
            ReplicationOperation::Flush => RpcReplicationOperation::Flush,
            ReplicationOperation::Compact => RpcReplicationOperation::Compact,
            ReplicationOperation::SchemaChange => RpcReplicationOperation::SchemaChange,
        };

        ReplicateRequest {
            source_node_id: self.local_node_id.clone(),
            shard_id: entry.shard_id.clone(),
            lsn: entry.lsn,
            timestamp: entry.timestamp,
            operation: rpc_operation,
            data: entry.data.clone(),
            checksum: entry.checksum,
            consistency: rpc_consistency,
            timeout: Duration::from_millis(self.config.replication_timeout_ms),
        }
    }

    /// Convert a ReplicateResponse to a ReplicationAck
    fn from_replicate_response(response: ReplicateResponse) -> ReplicationAck {
        ReplicationAck {
            node_id: response.node_id,
            lsn: response.acked_lsn,
            timestamp: chrono::Utc::now().timestamp_millis(),
            success: response.success,
            error: response.error,
        }
    }

    /// Replicate to a single node using RPC
    ///
    /// This method uses the configured ReplicationSink to make actual RPC calls.
    /// Falls back to simulated replication if RPC is not configured.
    async fn replicate_to_node_rpc(
        &self,
        node_id: &str,
        entry: &ReplicationEntry,
        consistency: ConsistencyLevel,
    ) -> Result<ReplicationAck> {
        // If we have RPC configured, use it
        if let (Some(sink), Some(registry)) = (&self.sink, &self.node_registry) {
            // Look up the node in the registry
            let node_info = registry
                .get_node(node_id)
                .await
                .ok_or_else(|| anyhow!("Node '{}' not found in registry", node_id))?;

            // Create endpoint from node info
            let endpoint = NodeEndpoint::new(node_id, &node_info.address);

            // Convert entry to RPC request
            let request = self.to_replicate_request(entry, consistency);

            debug!(
                "Replicating entry {} to node {} via RPC",
                entry.entry_id, node_id
            );

            // Make the RPC call
            match sink.replicate(&endpoint, request).await {
                Ok(response) => {
                    // Update connection manager health on success
                    if let Some(conn_mgr) = &self.connection_manager {
                        conn_mgr.mark_healthy(&endpoint).await;
                    }
                    Ok(Self::from_replicate_response(response))
                }
                Err(e) => {
                    // Update connection manager health on failure
                    if let Some(conn_mgr) = &self.connection_manager {
                        conn_mgr.mark_unhealthy(&endpoint, e.message()).await;
                    }
                    Err(anyhow!("RPC replication failed: {}", e))
                }
            }
        } else {
            // Fallback to simulated replication (local-only mode)
            Self::replicate_to_node_local(node_id, entry).await
        }
    }

    /// Simulated local replication (for local-only mode or testing)
    async fn replicate_to_node_local(
        node_id: &str,
        entry: &ReplicationEntry,
    ) -> Result<ReplicationAck> {
        debug!(
            "Simulating replication of entry {} to node {}",
            entry.entry_id, node_id
        );

        // Simulate network latency
        tokio::time::sleep(Duration::from_micros(100)).await;

        Ok(ReplicationAck {
            node_id: node_id.to_string(),
            lsn: entry.lsn,
            timestamp: chrono::Utc::now().timestamp_millis(),
            success: true,
            error: None,
        })
    }

    /// Replicate to a single node with retries (instance method)
    async fn replicate_to_node_with_retry_rpc(
        &self,
        node_id: &str,
        entry: &ReplicationEntry,
        consistency: ConsistencyLevel,
    ) -> Result<ReplicationAck> {
        let config = &self.config.retry_config;
        let mut attempt = 0;
        let mut backoff = config.initial_backoff_ms;

        loop {
            match self
                .replicate_to_node_rpc(node_id, entry, consistency)
                .await
            {
                Ok(ack) => return Ok(ack),
                Err(e) => {
                    attempt += 1;
                    if attempt >= config.max_retries {
                        return Err(anyhow!(
                            "Replication failed after {} attempts: {}",
                            attempt,
                            e
                        ));
                    }

                    warn!(
                        "Replication to {} failed (attempt {}), retrying in {}ms: {}",
                        node_id, attempt, backoff, e
                    );

                    // Update retry stats
                    {
                        let mut stats = self.stats.write().await;
                        stats.retry_count += 1;
                    }

                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                    backoff = (backoff as f64 * config.backoff_multiplier) as u64;
                    backoff = backoff.min(config.max_backoff_ms);
                }
            }
        }
    }

    /// Create a replication entry for a write operation
    pub async fn create_entry(
        &self,
        shard_id: &ShardId,
        operation: ReplicationOperation,
        data: Vec<u8>,
    ) -> Result<ReplicationEntry> {
        let entry_id = {
            let mut counter = self.entry_id_counter.write().await;
            *counter += 1;
            *counter
        };

        let lsn = {
            let mut lsn = self.current_lsn.write().await;
            *lsn += 1;
            *lsn
        };

        let checksum = crc32fast::hash(&data);

        Ok(ReplicationEntry {
            entry_id,
            shard_id: shard_id.id().to_string(),
            lsn,
            timestamp: chrono::Utc::now().timestamp_millis(),
            operation,
            data,
            checksum,
        })
    }

    /// Replicate an entry to shard replicas
    pub async fn replicate(
        &self,
        entry: ReplicationEntry,
        shard: &Shard,
        consistency: ConsistencyLevel,
    ) -> Result<Vec<ReplicationAck>> {
        let start = Instant::now();
        let replicas = shard.replica_nodes();

        if replicas.is_empty()
            && matches!(
                consistency,
                ConsistencyLevel::Quorum | ConsistencyLevel::All
            )
        {
            // No replicas, but consistency requires them
            warn!("No replicas for shard {}, degraded consistency", shard.id);
        }

        let required_acks = self.calculate_required_acks(
            replicas.len() + 1, // +1 for primary
            consistency,
        );

        // Add to pending entries
        {
            let mut pending = self.pending_entries.write().await;
            pending
                .entry(shard.id.clone())
                .or_default()
                .push(entry.clone());
        }

        // Replicate to replicas
        let acks = if self.config.async_replication {
            self.replicate_async(&entry, &replicas, consistency).await?
        } else {
            self.replicate_sync(&entry, &replicas, required_acks, consistency)
                .await?
        };

        // Check if we have enough acks
        let successful_acks = acks.iter().filter(|a| a.success).count() + 1; // +1 for primary

        if successful_acks < required_acks {
            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.failed_replications += 1;
            }

            return Err(anyhow!(
                "Insufficient replicas acknowledged: {} of {} required",
                successful_acks,
                required_acks
            ));
        }

        // Remove from pending after successful replication
        {
            let mut pending = self.pending_entries.write().await;
            if let Some(entries) = pending.get_mut(&shard.id) {
                entries.retain(|e| e.entry_id != entry.entry_id);
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_entries_replicated += 1;
            stats.total_bytes_replicated += entry.data.len() as u64;
            stats.successful_replications += 1;
            stats.total_latency_ms += start.elapsed().as_millis() as u64;
        }

        Ok(acks)
    }

    /// Synchronous replication to replicas
    ///
    /// When RPC is configured, uses actual network calls. Otherwise, falls back
    /// to simulated replication for testing/local mode.
    async fn replicate_sync(
        &self,
        entry: &ReplicationEntry,
        replicas: &[&str],
        required_acks: usize,
        consistency: ConsistencyLevel,
    ) -> Result<Vec<ReplicationAck>> {
        use futures::future::join_all;
        use std::future::Future;
        use std::pin::Pin;

        type ReplicationFuture = Pin<Box<dyn Future<Output = Result<ReplicationAck>> + Send>>;

        let timeout = Duration::from_millis(self.config.replication_timeout_ms);
        let semaphore = self.replication_semaphore.clone();

        // Build futures based on whether RPC is configured
        let futures: Vec<ReplicationFuture> = if self.has_rpc() {
            // RPC-enabled path: collect info needed for async tasks
            let sink = self.sink.clone();
            let registry = self.node_registry.clone();
            let conn_mgr = self.connection_manager.clone();
            let retry_config = self.config.retry_config.clone();
            let local_node_id = self.local_node_id.clone();
            let replication_timeout = self.config.replication_timeout_ms;

            replicas
                .iter()
                .map(|replica| {
                    let entry = entry.clone();
                    let replica = replica.to_string();
                    let semaphore = semaphore.clone();
                    let sink = sink.clone();
                    let registry = registry.clone();
                    let conn_mgr = conn_mgr.clone();
                    let retry_config = retry_config.clone();
                    let local_node_id = local_node_id.clone();

                    Box::pin(async move {
                        let _permit = semaphore.acquire().await.unwrap();
                        replicate_to_node_with_rpc(
                            &replica,
                            &entry,
                            consistency,
                            &retry_config,
                            sink.as_ref(),
                            registry.as_ref(),
                            conn_mgr.as_ref(),
                            &local_node_id,
                            replication_timeout,
                        )
                        .await
                    }) as ReplicationFuture
                })
                .collect()
        } else {
            // Fallback path: simulated replication
            replicas
                .iter()
                .map(|replica| {
                    let entry = entry.clone();
                    let replica = replica.to_string();
                    let semaphore = semaphore.clone();
                    let config = self.config.retry_config.clone();

                    Box::pin(async move {
                        let _permit = semaphore.acquire().await.unwrap();
                        Self::replicate_to_node_with_retry(&replica, &entry, &config).await
                    }) as ReplicationFuture
                })
                .collect()
        };

        // Wait for results with timeout
        let results = tokio::time::timeout(timeout, join_all(futures))
            .await
            .map_err(|_| anyhow!("Replication timeout"))?;

        let acks: Vec<ReplicationAck> = results.into_iter().filter_map(|r| r.ok()).collect();

        // Check if we got enough acks early (optimization)
        let successful = acks.iter().filter(|a| a.success).count() + 1;
        if successful >= required_acks {
            debug!(
                "Got {} acks, required {}, proceeding",
                successful, required_acks
            );
        }

        Ok(acks)
    }

    /// Asynchronous replication to replicas (fire and forget with eventual consistency)
    ///
    /// When RPC is configured, uses actual network calls. Otherwise, falls back
    /// to simulated replication for testing/local mode.
    async fn replicate_async(
        &self,
        entry: &ReplicationEntry,
        replicas: &[&str],
        consistency: ConsistencyLevel,
    ) -> Result<Vec<ReplicationAck>> {
        // For async replication, we don't wait for acks
        // Instead, spawn background tasks and return immediately

        if self.has_rpc() {
            // RPC-enabled path
            let sink = self.sink.clone();
            let registry = self.node_registry.clone();
            let conn_mgr = self.connection_manager.clone();
            let retry_config = self.config.retry_config.clone();
            let local_node_id = self.local_node_id.clone();
            let replication_timeout = self.config.replication_timeout_ms;

            for replica in replicas {
                let entry = entry.clone();
                let replica = replica.to_string();
                let sink = sink.clone();
                let registry = registry.clone();
                let conn_mgr = conn_mgr.clone();
                let retry_config = retry_config.clone();
                let local_node_id = local_node_id.clone();

                tokio::spawn(async move {
                    if let Err(e) = replicate_to_node_with_rpc(
                        &replica,
                        &entry,
                        consistency,
                        &retry_config,
                        sink.as_ref(),
                        registry.as_ref(),
                        conn_mgr.as_ref(),
                        &local_node_id,
                        replication_timeout,
                    )
                    .await
                    {
                        warn!("Async RPC replication to {} failed: {}", replica, e);
                    }
                });
            }
        } else {
            // Fallback path: simulated replication
            for replica in replicas {
                let entry = entry.clone();
                let replica = replica.to_string();
                let config = self.config.retry_config.clone();

                tokio::spawn(async move {
                    if let Err(e) =
                        Self::replicate_to_node_with_retry(&replica, &entry, &config).await
                    {
                        warn!("Async replication to {} failed: {}", replica, e);
                    }
                });
            }
        }

        // Return empty acks for async mode
        Ok(Vec::new())
    }

    /// Replicate to a single node with retries
    async fn replicate_to_node_with_retry(
        node_id: &str,
        entry: &ReplicationEntry,
        config: &ReplicationRetryConfig,
    ) -> Result<ReplicationAck> {
        let mut attempt = 0;
        let mut backoff = config.initial_backoff_ms;

        loop {
            match Self::replicate_to_node(node_id, entry).await {
                Ok(ack) => return Ok(ack),
                Err(e) => {
                    attempt += 1;
                    if attempt >= config.max_retries {
                        return Err(anyhow!(
                            "Replication failed after {} attempts: {}",
                            attempt,
                            e
                        ));
                    }

                    warn!(
                        "Replication to {} failed (attempt {}), retrying in {}ms: {}",
                        node_id, attempt, backoff, e
                    );

                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                    backoff = (backoff as f64 * config.backoff_multiplier) as u64;
                    backoff = backoff.min(config.max_backoff_ms);
                }
            }
        }
    }

    /// Replicate to a single node
    async fn replicate_to_node(node_id: &str, entry: &ReplicationEntry) -> Result<ReplicationAck> {
        // In a real implementation, this would make an RPC call
        // For now, simulate replication
        debug!("Replicating entry {} to node {}", entry.entry_id, node_id);

        // Simulate network latency
        tokio::time::sleep(Duration::from_micros(100)).await;

        Ok(ReplicationAck {
            node_id: node_id.to_string(),
            lsn: entry.lsn,
            timestamp: chrono::Utc::now().timestamp_millis(),
            success: true,
            error: None,
        })
    }

    /// Calculate required acknowledgments for consistency level
    fn calculate_required_acks(
        &self,
        total_replicas: usize,
        consistency: ConsistencyLevel,
    ) -> usize {
        match consistency {
            ConsistencyLevel::One => 1,
            ConsistencyLevel::Quorum => (total_replicas / 2) + 1,
            ConsistencyLevel::All => total_replicas,
            ConsistencyLevel::LocalQuorum => (total_replicas / 2) + 1,
        }
    }

    /// Update replica state after receiving acknowledgment
    pub async fn update_replica_state(&self, ack: &ReplicationAck) {
        let mut states = self.replica_states.write().await;

        let state = states
            .entry(ack.node_id.clone())
            .or_insert_with(|| ReplicaState {
                node_id: ack.node_id.clone(),
                last_ack_lsn: 0,
                lag_ms: 0,
                healthy: true,
                last_heartbeat: 0,
                pending_entries: 0,
            });

        if ack.success {
            state.last_ack_lsn = ack.lsn;
            state.last_heartbeat = ack.timestamp;
            state.healthy = true;
        } else {
            state.healthy = false;
        }
    }

    /// Get pending entries for a shard
    pub async fn get_pending_entries(&self, shard_id: &ShardId) -> Vec<ReplicationEntry> {
        let pending = self.pending_entries.read().await;
        pending.get(shard_id).cloned().unwrap_or_default()
    }

    /// Get replica states
    pub async fn get_replica_states(&self) -> HashMap<String, ReplicaState> {
        self.replica_states.read().await.clone()
    }

    /// Get current LSN
    pub async fn current_lsn(&self) -> u64 {
        *self.current_lsn.read().await
    }

    /// Check replication health
    pub async fn check_health(&self) -> ReplicationHealth {
        let states = self.replica_states.read().await;
        let current_lsn = *self.current_lsn.read().await;

        let healthy_replicas = states.values().filter(|s| s.healthy).count();
        let total_replicas = states.len();

        let max_lag = states
            .values()
            .map(|s| current_lsn.saturating_sub(s.last_ack_lsn))
            .max()
            .unwrap_or(0);

        let pending = self.pending_entries.read().await;
        let pending_entries: usize = pending.values().map(|v| v.len()).sum();

        ReplicationHealth {
            healthy: healthy_replicas == total_replicas && max_lag < 100,
            healthy_replicas,
            total_replicas,
            current_lsn,
            max_lag,
            pending_entries,
        }
    }

    /// Get replication statistics
    pub async fn get_stats(&self) -> ReplicationStatsSummary {
        let stats = self.stats.read().await;
        ReplicationStatsSummary {
            total_entries_replicated: stats.total_entries_replicated,
            total_bytes_replicated: stats.total_bytes_replicated,
            successful_replications: stats.successful_replications,
            failed_replications: stats.failed_replications,
            avg_latency_ms: if stats.successful_replications > 0 {
                stats.total_latency_ms / stats.successful_replications
            } else {
                0
            },
            retry_count: stats.retry_count,
        }
    }
}

// ============================================================================
// STANDALONE RPC REPLICATION FUNCTIONS
// ============================================================================

/// Replicate to a single node with RPC and retries
///
/// This is a standalone function that can be called from async tasks without
/// needing a reference to `self`. It handles the full RPC replication flow
/// including retries and connection health tracking.
async fn replicate_to_node_with_rpc(
    node_id: &str,
    entry: &ReplicationEntry,
    consistency: ConsistencyLevel,
    retry_config: &ReplicationRetryConfig,
    sink: Option<&Arc<dyn ReplicationSink>>,
    registry: Option<&Arc<NodeRegistry>>,
    conn_mgr: Option<&Arc<ConnectionManager>>,
    local_node_id: &str,
    replication_timeout_ms: u64,
) -> Result<ReplicationAck> {
    let mut attempt = 0;
    let mut backoff = retry_config.initial_backoff_ms;

    loop {
        match replicate_to_node_once(
            node_id,
            entry,
            consistency,
            sink,
            registry,
            conn_mgr,
            local_node_id,
            replication_timeout_ms,
        )
        .await
        {
            Ok(ack) => return Ok(ack),
            Err(e) => {
                attempt += 1;
                if attempt >= retry_config.max_retries {
                    return Err(anyhow!(
                        "RPC replication failed after {} attempts: {}",
                        attempt,
                        e
                    ));
                }

                warn!(
                    "RPC replication to {} failed (attempt {}), retrying in {}ms: {}",
                    node_id, attempt, backoff, e
                );

                tokio::time::sleep(Duration::from_millis(backoff)).await;
                backoff = (backoff as f64 * retry_config.backoff_multiplier) as u64;
                backoff = backoff.min(retry_config.max_backoff_ms);
            }
        }
    }
}

/// Perform a single RPC replication attempt
async fn replicate_to_node_once(
    node_id: &str,
    entry: &ReplicationEntry,
    consistency: ConsistencyLevel,
    sink: Option<&Arc<dyn ReplicationSink>>,
    registry: Option<&Arc<NodeRegistry>>,
    conn_mgr: Option<&Arc<ConnectionManager>>,
    local_node_id: &str,
    replication_timeout_ms: u64,
) -> Result<ReplicationAck> {
    // Ensure we have RPC components
    let sink = sink.ok_or_else(|| anyhow!("ReplicationSink not configured"))?;
    let registry = registry.ok_or_else(|| anyhow!("NodeRegistry not configured"))?;

    // Look up the node in the registry
    let node_info = registry
        .get_node(node_id)
        .await
        .ok_or_else(|| anyhow!("Node '{}' not found in registry", node_id))?;

    // Create endpoint from node info
    let endpoint = NodeEndpoint::new(node_id, &node_info.address);

    // Convert entry to RPC request
    let request =
        create_replicate_request(entry, consistency, local_node_id, replication_timeout_ms);

    debug!(
        "Replicating entry {} to node {} via RPC",
        entry.entry_id, node_id
    );

    // Make the RPC call
    match sink.replicate(&endpoint, request).await {
        Ok(response) => {
            // Update connection manager health on success
            if let Some(conn_mgr) = conn_mgr {
                conn_mgr.mark_healthy(&endpoint).await;
            }
            Ok(ReplicationAck {
                node_id: response.node_id,
                lsn: response.acked_lsn,
                timestamp: chrono::Utc::now().timestamp_millis(),
                success: response.success,
                error: response.error,
            })
        }
        Err(e) => {
            // Update connection manager health on failure
            if let Some(conn_mgr) = conn_mgr {
                conn_mgr.mark_unhealthy(&endpoint, e.message()).await;
            }
            Err(anyhow!("RPC replication failed: {}", e))
        }
    }
}

/// Create a ReplicateRequest from a ReplicationEntry
fn create_replicate_request(
    entry: &ReplicationEntry,
    consistency: ConsistencyLevel,
    local_node_id: &str,
    replication_timeout_ms: u64,
) -> ReplicateRequest {
    use super::rpc::ConsistencyLevel as RpcConsistencyLevel;
    use super::rpc::ReplicationOperation as RpcReplicationOperation;

    // Convert local ConsistencyLevel to RPC ConsistencyLevel
    let rpc_consistency = match consistency {
        ConsistencyLevel::One => RpcConsistencyLevel::One,
        ConsistencyLevel::Quorum => RpcConsistencyLevel::Quorum,
        ConsistencyLevel::All => RpcConsistencyLevel::All,
        ConsistencyLevel::LocalQuorum => RpcConsistencyLevel::LocalQuorum,
    };

    // Convert local ReplicationOperation to RPC ReplicationOperation
    let rpc_operation = match &entry.operation {
        ReplicationOperation::Insert { .. } => RpcReplicationOperation::Insert,
        ReplicationOperation::Update { .. } => RpcReplicationOperation::Update,
        ReplicationOperation::Delete { .. } => RpcReplicationOperation::Delete,
        ReplicationOperation::Flush => RpcReplicationOperation::Flush,
        ReplicationOperation::Compact => RpcReplicationOperation::Compact,
        ReplicationOperation::SchemaChange => RpcReplicationOperation::SchemaChange,
    };

    ReplicateRequest {
        source_node_id: local_node_id.to_string(),
        shard_id: entry.shard_id.clone(),
        lsn: entry.lsn,
        timestamp: entry.timestamp,
        operation: rpc_operation,
        data: entry.data.clone(),
        checksum: entry.checksum,
        consistency: rpc_consistency,
        timeout: Duration::from_millis(replication_timeout_ms),
    }
}

/// Replication health status
#[derive(Debug, Clone)]
pub struct ReplicationHealth {
    pub healthy: bool,
    pub healthy_replicas: usize,
    pub total_replicas: usize,
    pub current_lsn: u64,
    pub max_lag: u64,
    pub pending_entries: usize,
}

/// Summary of replication statistics
#[derive(Debug, Clone)]
pub struct ReplicationStatsSummary {
    pub total_entries_replicated: u64,
    pub total_bytes_replicated: u64,
    pub successful_replications: u64,
    pub failed_replications: u64,
    pub avg_latency_ms: u64,
    pub retry_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::shard::ShardPlacement;

    fn create_test_shard() -> Shard {
        let mut shard = Shard::new("test-collection", 0);
        shard.add_placement(ShardPlacement {
            node_id: "primary-node".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });
        shard.add_placement(ShardPlacement {
            node_id: "replica-1".to_string(),
            is_primary: false,
            priority: 1,
            lag_ms: None,
        });
        shard.add_placement(ShardPlacement {
            node_id: "replica-2".to_string(),
            is_primary: false,
            priority: 2,
            lag_ms: None,
        });
        shard
    }

    #[tokio::test]
    async fn test_replication_creation() {
        let config = ReplicationConfig::default();
        let replication = EngineReplication::new(config, "local-node".to_string());

        let lsn = replication.current_lsn().await;
        assert_eq!(lsn, 0);
    }

    #[tokio::test]
    async fn test_create_entry() {
        let replication =
            EngineReplication::new(ReplicationConfig::default(), "local-node".to_string());

        let shard_id = ShardId::generate("test", 0);
        let entry = replication
            .create_entry(
                &shard_id,
                ReplicationOperation::Insert { count: 10 },
                vec![1, 2, 3, 4, 5],
            )
            .await
            .unwrap();

        assert_eq!(entry.entry_id, 1);
        assert_eq!(entry.lsn, 1);
        assert_eq!(entry.data, vec![1, 2, 3, 4, 5]);
        assert!(entry.checksum > 0);
    }

    #[tokio::test]
    async fn test_replicate_to_shard() {
        let replication =
            EngineReplication::new(ReplicationConfig::default(), "primary-node".to_string());

        let shard = create_test_shard();
        let shard_id = shard.id.clone();

        let entry = replication
            .create_entry(
                &shard_id,
                ReplicationOperation::Insert { count: 5 },
                vec![1, 2, 3],
            )
            .await
            .unwrap();

        let acks = replication
            .replicate(entry, &shard, ConsistencyLevel::Quorum)
            .await
            .unwrap();

        // Should have acks from replicas
        assert!(!acks.is_empty());
        for ack in &acks {
            assert!(ack.success);
        }
    }

    #[tokio::test]
    async fn test_calculate_required_acks() {
        let replication =
            EngineReplication::new(ReplicationConfig::default(), "local-node".to_string());

        // 3 total replicas (including primary)
        assert_eq!(
            replication.calculate_required_acks(3, ConsistencyLevel::One),
            1
        );
        assert_eq!(
            replication.calculate_required_acks(3, ConsistencyLevel::Quorum),
            2
        );
        assert_eq!(
            replication.calculate_required_acks(3, ConsistencyLevel::All),
            3
        );
    }

    #[tokio::test]
    async fn test_health_check() {
        let replication =
            EngineReplication::new(ReplicationConfig::default(), "local-node".to_string());

        let health = replication.check_health().await;

        assert!(health.healthy);
        assert_eq!(health.current_lsn, 0);
        assert_eq!(health.pending_entries, 0);
    }

    #[tokio::test]
    async fn test_replica_state_tracking() {
        let replication =
            EngineReplication::new(ReplicationConfig::default(), "local-node".to_string());

        let ack = ReplicationAck {
            node_id: "replica-1".to_string(),
            lsn: 100,
            timestamp: chrono::Utc::now().timestamp_millis(),
            success: true,
            error: None,
        };

        replication.update_replica_state(&ack).await;

        let states = replication.get_replica_states().await;
        assert!(states.contains_key("replica-1"));

        let state = states.get("replica-1").unwrap();
        assert_eq!(state.last_ack_lsn, 100);
        assert!(state.healthy);
    }

    #[tokio::test]
    async fn test_replication_without_rpc() {
        // Test that replication works in local-only mode (no RPC configured)
        let replication =
            EngineReplication::new(ReplicationConfig::default(), "local-node".to_string());

        assert!(!replication.has_rpc());
        assert!(replication.sink().is_none());
        assert!(replication.connection_manager().is_none());
        assert!(replication.node_registry().is_none());
    }

    #[tokio::test]
    async fn test_create_replicate_request() {
        // Test the conversion from ReplicationEntry to ReplicateRequest
        let entry = ReplicationEntry {
            entry_id: 42,
            shard_id: "test-shard".to_string(),
            lsn: 100,
            timestamp: 1234567890,
            operation: ReplicationOperation::Insert { count: 10 },
            data: vec![1, 2, 3, 4, 5],
            checksum: 12345,
        };

        let request = create_replicate_request(&entry, ConsistencyLevel::Quorum, "node-1", 5000);

        assert_eq!(request.source_node_id, "node-1");
        assert_eq!(request.shard_id, "test-shard");
        assert_eq!(request.lsn, 100);
        assert_eq!(request.timestamp, 1234567890);
        assert_eq!(request.data, vec![1, 2, 3, 4, 5]);
        assert_eq!(request.checksum, 12345);
        assert_eq!(request.timeout, Duration::from_millis(5000));
    }

    #[tokio::test]
    async fn test_create_replicate_request_all_operations() {
        // Test all operation types are correctly converted
        let operations = vec![
            (
                ReplicationOperation::Insert { count: 1 },
                super::super::rpc::ReplicationOperation::Insert,
            ),
            (
                ReplicationOperation::Update {
                    ids: vec!["id1".to_string()],
                },
                super::super::rpc::ReplicationOperation::Update,
            ),
            (
                ReplicationOperation::Delete {
                    ids: vec!["id1".to_string()],
                },
                super::super::rpc::ReplicationOperation::Delete,
            ),
            (
                ReplicationOperation::Flush,
                super::super::rpc::ReplicationOperation::Flush,
            ),
            (
                ReplicationOperation::Compact,
                super::super::rpc::ReplicationOperation::Compact,
            ),
            (
                ReplicationOperation::SchemaChange,
                super::super::rpc::ReplicationOperation::SchemaChange,
            ),
        ];

        for (local_op, expected_rpc_op) in operations {
            let entry = ReplicationEntry {
                entry_id: 1,
                shard_id: "shard".to_string(),
                lsn: 1,
                timestamp: 0,
                operation: local_op,
                data: vec![],
                checksum: 0,
            };

            let request = create_replicate_request(&entry, ConsistencyLevel::One, "node", 1000);
            assert_eq!(request.operation, expected_rpc_op);
        }
    }

    #[tokio::test]
    async fn test_consistency_level_conversion() {
        use super::super::rpc::ConsistencyLevel as RpcConsistencyLevel;

        let entry = ReplicationEntry {
            entry_id: 1,
            shard_id: "shard".to_string(),
            lsn: 1,
            timestamp: 0,
            operation: ReplicationOperation::Insert { count: 1 },
            data: vec![],
            checksum: 0,
        };

        // Test all consistency level conversions
        let request = create_replicate_request(&entry, ConsistencyLevel::One, "node", 1000);
        assert_eq!(request.consistency, RpcConsistencyLevel::One);

        let request = create_replicate_request(&entry, ConsistencyLevel::Quorum, "node", 1000);
        assert_eq!(request.consistency, RpcConsistencyLevel::Quorum);

        let request = create_replicate_request(&entry, ConsistencyLevel::All, "node", 1000);
        assert_eq!(request.consistency, RpcConsistencyLevel::All);

        let request = create_replicate_request(&entry, ConsistencyLevel::LocalQuorum, "node", 1000);
        assert_eq!(request.consistency, RpcConsistencyLevel::LocalQuorum);
    }

    #[tokio::test]
    async fn test_replicate_to_node_once_without_sink() {
        // Test that replicate_to_node_once fails gracefully without RPC components
        let entry = ReplicationEntry {
            entry_id: 1,
            shard_id: "shard".to_string(),
            lsn: 1,
            timestamp: 0,
            operation: ReplicationOperation::Insert { count: 1 },
            data: vec![],
            checksum: 0,
        };

        let result = replicate_to_node_once(
            "test-node",
            &entry,
            ConsistencyLevel::Quorum,
            None, // No sink
            None, // No registry
            None, // No connection manager
            "local-node",
            5000,
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ReplicationSink not configured")
        );
    }
}
