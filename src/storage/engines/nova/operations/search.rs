//! Search operations module for NOVA engine
//! Handles all search-related logic including hierarchical pruning and progressive refinement

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::compute::distance_computation::DistanceMetric;
use crate::core::search::bounded_queue::BoundedPriorityQueue;
use crate::core::search::results::OptimizedSearchRecord;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Handles all search operations for NOVA engine
pub struct NovaSearchOperations {
    filesystem: Arc<FilesystemFactory>,
    distance_engine: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
}

impl NovaSearchOperations {
    /// Create new search operations handler
    pub fn new(filesystem: Arc<FilesystemFactory>, distance_metric: DistanceMetric) -> Self {
        Self {
            filesystem,
            distance_engine: Arc::new(
                crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                    distance_metric,
                ),
            ),
        }
    }

    /// Search vectors with unified interface
    pub async fn search_vectors_unified(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        // Extract search parameters from context
        let query_vector = ctx
            .query_vector()
            .ok_or_else(|| anyhow::anyhow!("No query vector provided"))?;
        let k = ctx.top_k();
        let _distance_metric = ctx.distance_metric();
        let collection_id = &ctx.collection.id;
        let filter_expression = ctx.search_params.filter_expression.as_ref();

        info!(
            "🔍 NOVA: Searching with k={}, query_dim={}, filters={:?}",
            k,
            query_vector.len(),
            filter_expression.is_some()
        );

        let collection_size = 1000; // Default collection size estimate

        // For now, implement direct search logic here
        // Check if we should use progressive search
        if self.should_use_progressive_search(k, collection_size, filter_expression.is_some()) {
            self.search_with_progressive_refinement(ctx, collection_id)
                .await
        } else if self.should_use_streaming_search(k, collection_size) {
            self.search_with_streaming(ctx, collection_id).await
        } else {
            self.search_standard(ctx, collection_id).await
        }
    }

    /// Search with progressive refinement
    async fn search_with_progressive_refinement(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
        collection_id: &str,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        // For now, use standard search as progressive search needs more setup
        self.search_standard(ctx, collection_id).await
    }

    /// Search with streaming
    async fn search_with_streaming(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
        collection_id: &str,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        // For now, use standard search as streaming search needs more setup
        self.search_standard(ctx, collection_id).await
    }

    /// Standard search without optimization
    async fn search_standard(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
        _collection_id: &str,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        use crate::core::search::results::OptimizedSearchRecord;
        use crate::storage::engines::core::formats::columnar::UnifiedParquetReader;
        use crate::storage::persistence::filesystem::caching_filesystem::UnifiedCachingFilesystem;

        // Get search parameters from context
        let query_vector = ctx
            .query_vector()
            .ok_or_else(|| anyhow::anyhow!("No query vector provided"))?;
        let k = ctx.top_k();
        let filter_expression = ctx.search_params.filter_expression.as_ref();

        // Get files for the collection
        // NOVA stores files in {base_location}/{collection_id}/data (standard path)
        // Production behavior: metadata.storage_path is base_location
        let base_location = ctx
            .storage_url()
            .ok_or_else(|| anyhow::anyhow!("No storage path in context"))?;
        let collection_id = &ctx.collection.id;

        // Use standard collection data path (same as other engines)
        let data_path = proximadb_storage_common::storage_path::StoragePath::collection_data_path(
            base_location,
            collection_id,
        );

        debug!(
            "📂 NOVA search: base_location={}, collection_id={}",
            base_location, collection_id
        );
        debug!("📂 NOVA search: Constructed data_path={}", data_path);

        let fs = self.filesystem.get_filesystem(&data_path)?;

        // List files in the data directory
        let entries = match fs.list(&data_path).await {
            Ok(e) => e,
            Err(err) => {
                debug!(
                    "📂 NOVA search: Failed to list directory {}: {}",
                    data_path, err
                );
                return Ok(Vec::new());
            }
        };

        debug!(
            "📂 NOVA search: Listed {} entries in {}",
            entries.len(),
            data_path
        );
        for entry in &entries {
            debug!(
                "  - {} (is_dir={}, name={})",
                entry.url, entry.metadata.is_directory, entry.name
            );
        }

        let files: Vec<String> = entries
            .into_iter()
            .filter(|e| !e.metadata.is_directory && e.name.ends_with(".parquet"))
            .map(|e| format!("{}/{}", data_path, e.name))
            .collect();

        if files.is_empty() {
            debug!("📂 NOVA search: No parquet files found in {}", data_path);
            return Ok(Vec::new());
        }

        debug!(
            "📂 NOVA search: Found {} parquet files in {}",
            files.len(),
            data_path
        );

        // Use bounded priority queue to maintain only top-k results
        let mut priority_queue = BoundedPriorityQueue::new(k);
        let dimension = query_vector.len();

        // Track search statistics
        let mut files_scanned = 0usize;
        let total_files = files.len();

        for file_path in files {
            files_scanned += 1;

            // Create unified caching filesystem
            let fs = self.filesystem.get_filesystem(&file_path)?;
            let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
                fs,
                collection_id.to_string(),
                "nova".to_string(),
            ));

            let reader = UnifiedParquetReader::new(
                vec![file_path],
                dimension,
                self.filesystem.clone(),
                unified_fs,
                collection_id.to_string(),
                "nova".to_string(),
            )?;

            // Convert filter if provided
            let _metadata_filter = filter_expression.as_ref().map(|_f| {
                // Convert FilterExpression to MetadataFilter
                // This is a simplified conversion - real implementation would be more complex
                crate::storage::engines::core::formats::columnar::MetadataFilter {
                    conditions: vec![], // Would need proper conversion
                    logic: crate::storage::engines::core::formats::columnar::FilterLogic::And,
                }
            });

            let records = reader.read_all_records(10000, None).await?;

            // Compute distances and insert into bounded queue
            for record in records {
                let vector = record
                    .embeddings
                    .first()
                    .map_or(Vec::new(), |embedding| embedding.values.clone());
                let similarity_result = self.distance_engine.calculate_distance(
                    query_vector,
                    &vector,
                    &ctx.distance_metric(),
                );
                let wire_record = crate::proto::proximadb_v1::VectorRecord::from(&record);

                let search_record = OptimizedSearchRecord {
                    id: wire_record.id.clone(),
                    vector_id: Some(wire_record.id),
                    score: similarity_result.normalized_score,
                    similarity: Some(similarity_result.normalized_score),
                    vector: Some(Arc::new(vector)),
                    metadata: crate::core::search::results::sql_map_to_proxima(
                        wire_record.metadata,
                    ),
                    version: wire_record.version,
                    timestamp: wire_record.timestamp,
                    ..Default::default()
                };

                // Try to insert into bounded queue - only keeps top-k
                priority_queue.try_insert(search_record);
            }
        }

        // Log search statistics
        if total_files > 1 {
            debug!(
                "📊 NOVA search: scanned {}/{} files",
                files_scanned, total_files
            );
        }

        // Get sorted results from bounded queue
        let results = priority_queue.into_sorted_vec();

        Ok(results)
    }

    /// Determine if progressive search should be used
    fn should_use_progressive_search(
        &self,
        _k: usize,
        collection_size: usize,
        has_filter: bool,
    ) -> bool {
        // Use progressive search for large collections or complex filters
        let is_large_collection = collection_size > 100000;

        has_filter || is_large_collection
    }

    /// Determine if streaming search should be used
    fn should_use_streaming_search(&self, _k: usize, collection_size: usize) -> bool {
        // Use streaming for very large collections
        collection_size > 1000000
    }

    /// Search by vector ID
    pub async fn vector_by_id(
        &self,
        _collection_id: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        // Implement ID-based search using bloom filters and hierarchical index
        debug!("🔍 NOVA: Searching for vector ID: {}", vector_id);

        // This would use the hierarchical index and bloom filters for fast ID lookup
        // For now, return a placeholder
        Ok(None)
    }
}
