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

//! SST Engine Search Module
//!
//! Contains search operations and coordination logic for the SST engine.
//! This module implements the three-stage filtering pipeline:
//! 1. Bloom filter stage - eliminate non-matching SST files
//! 2. Row filter stage - filter records within SST files
//! 3. Vector stage - compute distances for remaining candidates
//!
//! The module provides:
//! - Main unified search implementation
//! - Direct search fallback for simple queries
//! - Search coordination and optimization
//! - File discovery and routing logic

pub mod coordinator;
pub mod operations;
pub mod optimizer;

use std::collections::HashMap;
use std::sync::Arc;
use anyhow::{Result, Context};
use tracing::{info, debug, warn};

use crate::storage::engines::impls::sst::{SstEngine, SstError};
use crate::storage::traits::StorageQueryContext;
use crate::core::search::results::OptimizedSearchRecord;
use crate::core::search::FilterExpression;
use crate::compute::distance_computation::DistanceMetric;

pub use coordinator::SearchCoordinator;
pub use operations::SearchOperations;
pub use optimizer::SearchOptimizer;

impl SstEngine {
    /// Main unified search implementation with orchestration
    ///
    /// This is the primary search entry point that implements intelligent
    /// search routing and the three-stage filtering pipeline.
    pub async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let search_start = std::time::Instant::now();

        // Track metadata access for cache optimization
        if let Some(orch) = self.orchestrator() {
            (**orch).pattern_tracker().track_access_async(
                format!("{}::sst::metadata", ctx.collection_id()),
                crate::storage::cache::orchestrator::CacheType::Metadata,
            );
        }

        // Extract search parameters from context
        let collection_id = ctx.collection_id();
        let storage_url = ctx
            .collection_storage_path()
            .ok_or_else(|| SstError::InvalidArgument("No storage URL in context".into()))?;
        let query_vector = ctx
            .query_vector()
            .ok_or_else(|| SstError::InvalidArgument("No query vector in context".into()))?;
        let k = ctx.top_k();
        let distance_metric = ctx.distance_metric();
        let filter_expression = ctx.search_params.filter_expression.as_ref();

        info!(
            "🚀 SST: Starting unified search for collection {} with {} dimensions",
            collection_id,
            query_vector.len()
        );

        // Determine search strategy based on context
        let use_orchestration = ctx.metadata.use_axis_indexes || ctx.metadata.has_quantization;

        if use_orchestration {
            // Use advanced orchestration when available
            self.execute_orchestrated_search(ctx, collection_id, &storage_url, query_vector, k, distance_metric, filter_expression).await
        } else {
            // Use direct search for simple queries
            self.execute_direct_search(ctx, collection_id, &storage_url, query_vector, k, distance_metric, filter_expression).await
        }
    }

    /// Execute orchestrated search with intelligent routing
    async fn execute_orchestrated_search(
        &self,
        ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        info!("🎯 SST: Using orchestrated search strategy");

        // For now, fall back to direct search since full orchestration requires
        // integration with AdvancedSearchOptimizer which is not yet available
        warn!("🔄 SST: Orchestration requested but falling back to direct search until integration complete");

        self.fallback_to_direct_search(
            ctx,
            collection_id,
            storage_url,
            query_vector,
            k,
            distance_metric,
            filter_expression,
            true, // include_vectors
            true, // include_metadata
        ).await
    }

    /// Execute direct search without orchestration
    async fn execute_direct_search(
        &self,
        ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        info!("🔍 SST: Using direct search strategy");

        self.fallback_to_direct_search(
            ctx,
            collection_id,
            storage_url,
            query_vector,
            k,
            distance_metric,
            filter_expression,
            true, // include_vectors
            true, // include_metadata
        ).await
    }

    /// Fallback direct search implementation
    ///
    /// This method implements a simplified but efficient search that:
    /// 1. Discovers relevant SSTable files
    /// 2. Searches each file using the unified reader
    /// 3. Combines and ranks results
    pub async fn fallback_to_direct_search(
        &self,
        _ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        debug!("🔄 SST: Starting direct search for collection {}", collection_id);

        let mut all_candidates = Vec::new();

        // Discover SSTable files for this collection
        let sstable_files = self.discover_sstable_files(storage_url).await?;

        debug!(
            "🔍 SST: Found {} SSTable files for collection {}",
            sstable_files.len(),
            collection_id
        );

        // Search each SSTable file
        for sstable_path in &sstable_files {
            debug!("🔍 SST: Searching SSTable: {}", sstable_path);

            match self.sstable_reader().search_with_filter(
                sstable_path,
                query_vector,
                filter_expression.cloned(),
                k * 2, // Get more candidates for better accuracy
                distance_metric,
            ).await {
                Ok(results) => {
                    debug!("📊 Found {} candidates in {}", results.len(), sstable_path);
                    all_candidates.extend(results);
                }
                Err(e) => {
                    warn!("⚠️ Failed to search SSTable {}: {}", sstable_path, e);
                    // Continue with other files
                }
            }
        }

        // Sort candidates by score and take top-k
        all_candidates.sort_by(|a, b| {
            a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        all_candidates.truncate(k);

        // Filter results based on include flags
        self.filter_search_results(&mut all_candidates, include_vectors, include_metadata);

        info!(
            "🏁 SST: Direct search completed - Collection: {}, Results: {}/{}",
            collection_id,
            all_candidates.len(),
            k
        );

        Ok(all_candidates)
    }

    /// Discover SSTable files for a collection
    async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
        let mut files = Vec::new();

        // storage_url is already the correct data directory path from collection_storage_path()
        // No need to parse and reconstruct - use it directly
        let data_url = storage_url;

        debug!("🔍 SST discover_sstable_files: Looking for .sst files in {}", data_url);

        // List files in the collection directory
        let fs = self.filesystem().get_filesystem(data_url)?;
        let entries = fs.list(data_url).await?;

        for entry in entries {
            if !entry.metadata.is_directory && entry.name.ends_with(".sst") {
                files.push(entry.url);
                debug!("   Found SST file: {}", entry.name);
            }
        }

        debug!("📂 Discovered {} SST files in {}", files.len(), data_url);
        Ok(files)
    }

    /// Parse storage URL to extract base URL and collection ID
    fn parse_storage_url(&self, storage_url: &str) -> Result<(String, String)> {
        if let Some((base, coll)) = crate::utils::StoragePath::parse_collection_path(
            &format!("{}/dummy", storage_url)
        ) {
            Ok((base, coll))
        } else {
            // Fallback: assume storage_url is base_url/collection_id format
            if let Some(last_slash) = storage_url.rfind('/') {
                let base = &storage_url[..last_slash];
                let collection = &storage_url[last_slash + 1..];
                Ok((base.to_string(), collection.to_string()))
            } else {
                Err(SstError::InvalidArgument(format!("Invalid storage URL format: {}", storage_url)).into())
            }
        }
    }

    /// Filter search results based on include flags
    fn filter_search_results(
        &self,
        results: &mut Vec<OptimizedSearchRecord>,
        include_vectors: bool,
        include_metadata: bool,
    ) {
        if !include_vectors {
            for result in results.iter_mut() {
                result.vector = None;
            }
        }

        if !include_metadata {
            for result in results.iter_mut() {
                result.metadata = HashMap::new();
            }
        }
    }

    /// List SSTable files for search in a specific directory
    pub async fn list_sstable_files_for_search(&self, data_dir: &str) -> Result<Vec<String>> {
        let mut sstable_files = Vec::new();

        // Use filesystem to list files directly
        if let Ok(mut entries) = tokio::fs::read_dir(data_dir).await {
            while let Some(entry) = entries.next_entry().await? {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".sst") {
                        sstable_files.push(format!("{}/{}", data_dir, name));
                    }
                }
            }
        }

        debug!("📋 Listed {} SSTable files in {}", sstable_files.len(), data_dir);
        Ok(sstable_files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::impls::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;

    #[tokio::test]
    async fn test_parse_storage_url() {
        let engine = create_test_engine().await;

        // Test valid storage URL
        let (base, collection) = engine.parse_storage_url("file:///data/collections/test_collection").unwrap();
        assert_eq!(base, "file:///data/collections");
        assert_eq!(collection, "test_collection");

        // Test invalid storage URL
        assert!(engine.parse_storage_url("invalid_url").is_err());
    }

    #[tokio::test]
    async fn test_filter_search_results() {
        let engine = create_test_engine().await;
        let mut results = vec![
            create_test_search_result("id1", vec![1.0, 2.0], 0.5),
            create_test_search_result("id2", vec![3.0, 4.0], 0.3),
        ];

        // Test removing vectors
        engine.filter_search_results(&mut results, false, true);
        assert!(results[0].vector.is_none());
        assert!(results[1].vector.is_none());

        // Test removing metadata
        let mut results = vec![create_test_search_result("id1", vec![1.0, 2.0], 0.5)];
        engine.filter_search_results(&mut results, true, false);
        assert!(results[0].metadata.is_empty());
    }

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute).await.unwrap()
    }

    fn create_test_search_result(id: &str, values: Vec<f32>, score: f32) -> OptimizedSearchRecord {
        let mut record = OptimizedSearchRecord::default();
        record.id = id.to_string();
        record.score = score;
        record.vector = Some(Arc::new(values));
        record.metadata = {
            let mut metadata = HashMap::new();
            // Convert to SqlValue for proper metadata type
            let sql_value = crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("test_value".to_string())),
            };
            metadata.insert("test_key".to_string(), sql_value);
            metadata
        };
        record
    }
}