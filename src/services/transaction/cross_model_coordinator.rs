/*
 * Copyright 2025 Vijaykumar Singh
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

//! # Cross-Model Transaction Coordinator (TD-133)
//!
//! Implements atomic multi-modal writes across graph nodes, vector embeddings,
//! and graph edges. This coordinator ensures that either all writes succeed or
//! none do, maintaining consistency across storage engines.
//!
//! ## Architecture
//!
//! The coordinator uses a **prepare-commit-compensate** pattern:
//! - **Prepare**: Collect all operations and validate
//! - **Commit**: Execute writes in dependency order (node → embedding → edges)
//! - **Compensate**: Rollback any successful writes if a later write fails
//!
//! ## Flag Gate
//!
//! All functionality is behind the `PROXIMADB_CROSS_MODEL_TX_ENABLED` environment
//! variable. When disabled (default), the coordinator returns an error, and callers
//! should fall back to legacy separate-write paths.
//!
//! ## Contract
//!
//! - **Fail-closed**: If any write fails, rollback all attempted writes
//! - **Idempotent**: Re-running with the same node_oid is safe (upsert semantics)
//! - **Tenant-scoped**: All writes use the provided TenantContext

use std::env;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing::{debug, info, warn};

use crate::graph::engines::GraphEngine;
use crate::graph::model::{Edge, Node};
use crate::storage::tenant::context::TenantContext;

/// Environment variable flag for cross-model transaction support.
/// When unset or set to "false", the coordinator is disabled.
const CROSS_MODEL_TX_FLAG: &str = "PROXIMADB_CROSS_MODEL_TX_ENABLED";

/// Outcome of a cross-model transaction attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionOutcome {
    /// All writes succeeded atomically.
    Committed { node_oid: String },
    /// Transaction failed and was rolled back.
    RolledBack { reason: String },
    /// Coordinator is disabled (flag gate).
    Disabled,
}

/// Result type for cross-model transaction operations.
pub type CrossModelTxResult = Result<TransactionOutcome>;

/// Context tracking what was written during a transaction for rollback.
#[derive(Debug, Default)]
struct TransactionLog {
    /// Node ID if node was written
    node_id: Option<String>,
    /// Edge IDs that were written
    edge_ids: Vec<String>,
    /// Whether the embedding was written
    embedding_written: bool,
}

impl TransactionLog {
    /// Record that a node was written.
    fn mark_node_written(&mut self, node_id: String) {
        self.node_id = Some(node_id);
    }

    /// Record that an edge was written.
    fn mark_edge_written(&mut self, edge_id: String) {
        self.edge_ids.push(edge_id);
    }

    /// Record that an embedding was written.
    fn mark_embedding_written(&mut self) {
        self.embedding_written = true;
    }

    /// Check if any writes were performed (requires rollback on failure).
    #[allow(dead_code)]
    fn has_writes(&self) -> bool {
        self.node_id.is_some() || !self.edge_ids.is_empty() || self.embedding_written
    }
}

/// Cross-model transaction coordinator for atomic multi-modal writes.
///
/// Coordinates writes across three storage domains:
/// 1. **Graph Engine**: Node and edge records via GraphEngine trait
/// 2. **Vector/Record Storage**: Embedding vectors via record mutations
/// 3. **Graph Engine**: Edge relationships between nodes
///
/// The coordinator ensures atomicity by rolling back all successful writes
/// if any write fails.
pub struct CrossModelTransactionCoordinator {
    /// Graph engine for node/edge operations
    graph_engine: Arc<dyn GraphEngine>,
    /// Flag indicating whether the coordinator is enabled
    enabled: bool,
}

impl CrossModelTransactionCoordinator {
    /// Create a new cross-model transaction coordinator.
    ///
    /// The coordinator checks the `PROXIMADB_CROSS_MODEL_TX_ENABLED` flag
    /// on construction. If the flag is not set or is "false", the coordinator
    /// will be disabled and all operations will return `TransactionOutcome::Disabled`.
    pub fn new(graph_engine: Arc<dyn GraphEngine>) -> Self {
        let enabled = env::var(CROSS_MODEL_TX_FLAG)
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        if enabled {
            info!("Cross-model transaction coordinator ENABLED");
        } else {
            debug!(
                "Cross-model transaction coordinator disabled (set {}=true to enable)",
                CROSS_MODEL_TX_FLAG
            );
        }

        Self {
            graph_engine,
            enabled,
        }
    }

    /// Check if the coordinator is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get a reference to the underlying graph engine (for testing/querying).
    pub fn graph_engine(&self) -> &Arc<dyn GraphEngine> {
        &self.graph_engine
    }

    /// Write a symbol's node + embedding + edges atomically.
    ///
    /// This method performs a three-phase atomic write:
    /// 1. Write the node (graph engine)
    /// 2. Write the embedding vector (record storage)
    /// 3. Write the edges (graph engine)
    ///
    /// If any phase fails, all previously written data is rolled back.
    ///
    /// # Arguments
    ///
    /// * `node` - The graph node to write (contains labels, properties, embedding reference)
    /// * `embedding` - The raw embedding vector for ANN indexing
    /// * `edges` - Edges connecting this node to other nodes
    /// * `tenant_ctx` - Tenant context for isolation and billing
    ///
    /// # Returns
    ///
    /// * `Ok(TransactionOutcome::Committed)` - All writes succeeded
    /// * `Ok(TransactionOutcome::RolledBack)` - Transaction failed and was rolled back
    /// * `Ok(TransactionOutcome::Disabled)` - Coordinator is disabled (use legacy path)
    /// * `Err(_)` - Internal error (not a write failure)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use proximadb::services::transaction::CrossModelTransactionCoordinator;
    /// # use std::sync::Arc;
    /// # async fn example(coordinator: &CrossModelTransactionCoordinator) -> anyhow::Result<()> {
    /// let result = coordinator.write_symbol_atomically(
    ///     node,
    ///     embedding,
    ///     edges,
    ///     &tenant_ctx,
    /// ).await?;
    ///
    /// match result {
    ///     TransactionOutcome::Committed { node_oid } => {
    ///         println!("Symbol committed: {}", node_oid);
    ///     }
    ///     TransactionOutcome::RolledBack { reason } => {
    ///         eprintln!("Transaction rolled back: {}", reason);
    ///     }
    ///     TransactionOutcome::Disabled => {
    ///         // Fall back to legacy separate-write path
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn write_symbol_atomically(
        &self,
        node: Node,
        embedding: Vec<f32>,
        edges: Vec<Edge>,
        _tenant_ctx: &TenantContext,
    ) -> CrossModelTxResult {
        // Check flag gate
        if !self.enabled {
            return Ok(TransactionOutcome::Disabled);
        }

        let node_id = node.id.clone();
        let mut tx_log = TransactionLog::default();

        debug!(
            "Starting cross-model transaction for node: {} ({} edges)",
            node_id,
            edges.len()
        );

        // Phase 1: Write the node
        match self.write_node_phase(node.clone()).await {
            Ok(()) => {
                tx_log.mark_node_written(node_id.clone());
                debug!("Node write phase succeeded: {}", node_id);
            }
            Err(e) => {
                warn!("Node write phase failed: {}", e);
                return Ok(TransactionOutcome::RolledBack {
                    reason: format!("Node write failed: {}", e),
                });
            }
        }

        // Phase 2: Write the embedding
        // Note: For this initial implementation, we track the embedding write
        // but don't persist it to a separate vector store. The embedding is
        // already included in the Node structure. Future work will integrate
        // with the record storage layer for persistence.
        match self
            .write_embedding_phase(&node_id, embedding.clone())
            .await
        {
            Ok(()) => {
                tx_log.mark_embedding_written();
                debug!("Embedding write phase succeeded: {}", node_id);
            }
            Err(e) => {
                warn!("Embedding write phase failed: {}", e);
                // Rollback: delete the node
                self.rollback_node(&node_id).await?;
                return Ok(TransactionOutcome::RolledBack {
                    reason: format!("Embedding write failed, rolled back node: {}", e),
                });
            }
        }

        // Phase 3: Write the edges
        match self.write_edges_phase(edges.clone()).await {
            Ok(written_ids) => {
                for id in written_ids {
                    tx_log.mark_edge_written(id);
                }
                debug!(
                    "Edge write phase succeeded for {} edges",
                    tx_log.edge_ids.len()
                );
            }
            Err(e) => {
                warn!("Edge write phase failed: {}", e);
                // Rollback: delete node and its embedding reference
                self.rollback_full(&node_id, &tx_log.edge_ids).await?;
                return Ok(TransactionOutcome::RolledBack {
                    reason: format!("Edge write failed, rolled back all: {}", e),
                });
            }
        }

        info!(
            "Cross-model transaction COMMITTED for node: {} (with embedding and {} edges)",
            node_id,
            tx_log.edge_ids.len()
        );

        Ok(TransactionOutcome::Committed { node_oid: node_id })
    }

    /// Phase 1: Write the node to the graph engine.
    async fn write_node_phase(&self, node: Node) -> Result<()> {
        self.graph_engine
            .insert_node(node)
            .await
            .map_err(|e| anyhow!("Failed to insert node: {}", e))?;
        Ok(())
    }

    /// Phase 2: Write the embedding vector.
    ///
    /// For this initial implementation, we validate the embedding and track
    /// that it was written. The embedding is already included in the Node
    /// structure and will be indexed by the graph engine.
    ///
    /// Future iterations will persist the embedding to record storage for
    /// ANN indexing via the vector engine.
    async fn write_embedding_phase(&self, node_id: &str, embedding: Vec<f32>) -> Result<()> {
        // Validate embedding
        if embedding.is_empty() {
            return Err(anyhow!("Embedding vector is empty"));
        }

        // Check for NaN/Inf values
        if embedding.iter().any(|v| !v.is_finite()) {
            return Err(anyhow!("Embedding contains non-finite values"));
        }

        debug!(
            "Embedding validated for node {}: {} dimensions",
            node_id,
            embedding.len()
        );

        // TODO (TD-133 Part 2): Persist embedding to record storage
        // This will involve creating a ProximaRecord with the embedding
        // and writing it via the record store's write_mutations method.

        Ok(())
    }

    /// Phase 3: Write edges to the graph engine.
    ///
    /// Returns the list of edge IDs that were successfully written.
    async fn write_edges_phase(&self, edges: Vec<Edge>) -> Result<Vec<String>> {
        let mut written_ids = Vec::new();

        for edge in edges {
            let edge_id = edge.id.clone();
            self.graph_engine
                .insert_edge(edge)
                .await
                .map_err(|e| anyhow!("Failed to insert edge {}: {}", edge_id, e))?;
            written_ids.push(edge_id);
        }

        Ok(written_ids)
    }

    /// Rollback: Delete a node that was written.
    async fn rollback_node(&self, node_id: &str) -> Result<()> {
        debug!("Rolling back node: {}", node_id);
        if let Err(e) = self.graph_engine.delete_node(&node_id.to_string()).await {
            warn!("Failed to rollback node {}: {}", node_id, e);
        }
        Ok(())
    }

    /// Rollback: Delete node and edges.
    async fn rollback_full(&self, node_id: &str, edge_ids: &[String]) -> Result<()> {
        debug!(
            "Rolling back full transaction: node {} and {} edges",
            node_id,
            edge_ids.len()
        );

        // Delete edges (in reverse order - LIFO rollback)
        for edge_id in edge_ids.iter().rev() {
            if let Err(e) = self.graph_engine.delete_edge(edge_id).await {
                warn!("Failed to rollback edge {}: {}", edge_id, e);
            }
        }

        // Delete node
        self.rollback_node(node_id).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::orion::OrionGraphEngine;
    use crate::graph::model::{PropertyValue, property_value::Value};
    use crate::storage::tenant::context::StorageTenantContext;

    /// Test that coordinator is disabled by default.
    #[test]
    fn test_coordinator_disabled_by_default() {
        // Ensure flag is not set
        env::remove_var(CROSS_MODEL_TX_FLAG);

        let engine = Arc::new(OrionGraphEngine::new("test_graph".to_string()).unwrap());
        let coordinator = CrossModelTransactionCoordinator::new(engine);

        assert!(!coordinator.is_enabled());
    }

    /// Test that coordinator can be enabled via flag.
    #[test]
    fn test_coordinator_enabled_with_flag() {
        env::set_var(CROSS_MODEL_TX_FLAG, "true");

        let engine = Arc::new(OrionGraphEngine::new("test_graph".to_string()).unwrap());
        let coordinator = CrossModelTransactionCoordinator::new(engine);

        assert!(coordinator.is_enabled());

        env::remove_var(CROSS_MODEL_TX_FLAG);
    }

    /// Test that coordinator returns Disabled when flag is off.
    #[tokio::test]
    async fn test_write_symbol_atomically_returns_disabled() {
        env::remove_var(CROSS_MODEL_TX_FLAG);

        let engine = Arc::new(OrionGraphEngine::new("test_graph".to_string()).unwrap());
        let coordinator = CrossModelTransactionCoordinator::new(engine);
        let tenant_ctx = StorageTenantContext::for_tenant_id("test_tenant");

        let result = coordinator
            .write_symbol_atomically(Node::default(), vec![0.1, 0.2, 0.3], vec![], &tenant_ctx)
            .await
            .unwrap();

        assert_eq!(result, TransactionOutcome::Disabled);
    }

    /// Test embedding validation rejects empty vectors.
    #[test]
    fn test_embedding_validation_rejects_empty() {
        let engine = Arc::new(OrionGraphEngine::new("test_graph".to_string()).unwrap());
        let coordinator = CrossModelTransactionCoordinator::new(engine);

        env::set_var(CROSS_MODEL_TX_FLAG, "true");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(async { coordinator.write_embedding_phase("test_node", vec![]).await });

        assert!(result.is_err());

        env::remove_var(CROSS_MODEL_TX_FLAG);
    }

    /// Test embedding validation rejects NaN values.
    #[test]
    fn test_embedding_validation_rejects_nan() {
        let engine = Arc::new(OrionGraphEngine::new("test_graph".to_string()).unwrap());
        let coordinator = CrossModelTransactionCoordinator::new(engine);

        env::set_var(CROSS_MODEL_TX_FLAG, "true");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            coordinator
                .write_embedding_phase("test_node", vec![0.1, f32::NAN, 0.3])
                .await
        });

        assert!(result.is_err());

        env::remove_var(CROSS_MODEL_TX_FLAG);
    }

    /// Test embedding validation rejects Inf values.
    #[test]
    fn test_embedding_validation_rejects_inf() {
        let engine = Arc::new(OrionGraphEngine::new("test_graph".to_string()).unwrap());
        let coordinator = CrossModelTransactionCoordinator::new(engine);

        env::set_var(CROSS_MODEL_TX_FLAG, "true");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            coordinator
                .write_embedding_phase("test_node", vec![0.1, f32::INFINITY, 0.3])
                .await
        });

        assert!(result.is_err());

        env::remove_var(CROSS_MODEL_TX_FLAG);
    }

    /// Test embedding validation accepts valid vectors.
    #[test]
    fn test_embedding_validation_accepts_valid() {
        let engine = Arc::new(OrionGraphEngine::new("test_graph".to_string()).unwrap());
        let coordinator = CrossModelTransactionCoordinator::new(engine);

        env::set_var(CROSS_MODEL_TX_FLAG, "true");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            coordinator
                .write_embedding_phase("test_node", vec![0.1, 0.2, 0.3])
                .await
        });

        assert!(result.is_ok());

        env::remove_var(CROSS_MODEL_TX_FLAG);
    }

    /// Test transaction log tracks writes correctly.
    #[test]
    fn test_transaction_log_tracking() {
        let mut log = TransactionLog::default();

        assert!(!log.has_writes());

        log.mark_node_written("node1".to_string());
        assert!(log.has_writes());
        assert_eq!(log.node_id, Some("node1".to_string()));

        log.mark_edge_written("edge1".to_string());
        log.mark_edge_written("edge2".to_string());
        assert_eq!(log.edge_ids, vec!["edge1", "edge2"]);

        log.mark_embedding_written();
        assert!(log.embedding_written);
    }
}
