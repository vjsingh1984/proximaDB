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

use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, info, trace, warn};

use crate::compute::distance_computation::DistanceMetric;
use crate::core::search::bounded_queue::BoundedPriorityQueue;
use crate::core::search::results::OptimizedSearchRecord;
use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::index::axis::management::manager::{
    FilterOperator, HybridQuery, MetadataFilter, VectorQuery,
};
use crate::storage::engines::core::formats::arrow_block::ArrowBlockReader;
use crate::storage::engines::impls::sst::{SstEngine, SstError};
use crate::storage::traits::StorageQueryContext;

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
        let _search_start = std::time::Instant::now();

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
        // Use orchestration if:
        // 1. AXIS indexes are explicitly configured, OR
        // 2. Quantization is enabled, OR
        // 3. AXIS manager is available (for collections built after AXIS became available)
        let has_axis_manager = self.axis_manager().is_some();
        let use_orchestration =
            ctx.metadata.use_axis_indexes || ctx.metadata.has_quantization || has_axis_manager;

        if has_axis_manager {
            debug!("🔍 SST: AXIS manager is available for HNSW/IVF search");
        }

        if use_orchestration {
            // Use advanced orchestration when available
            self.execute_orchestrated_search(
                ctx,
                collection_id,
                &storage_url,
                query_vector,
                k,
                distance_metric,
                filter_expression,
            )
            .await
        } else {
            // Use direct search for simple queries
            self.execute_direct_search(
                ctx,
                collection_id,
                &storage_url,
                query_vector,
                k,
                distance_metric,
                filter_expression,
            )
            .await
        }
    }

    /// Execute orchestrated search with intelligent routing
    ///
    /// This method uses the AXIS manager for HNSW/IVF-based approximate search
    /// when available, falling back to direct search otherwise.
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

        // Check if AXIS manager is available for HNSW/IVF search
        if let Some(axis_manager) = self.axis_manager() {
            info!(
                "🔗 SST: AXIS manager available, attempting HNSW index search for collection {}",
                collection_id
            );

            // Convert filter expression to AXIS metadata filters
            let axis_filters = Self::convert_filter_to_axis(filter_expression);

            // Build hybrid query for AXIS
            let hybrid_query = HybridQuery {
                collection_id: collection_id.to_string(),
                vector_query: Some(VectorQuery::Dense {
                    vector: query_vector.to_vec(),
                    similarity_threshold: 0.0, // Return all results up to k
                }),
                metadata_filters: axis_filters,
                id_filters: Vec::new(),
                top_k: k,
                include_expired: false,
            };

            // Execute AXIS query (HNSW or IVF based on index type)
            let axis_start = std::time::Instant::now();
            match axis_manager.query(hybrid_query).await {
                Ok(axis_results) => {
                    let axis_duration = axis_start.elapsed();
                    info!(
                        "✅ SST: AXIS HNSW search completed in {:?} - found {} candidates",
                        axis_duration,
                        axis_results.results.len()
                    );

                    // Convert AXIS results to OptimizedSearchRecord
                    let results: Vec<OptimizedSearchRecord> = axis_results
                        .results
                        .into_iter()
                        .take(k)
                        .map(|scored| OptimizedSearchRecord {
                            id: scored.vector_id.to_string(),
                            vector_id: Some(scored.vector_id.to_string()),
                            score: scored.similarity,
                            similarity: Some(scored.similarity),
                            vector: None, // AXIS doesn't return vectors by default
                            ..Default::default()
                        })
                        .collect();

                    // If we need vectors or got fewer results, optionally refine with SST lookup
                    if results.is_empty() {
                        info!("⚠️ SST: AXIS returned no results, falling back to direct search");
                        return self
                            .fallback_to_direct_search(
                                ctx,
                                collection_id,
                                storage_url,
                                query_vector,
                                k,
                                distance_metric,
                                filter_expression,
                                true,
                                true,
                            )
                            .await;
                    }

                    return Ok(results);
                }
                Err(e) => {
                    warn!(
                        "⚠️ SST: AXIS query failed ({}), falling back to direct search",
                        e
                    );
                }
            }
        } else {
            debug!("🔍 SST: AXIS manager not available, using direct search");
        }

        // Fall back to direct search if AXIS is unavailable or failed
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
        )
        .await
    }

    /// Convert FilterExpression to AXIS MetadataFilter format
    fn convert_filter_to_axis(filter_expression: Option<&FilterExpression>) -> Vec<MetadataFilter> {
        let Some(filter) = filter_expression else {
            return Vec::new();
        };

        // Convert filter expressions to AXIS metadata filters
        let mut axis_filters = Vec::new();

        match filter {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                // Convert ComparisonOperator to AXIS FilterOperator
                let axis_operator = match operator {
                    ComparisonOperator::Equals => FilterOperator::Equals,
                    ComparisonOperator::NotEquals => FilterOperator::NotEquals,
                    ComparisonOperator::GreaterThan => FilterOperator::GreaterThan,
                    ComparisonOperator::GreaterThanOrEqual => FilterOperator::GreaterThan, // Approximate
                    ComparisonOperator::LessThan => FilterOperator::LessThan,
                    ComparisonOperator::LessThanOrEqual => FilterOperator::LessThan, // Approximate
                    ComparisonOperator::In => FilterOperator::In,
                    ComparisonOperator::NotIn => FilterOperator::NotIn,
                    _ => {
                        debug!(
                            "Operator {:?} not directly supported by AXIS, will use post-filtering",
                            operator
                        );
                        return axis_filters;
                    }
                };

                axis_filters.push(MetadataFilter {
                    field: field.clone(),
                    operator: axis_operator,
                    value: value.clone(),
                });
            }
            FilterExpression::And(filters) => {
                for f in filters {
                    axis_filters.extend(Self::convert_filter_to_axis(Some(f)));
                }
            }
            FilterExpression::Or(_) | FilterExpression::Not(_) => {
                // OR and NOT are not directly supported by AXIS, will use post-filtering
                debug!("OR/NOT filters not supported by AXIS, will use post-filtering");
            }
        }

        axis_filters
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
        )
        .await
    }

    /// Fallback direct search implementation
    ///
    /// This method implements a simplified but efficient search that:
    /// 1. Discovers relevant SSTable files
    /// 2. Searches each file using the unified reader
    /// 3. Combines and ranks results
    pub async fn fallback_to_direct_search(
        &self,
        ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        tracing::debug!(
            "[SST] Starting direct search for collection {}, storage_url: {}",
            collection_id,
            storage_url
        );

        let mut all_candidates = Vec::new();

        // Discover SSTable files for this collection with optional centroid pruning
        // When SearchMode is Approximate, uses centroid-based IVF-style optimization
        tracing::debug!(storage_url = %storage_url, "Discovering SSTable files");
        let search_mode = &ctx.search_params.search_mode;
        let prune_config = &ctx.search_params.block_prune; // [AGENT_FIX] Get prune config
        let sstable_files = self
            .discover_sstable_files_with_centroid_pruning(
                storage_url,
                query_vector,
                distance_metric,
                search_mode,
                prune_config, // [AGENT_FIX] Pass prune config down
            )
            .await?;
        tracing::debug!(
            "[SST] Discovered {} SSTable files (search_mode={:?})",
            sstable_files.len(),
            search_mode
        );
        for (i, file) in sstable_files.iter().enumerate() {
            tracing::trace!(index = i, file = %file, "Discovered SSTable file");
        }

        debug!(
            "🔍 SST: Found {} SSTable files for collection {}",
            sstable_files.len(),
            collection_id
        );

        // Search each SSTable file with block-level pruning
        // Uses Z-order spatial codes and centroid distances for intelligent block selection
        let block_prune = &ctx.search_params.block_prune;
        for (file_idx, sstable_path) in sstable_files.iter().enumerate() {
            trace!(
                "SST: Searching file [{}/{}]: {} (force_exact={})",
                file_idx + 1,
                sstable_files.len(),
                sstable_path,
                block_prune.force_exact
            );

            // Dispatch based on file format (Arrow vs ProximaBlocks)
            let search_result = if sstable_path.ends_with(".arrow") {
                // Use ArrowBlockReader for Arrow format files
                self.search_arrow_file(
                    sstable_path,
                    query_vector,
                    filter_expression.cloned(),
                    k * 2,
                    distance_metric,
                )
                .await
            } else {
                // Use SSTable reader for ProximaBlocks format
                self.sstable_reader()
                    .search_with_filter_and_pruning(
                        sstable_path,
                        query_vector,
                        filter_expression.cloned(),
                        k * 2, // Get more candidates for better accuracy
                        distance_metric,
                        Some(&*ctx.collection), // Pass collection for type-safe metadata deserialization
                        block_prune, // Pass block pruning config for Z-order/centroid pruning
                    )
                    .await
            };

            match search_result {
                Ok(results) => {
                    trace!(
                        "SST: Found {} candidates in file {}",
                        results.len(),
                        file_idx + 1
                    );
                    all_candidates.extend(results);
                }
                Err(e) => {
                    warn!("SST: Failed to search file {}: {}", sstable_path, e);
                    // Continue with other files
                }
            }
        }

        // Use bounded priority queue for efficient top-k selection
        let mut priority_queue = BoundedPriorityQueue::new(k);

        // Insert all candidates into bounded queue
        for candidate in all_candidates {
            priority_queue.try_insert(candidate);
        }

        // Get sorted results from bounded queue
        let mut all_candidates = priority_queue.into_sorted_vec();
        tracing::debug!(candidate_count = all_candidates.len(), "Before filtering");

        // Filter results based on include flags
        self.filter_search_results(&mut all_candidates, include_vectors, include_metadata);
        tracing::debug!(filtered_count = all_candidates.len(), "After filtering");

        info!(
            "🏁 SST: Direct search completed - Collection: {}, Results: {}/{}",
            collection_id,
            all_candidates.len(),
            k
        );

        Ok(all_candidates)
    }

    /// Discover SSTable files with optional centroid-based pruning (LanceDB-inspired IVF optimization)
    ///
    /// When `search_mode` is Approximate, this method:
    /// 1. Loads headers from all SST files to get centroids
    /// 2. Computes distance from query to each centroid
    /// 3. Returns only the top nprobe files (closest centroids to query)
    /// 4. This can skip 80-90% of files for large datasets
    async fn discover_sstable_files_with_centroid_pruning(
        &self,
        storage_url: &str,
        query_vector: &[f32],
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        search_mode: &crate::core::search::SearchMode,
        prune_config: &crate::core::search::BlockPruneConfig, // [AGENT_FIX] New parameter
    ) -> Result<Vec<String>> {
        use crate::core::search::SearchMode;

        // First get all files
        let all_files = self.discover_sstable_files(storage_url).await?;

        // [AGENT_FIX] Use the configured min_keep value instead of a hardcoded number.
        let min_keep = prune_config.min_keep.max(1);

        // OPTIMIZATION: Skip file-level pruning for small datasets where overhead exceeds benefit.
        // Loading centroids from each file header and computing distances has significant I/O
        // and CPU overhead. Only worth it when we have many files to prune.
        use crate::storage::engines::core::constants::pruning;
        if all_files.len() < pruning::MIN_FILES_FOR_PRUNING {
            tracing::debug!(
                "SST file pruning skipped: {} files < {} threshold (overhead would exceed benefit)",
                all_files.len(),
                pruning::MIN_FILES_FOR_PRUNING
            );
            return Ok(all_files);
        }

        // For exact mode or small datasets (<= min_keep), search all files
        if matches!(search_mode, SearchMode::Exact) || all_files.len() <= min_keep {
            if !matches!(search_mode, SearchMode::Exact) {
                tracing::warn!(
                    "Fewer than or equal to `min_keep` ({}) SST files, forcing exact search.",
                    min_keep
                );
            }
            return Ok(all_files);
        }

        // [AGENT_FIX] If the search mode is not explicitly 'Approximate' with a defined
        // pruning strategy, it should be treated as an exact search. This corrects a
        // flaw where default-constructed SearchParams would lead to an unintended
        // approximate search.
        if !matches!(search_mode, SearchMode::Approximate { .. }) {
            tracing::warn!(
                "Search mode is not 'Approximate', but centroid pruning was called. Forcing exact search by returning all files."
            );
            return Ok(all_files);
        }

        // For adaptive mode with small datasets, search all files
        if let SearchMode::Adaptive { threshold: _threshold } = search_mode {
            if all_files.len() <= 3 {
                return Ok(all_files);
            }
        }

        // Calculate effective nprobe based on search mode and number of files
        let nprobe = search_mode.effective_nprobe(all_files.len(), all_files.len() * 1000); // Estimate 1000 vectors per file

        // If nprobe >= number of files, search all
        if nprobe >= all_files.len() {
            return Ok(all_files);
        }

        // Load headers and compute centroid distances
        let mut file_distances: Vec<(String, f32)> = Vec::new();

        for file_path in &all_files {
            match self.load_sst_header_centroid(file_path).await {
                Ok(Some((centroid, _max_distance_to_centroid))) => {
                    if centroid.len() == query_vector.len() {
                        // Compute distance from query to file centroid
                        let distance = self.compute_centroid_distance(
                            query_vector,
                            &centroid,
                            distance_metric,
                        );
                        file_distances.push((file_path.clone(), distance));
                    } else {
                        // Dimension mismatch - include file anyway
                        file_distances.push((file_path.clone(), 0.0));
                    }
                }
                Ok(None) => {
                    // No centroid - include file anyway (for backwards compatibility)
                    file_distances.push((file_path.clone(), 0.0));
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load centroid from {}: {}, including anyway",
                        file_path,
                        e
                    );
                    file_distances.push((file_path.clone(), 0.0));
                }
            }
        }

        // Sort by distance (ascending - closest first for similarity search)
        file_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Return top nprobe files
        let selected_files: Vec<String> = file_distances
            .into_iter()
            .take(nprobe)
            .map(|(path, _)| path)
            .collect();

        debug!(
            "🎯 SST Centroid pruning: selected {}/{} files (nprobe={})",
            selected_files.len(),
            all_files.len(),
            nprobe
        );

        Ok(selected_files)
    }

    /// Load centroid from SST header for partition-aware search
    async fn load_sst_header_centroid(&self, file_path: &str) -> Result<Option<(Vec<f32>, f32)>> {
        use crate::storage::engines::impls::sst::SstableHeader;

        let fs = self.filesystem().get_filesystem(file_path)?;

        // Read just the first part of the file to get header
        // Format: SST1 (4 bytes) + header_len (4 bytes) + header data
        let header_prefix = fs.read_range(file_path, 0, 8).await?;

        // Verify magic
        if &header_prefix[0..4] != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SST file format"));
        }

        let header_len = u32::from_le_bytes([
            header_prefix[4],
            header_prefix[5],
            header_prefix[6],
            header_prefix[7],
        ]) as usize;

        // Read header data
        let header_data = fs.read_range(file_path, 8, header_len as u64).await?;

        // Deserialize header
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize SST header: {}", e))?;

        // Return centroid and max_distance if available
        if let Some(centroid) = header.centroid {
            let max_dist = header.max_distance_to_centroid.unwrap_or(f32::MAX);
            Ok(Some((centroid, max_dist)))
        } else {
            Ok(None)
        }
    }

    /// Compute distance from query to centroid
    fn compute_centroid_distance(
        &self,
        query: &[f32],
        centroid: &[f32],
        metric: crate::compute::distance_computation::DistanceMetric,
    ) -> f32 {
        use crate::compute::distance_computation::DistanceMetric;

        match metric {
            DistanceMetric::Euclidean => {
                let mut sum = 0.0f32;
                for i in 0..query.len().min(centroid.len()) {
                    let diff = query[i] - centroid[i];
                    sum += diff * diff;
                }
                sum.sqrt()
            }
            DistanceMetric::Cosine | DistanceMetric::DotProduct => {
                // For cosine/IP, we want to maximize similarity
                // Return 1 - cosine_similarity as "distance"
                let mut dot = 0.0f32;
                let mut norm_q = 0.0f32;
                let mut norm_c = 0.0f32;
                for i in 0..query.len().min(centroid.len()) {
                    dot += query[i] * centroid[i];
                    norm_q += query[i] * query[i];
                    norm_c += centroid[i] * centroid[i];
                }
                let denom = (norm_q * norm_c).sqrt();
                if denom > 0.0 {
                    1.0 - (dot / denom)
                } else {
                    1.0
                }
            }
            _ => {
                // Default to Euclidean for other metrics
                let mut sum = 0.0f32;
                for i in 0..query.len().min(centroid.len()) {
                    let diff = query[i] - centroid[i];
                    sum += diff * diff;
                }
                sum.sqrt()
            }
        }
    }

    /// Discover SSTable files for a collection
    async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
        tracing::debug!(
            "[SST] discover_sstable_files called with storage_url: {}",
            storage_url
        );

        let mut files = Vec::new();

        // storage_url is already the correct data directory path from collection_storage_path()
        // No need to parse and reconstruct - use it directly
        let data_url = storage_url;

        // List files in the collection directory
        let fs = self.filesystem().get_filesystem(data_url)?;
        tracing::debug!("[SST] Got filesystem for data_url: {}", data_url);

        // Handle case where directory doesn't exist yet (e.g., before first flush)
        let entries = match fs.list(data_url).await {
            Ok(entries) => {
                tracing::debug!("[SST] Found {} entries in {}", entries.len(), data_url);
                entries
            }
            Err(e) if e.to_string().contains("No such file or directory") => {
                tracing::warn!("[SST] Directory doesn't exist yet: {}", data_url);
                return Ok(files);
            }
            Err(e) => {
                tracing::error!("[SST] Failed to list directory {}: {:?}", data_url, e);
                return Err(anyhow::anyhow!(
                    "Failed to list directory {}: {}",
                    data_url,
                    e
                ));
            }
        };

        for entry in entries {
            tracing::trace!(
                "[SST] Examining entry: name={}, url={}, is_dir={}",
                entry.name,
                entry.url,
                entry.metadata.is_directory
            );
            if !entry.metadata.is_directory
                && (entry.name.ends_with(".sst") || entry.name.ends_with(".arrow"))
            {
                files.push(entry.url);
                tracing::debug!("[SST] Found data file: {}", entry.name);
            }
        }

        tracing::debug!(
            "[SST] Discovered {} .sst files in {}",
            files.len(),
            data_url
        );
        Ok(files)
    }

    /// Parse storage URL to extract base URL and collection ID
    fn parse_storage_url(&self, storage_url: &str) -> Result<(String, String)> {
        // Fallback: assume storage_url is base_url/collection_id format
        if let Some(last_slash) = storage_url.rfind('/') {
            let base = &storage_url[..last_slash];
            let collection = &storage_url[last_slash + 1..];
            Ok((base.to_string(), collection.to_string()))
        } else {
            Err(
                SstError::InvalidArgument(format!("Invalid storage URL format: {}", storage_url))
                    .into(),
            )
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
                    if name.ends_with(".sst") || name.ends_with(".arrow") {
                        sstable_files.push(format!("{}/{}", data_dir, name));
                    }
                }
            }
        }

        debug!(
            "📋 Listed {} SSTable files in {}",
            sstable_files.len(),
            data_dir
        );
        Ok(sstable_files)
    }

    /// Search within an Arrow format file
    ///
    /// Uses ArrowBlockReader to read and search through Arrow IPC files,
    /// providing the same interface as SSTable searches for seamless integration.
    async fn search_arrow_file(
        &self,
        arrow_path: &str,
        query_vector: &[f32],
        _filter_expression: Option<FilterExpression>,
        limit: usize,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        use crate::compute::distance_computation::engine::{
            SimilarityResult, UnifiedDistanceCompute,
        };
        use std::sync::Arc;

        debug!("🏹 Searching Arrow file: {}", arrow_path);

        // Convert file:// URL to local path
        let local_path = arrow_path.strip_prefix("file://").unwrap_or(arrow_path);

        // Open the Arrow file reader
        let reader = ArrowBlockReader::open(local_path)
            .map_err(|e| anyhow::anyhow!("Failed to open Arrow file {}: {}", arrow_path, e))?;

        // Read all records from the Arrow file
        let records = reader
            .read_all()
            .map_err(|e| anyhow::anyhow!("Failed to read Arrow file {}: {}", arrow_path, e))?;

        trace!("🏹 Arrow file contains {} records", records.len());

        // Create distance computer with the specified metric
        let distance_computer = UnifiedDistanceCompute::new(distance_metric);

        // Score all records
        // Note: Metadata filtering for Arrow files is simplified - for full filter support,
        // use ProximaBlocks format which has optimized filter evaluation
        let mut candidates: Vec<OptimizedSearchRecord> =
            Vec::with_capacity(records.len().min(limit));

        for record in records.iter() {
            // Compute raw distance
            let raw_distance = distance_computer.distance(query_vector, &record.vector);

            // Use SimilarityResult to get normalized_score (higher = more similar)
            // This ensures consistency with the rest of the codebase and BoundedPriorityQueue
            let similarity_result = SimilarityResult::new(raw_distance, distance_metric);

            // VectorRecord.metadata is already HashMap<String, SqlValue>
            // Clone it directly for use in OptimizedSearchRecord
            let metadata = record.metadata.clone();

            candidates.push(OptimizedSearchRecord {
                id: record.id.clone(),
                vector_id: Some(record.id.clone()),
                score: similarity_result.normalized_score, // Use normalized_score (higher = better)
                similarity: Some(similarity_result.normalized_score),
                vector: Some(Arc::new(record.vector.clone())),
                metadata,
                ..Default::default()
            });
        }

        // Sort by score descending (higher normalized_score = more similar = better)
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(limit);

        debug!("🏹 Arrow search found {} candidates", candidates.len());
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::storage::engines::impls::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_parse_storage_url() {
        let engine = create_test_engine().await;

        // Test valid storage URL
        let (base, collection) = engine
            .parse_storage_url("file:///data/collections/test_collection")
            .unwrap();
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
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute)
            .await
            .unwrap()
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
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    "test_value".to_string(),
                )),
            };
            metadata.insert("test_key".to_string(), sql_value);
            metadata
        };
        record
    }
}
