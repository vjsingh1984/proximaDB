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

//! # Graph Collection Service
//!
//! Manages graph collections metadata, schemas, and configurations following
//! the same pattern as vector collections service.

use crate::core::error::ProximaDBError;
use crate::proto::proximadb_v1::{CreateGraphRequest, GraphCollection, GraphSchema};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::info;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Graph Collection Service - manages graph metadata and configurations
pub struct GraphCollectionService {
    /// In-memory cache of graph collections metadata
    collections: Arc<DashMap<String, Arc<GraphCollection>>>,

    /// Storage backend for persistence
    // TODO: Add persistent storage for graph metadata

    /// Configuration
    max_graphs: usize,
    metadata_cache_size: usize,
}

impl GraphCollectionService {
    /// Create a new graph collection service
    pub fn new() -> Self {
        Self {
            collections: Arc::new(DashMap::new()),
            max_graphs: 1000,
            metadata_cache_size: 100,
        }
    }

    /// Create a new graph collection
    pub async fn create_graph(&self, request: CreateGraphRequest) -> Result<Arc<GraphCollection>> {
        let graph_id = &request.graph_id;

        // Check if graph already exists
        if self.collections.contains_key(graph_id) {
            return Err(ProximaDBError::InvalidInput(format!(
                "Graph collection '{}' already exists",
                graph_id
            )));
        }

        // Check limits
        if self.collections.len() >= self.max_graphs {
            return Err(ProximaDBError::InvalidInput(format!(
                "Maximum number of graphs ({}) exceeded",
                self.max_graphs
            )));
        }

        // Create graph collection metadata
        let now = chrono::Utc::now().timestamp();
        let collection = GraphCollection {
            graph_id: graph_id.clone(),
            name: request.name.unwrap_or_else(|| graph_id.clone()),
            description: request.description.unwrap_or_default(),
            schema: request.schema,
            storage_config: request.storage_config,
            engine_config: request.engine_config,
            access_control: request.access_control,
            stats: None, // Will be populated by operations service
            created_at: now,
            updated_at: now,
        };

        let collection_arc = Arc::new(collection);
        self.collections
            .insert(graph_id.clone(), collection_arc.clone());

        info!("Created graph collection: {}", graph_id);
        Ok(collection_arc)
    }

    /// Get graph collection metadata
    pub async fn get_graph(&self, graph_id: &str) -> Result<Option<Arc<GraphCollection>>> {
        Ok(self
            .collections
            .get(graph_id)
            .map(|entry| Arc::clone(&entry)))
    }

    /// Delete a graph collection
    pub async fn delete_graph(&self, graph_id: &str) -> Result<()> {
        match self.collections.remove(graph_id) {
            Some(_) => {
                info!("Deleted graph collection: {}", graph_id);
                Ok(())
            }
            None => Err(ProximaDBError::InvalidInput(format!(
                "Graph collection '{}' not found",
                graph_id
            ))),
        }
    }

    /// List all graph collections
    pub async fn list_graphs(&self) -> Result<Vec<Arc<GraphCollection>>> {
        Ok(self
            .collections
            .iter()
            .map(|entry| Arc::clone(&entry))
            .collect())
    }

    /// Update graph schema
    pub async fn update_schema(&self, graph_id: &str, schema: GraphSchema) -> Result<()> {
        if let Some(mut collection_ref) = self.collections.get_mut(graph_id) {
            // TODO: Implement proper schema validation
            // TODO: Handle schema migration if data exists

            let mut collection = (**collection_ref).clone();
            collection.schema = Some(schema);
            collection.updated_at = chrono::Utc::now().timestamp();

            *collection_ref = Arc::new(collection);

            info!("Updated schema for graph: {}", graph_id);
            Ok(())
        } else {
            Err(ProximaDBError::InvalidInput(format!(
                "Graph collection '{}' not found",
                graph_id
            )))
        }
    }

    /// Validate that a graph exists before operations
    pub async fn ensure_graph_exists(&self, graph_id: &str) -> Result<Arc<GraphCollection>> {
        self.get_graph(graph_id).await?.ok_or_else(|| {
            ProximaDBError::InvalidInput(format!("Graph collection '{}' does not exist", graph_id))
        })
    }

    /// Get graph statistics
    pub async fn get_graph_stats(
        &self,
        graph_id: &str,
    ) -> Result<Option<crate::proto::proximadb_v1::GraphStats>> {
        if let Some(collection) = self.get_graph(graph_id).await? {
            Ok(collection.stats.clone())
        } else {
            Err(ProximaDBError::InvalidInput(format!(
                "Graph collection '{}' not found",
                graph_id
            )))
        }
    }
}

impl Default for GraphCollectionService {
    fn default() -> Self {
        Self::new()
    }
}

// TODO: Add tests
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_graph() {
        let service = GraphCollectionService::new();

        let request = CreateGraphRequest {
            graph_id: "test_graph".to_string(),
            name: Some("Test Graph".to_string()),
            description: Some("A test graph".to_string()),
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };

        let result = service.create_graph(request).await;
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.graph_id, "test_graph");
        assert_eq!(graph.name, "Test Graph");
    }

    #[tokio::test]
    async fn test_duplicate_graph() {
        let service = GraphCollectionService::new();

        let request = CreateGraphRequest {
            graph_id: "test_graph".to_string(),
            name: Some("Test Graph".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };

        // First creation should succeed
        assert!(service.create_graph(request.clone()).await.is_ok());

        // Second creation should fail
        assert!(service.create_graph(request).await.is_err());
    }
}
