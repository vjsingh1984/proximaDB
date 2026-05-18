//! Engine-backed implementations for Modular Graph RAG (TD-045).
//!
//! These types adapt existing ProximaDB services into the modular
//! Graph RAG traits instead of introducing duplicate retrieval or
//! traversal stacks.

use super::{NodeRetriever, RagQuery, Result, Subgraph, SubgraphBuilder, SubgraphEdge};
use crate::graph::NodeId;
use crate::graph::engines::GraphEngine;
use crate::services::VectorOperationsService;
use async_trait::async_trait;
use proximadb_kernel::error::{ProximaDBError, StorageError};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

/// A retriever that uses vector similarity to find seed nodes.
///
/// Wraps [`VectorOperationsService`] to perform a standard vector search
/// and returns the resulting document IDs as graph node IDs.
pub struct VectorNodeRetriever {
    vector_ops: Arc<VectorOperationsService>,
    collection: String,
    limit: usize,
}

impl VectorNodeRetriever {
    /// Create a new vector-based retriever for a specific collection.
    pub fn new(vector_ops: Arc<VectorOperationsService>, collection: String, limit: usize) -> Self {
        Self {
            vector_ops,
            collection,
            limit,
        }
    }
}

#[async_trait]
impl NodeRetriever for VectorNodeRetriever {
    async fn retrieve(&self, query: &RagQuery) -> Result<Vec<NodeId>> {
        let vector = match &query.query_vector {
            Some(v) => v.clone(),
            None => {
                // In a real system, we'd call an embedding service here.
                // For now, if no vector is provided, we can't do vector retrieval.
                return Err(ProximaDBError::InvalidInput(
                    "VectorNodeRetriever requires a query_vector".to_string(),
                ));
            }
        };

        let search_results = self
            .vector_ops
            .unified_search(
                &self.collection,
                vector,
                self.limit,
                None, // No filter
                None, // No specific search config
            )
            .await
            .map_err(|e| ProximaDBError::Storage(StorageError::SstEngine(e.to_string())))?;

        Ok(search_results.into_iter().map(|r| r.oid).collect())
    }
}

/// A builder that expands seeds using k-hop BFS traversal.
///
/// Wraps a [`GraphEngine`] to perform breadth-first expansion.
pub struct KHopSubgraphBuilder {
    engine: Arc<dyn GraphEngine>,
    k: u32,
    edge_type: Option<String>,
}

impl KHopSubgraphBuilder {
    /// Create a new k-hop builder.
    pub fn new(engine: Arc<dyn GraphEngine>, k: u32, edge_type: Option<String>) -> Self {
        Self {
            engine,
            k,
            edge_type,
        }
    }

    /// Access the underlying graph engine.
    pub fn engine(&self) -> &Arc<dyn GraphEngine> {
        &self.engine
    }
}

#[async_trait]
impl SubgraphBuilder for KHopSubgraphBuilder {
    async fn build(&self, seeds: &[NodeId]) -> Result<Subgraph> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        // Initialize with seeds
        for seed in seeds {
            if visited.insert(seed.clone()) {
                nodes.push(seed.clone());
                queue.push_back((seed.clone(), 0));
            }
        }

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= self.k {
                continue;
            }

            let neighbors = self
                .engine
                .get_outgoing_edges(&current_id, self.edge_type.as_deref())
                .map_err(|e| ProximaDBError::Storage(StorageError::SstEngine(e.to_string())))?;

            for edge in neighbors {
                let neighbor_id = edge.to_node_id.clone();

                edges.push(SubgraphEdge {
                    from: current_id.clone(),
                    to: neighbor_id.clone(),
                    edge_type: edge.edge_type.clone(),
                });

                if visited.insert(neighbor_id.clone()) {
                    nodes.push(neighbor_id.clone());
                    queue.push_back((neighbor_id, depth + 1));
                }
            }
        }

        Ok(Subgraph { nodes, edges })
    }
}
