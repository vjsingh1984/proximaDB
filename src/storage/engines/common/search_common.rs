//! Universal search components shared across all storage engines
//!
//! This module provides the common search pipeline infrastructure that all engines
//! can leverage while maintaining their specific optimizations.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use futures::stream::{self, StreamExt};

use crate::core::search::{SearchResult, FilterExpression};
use crate::core::VectorRecord;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::distance_computation::DistanceMetric;
use crate::compute::quantization::unified::UnifiedQuantizationEngine;

/// Configuration for the universal search pipeline
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Number of results to return
    pub top_k: usize,
    
    /// Distance metric to use
    pub distance_metric: DistanceMetric,
    
    /// Optional filter expression
    pub filter: Option<FilterExpression>,
    
    /// Include vectors in results
    pub include_vectors: bool,
    
    /// Include metadata in results
    pub include_metadata: bool,
    
    /// Enable final reranking stage
    pub enable_reranking: bool,
    
    /// Maximum files to search in parallel
    pub max_parallel_files: usize,
    
    /// Enable progressive quantization
    pub enable_progressive_search: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            top_k: 10,
            distance_metric: DistanceMetric::Cosine,
            filter: None,
            include_vectors: false,
            include_metadata: true,
            enable_reranking: true,
            max_parallel_files: 4,
            enable_progressive_search: true,
        }
    }
}

/// Configuration for progressive quantization search
#[derive(Debug, Clone)]
pub struct ProgressiveConfig {
    /// Enable binary filtering stage
    pub enable_binary: bool,
    
    /// Threshold for binary filtering (0.0 - 1.0)
    pub binary_threshold: f32,
    
    /// Enable INT8 approximation stage
    pub enable_int8: bool,
    
    /// Number of candidates to keep after INT8
    pub int8_top_k: usize,
    
    /// Enable product quantization stage
    pub enable_pq: bool,
    
    /// Number of candidates to keep after PQ
    pub pq_top_k: usize,
    
    /// Final number of results
    pub final_top_k: usize,
}

impl Default for ProgressiveConfig {
    fn default() -> Self {
        Self {
            enable_binary: true,
            binary_threshold: 0.7,
            enable_int8: true,
            int8_top_k: 100,
            enable_pq: true,
            pq_top_k: 50,
            final_top_k: 10,
        }
    }
}

/// Trait for searchable files (implemented by each engine)
pub trait SearchableFile: Send + Sync {
    /// Get unique identifier for the file
    fn id(&self) -> &str;
    
    /// Get file size in bytes
    fn size(&self) -> u64;
    
    /// Check if file might contain relevant data (bloom filter, metadata, etc.)
    fn might_contain(&self, filter: &Option<FilterExpression>) -> bool;
}

/// Trait for searchable blocks within files
pub trait SearchableBlock: Send + Sync {
    /// Get block identifier
    fn id(&self) -> &str;
    
    /// Get records in this block
    fn records(&self) -> &[VectorRecord];
    
    /// Check if block is relevant for the search
    fn is_relevant(&self, filter: &Option<FilterExpression>) -> bool;
}

/// Trait for engine-specific file searching
#[async_trait]
pub trait FileSearcher<F: SearchableFile, B: SearchableBlock>: Send + Sync {
    /// Search a single file
    async fn search_file(
        &self,
        file: &F,
        query_vector: &[f32],
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>>;
    
    /// Get searchable blocks from a file
    async fn get_blocks(&self, file: &F) -> Result<Vec<B>>;
    
    /// Search a single block
    async fn search_block(
        &self,
        block: &B,
        query_vector: &[f32],
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>>;
}

/// Universal search pipeline used by all engines
pub struct UniversalSearchPipeline {
    distance_compute: Arc<UnifiedDistanceCompute>,
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    filter_processor: Arc<FilterProcessor>,
    result_manager: Arc<ResultManager>,
}

impl UniversalSearchPipeline {
    pub fn new(
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        Self {
            distance_compute: distance_compute.clone(),
            quantization_engine,
            filter_processor: Arc::new(FilterProcessor::new()),
            result_manager: Arc::new(ResultManager::new(distance_compute)),
        }
    }
    
    /// Common search pipeline for all engines
    pub async fn search_pipeline<F, B, S>(
        &self,
        files: Vec<F>,
        query_vector: &[f32],
        config: SearchConfig,
        file_searcher: Arc<S>,
    ) -> Result<Vec<SearchResult>> 
    where
        F: SearchableFile + Send + 'static,
        B: SearchableBlock + Send + 'static,
        S: FileSearcher<F, B> + 'static,
    {
        // 1. File-level filtering
        let filtered_files = self.filter_files(files, &config.filter)?;
        
        if filtered_files.is_empty() {
            return Ok(Vec::new());
        }
        
        // 2. Parallel file search with controlled concurrency
        let file_results = self.search_files_parallel(
            filtered_files,
            query_vector,
            &config,
            file_searcher,
        ).await?;
        
        // 3. Merge and rank results
        let mut merged = self.result_manager.merge_results(file_results)?;
        
        // 4. Apply final ranking
        merged = self.result_manager.rank_by_distance(merged, &config.distance_metric)?;
        
        // 5. Select top-k
        merged = self.result_manager.select_top_k(merged, config.top_k)?;
        
        // 6. Final reranking if enabled
        if config.enable_reranking && !merged.is_empty() {
            merged = self.rerank_results(merged, query_vector, &config).await?;
        }
        
        // 7. Include/exclude fields as requested
        self.result_manager.apply_field_config(merged, &config)
    }
    
    /// Filter files based on bloom filters and metadata
    fn filter_files<F: SearchableFile>(
        &self,
        files: Vec<F>,
        filter: &Option<FilterExpression>,
    ) -> Result<Vec<F>> {
        if filter.is_none() {
            return Ok(files);
        }
        
        Ok(files.into_iter()
            .filter(|f| f.might_contain(filter))
            .collect())
    }
    
    /// Search files in parallel with controlled concurrency
    async fn search_files_parallel<F, B, S>(
        &self,
        files: Vec<F>,
        query_vector: &[f32],
        config: &SearchConfig,
        file_searcher: Arc<S>,
    ) -> Result<Vec<Vec<SearchResult>>>
    where
        F: SearchableFile + Send + 'static,
        B: SearchableBlock + Send + 'static,
        S: FileSearcher<F, B> + 'static,
    {
        let query_vector = query_vector.to_vec();
        let config = config.clone();
        
        // Create futures for parallel search
        let search_futures = files.into_iter().map(move |file| {
            let searcher = file_searcher.clone();
            let query = query_vector.clone();
            let cfg = config.clone();
            
            async move {
                searcher.search_file(&file, &query, &cfg).await
            }
        });
        
        // Execute with controlled parallelism
        let results: Vec<Result<Vec<SearchResult>>> = stream::iter(search_futures)
            .buffer_unordered(config.max_parallel_files)
            .collect()
            .await;
        
        // Collect successful results
        results.into_iter().collect()
    }
    
    /// Rerank top results with full precision
    async fn rerank_results(
        &self,
        candidates: Vec<SearchResult>,
        query_vector: &[f32],
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>> {
        if !config.include_vectors {
            // Can't rerank without vectors
            return Ok(candidates);
        }
        
        // Recompute distances with full precision
        let mut reranked = Vec::with_capacity(candidates.len());
        
        for mut result in candidates {
            if let Some(ref vector) = result.vector {
                let distance = self.distance_compute.as_ref().compute_distance(
                    query_vector,
                    vector,
                    &config.distance_metric,
                )?;
                result.distance = Some(distance);
            }
            reranked.push(result);
        }
        
        // Sort by distance
        reranked.sort_by(|a, b| {
            a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        Ok(reranked)
    }
    
    /// Progressive quantization search implementation
    pub async fn progressive_search(
        &self,
        records: Vec<VectorRecord>,
        query_vector: &[f32],
        config: &ProgressiveConfig,
    ) -> Result<Vec<SearchResult>> {
        let mut candidates = records;
        
        // Stage 1: Binary filtering
        if config.enable_binary && !candidates.is_empty() {
            candidates = self.binary_filter(candidates, query_vector, config.binary_threshold).await?;
        }
        
        // Stage 2: INT8 approximation
        if config.enable_int8 && !candidates.is_empty() {
            candidates = self.int8_rank(candidates, query_vector, config.int8_top_k).await?;
        }
        
        // Stage 3: PQ ranking
        if config.enable_pq && !candidates.is_empty() {
            candidates = self.pq_rank(candidates, query_vector, config.pq_top_k).await?;
        }
        
        // Stage 4: Full precision ranking
        self.full_precision_rank(candidates, query_vector, config.final_top_k).await
    }
    
    /// Binary filtering stage
    async fn binary_filter(
        &self,
        records: Vec<VectorRecord>,
        query_vector: &[f32],
        threshold: f32,
    ) -> Result<Vec<VectorRecord>> {
        // Use quantization engine for binary filtering
        let binary_query = self.quantization_engine.quantize_binary(query_vector)?;
        
        let mut filtered = Vec::new();
        for record in records {
            if let Some(ref binary) = record.quantized_vector {
                let similarity = self.quantization_engine.binary_similarity(&binary_query, binary)?;
                if similarity >= threshold {
                    filtered.push(record);
                }
            } else {
                // No binary quantization, include by default
                filtered.push(record);
            }
        }
        
        Ok(filtered)
    }
    
    /// INT8 approximation stage
    async fn int8_rank(
        &self,
        records: Vec<VectorRecord>,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<VectorRecord>> {
        // This would use INT8 quantization from the quantization engine
        // For now, return top_k records
        let mut candidates = records;
        candidates.truncate(top_k.min(candidates.len()));
        Ok(candidates)
    }
    
    /// Product quantization ranking stage
    async fn pq_rank(
        &self,
        records: Vec<VectorRecord>,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<VectorRecord>> {
        // This would use PQ from the quantization engine
        // For now, return top_k records
        let mut candidates = records;
        candidates.truncate(top_k.min(candidates.len()));
        Ok(candidates)
    }
    
    /// Full precision final ranking
    async fn full_precision_rank(
        &self,
        records: Vec<VectorRecord>,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        let mut results = Vec::with_capacity(records.len());
        
        for record in records {
            let distance = self.distance_compute.as_ref().compute_distance(
                query_vector,
                &record.vector,
                &DistanceMetric::Cosine, // Use default for now
            )?;
            
            results.push(SearchResult {
                id: record.id.unwrap_or_default(),
                similarity: distance,
                vector: record.vector,
                metadata: record.metadata,
                similarity: 1.0 - distance, // Convert distance to similarity score
                // rank removed -  0, // Default rank
                version: record.updated_at,
                timestamp: Some(record.timestamp),
                collection_id: None, // Default collection_id
            });
        }
        
        // Sort by distance
        results.sort_by(|a, b| {
            a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        results.truncate(top_k.min(results.len()));
        Ok(results)
    }
}

/// Filter processor for handling filter expressions
pub struct FilterProcessor {
    // Could add metadata index here for optimization
}

impl FilterProcessor {
    pub fn new() -> Self {
        Self {}
    }
    
    pub fn process_filter(&self, filter: &FilterExpression) -> Result<FilterPlan> {
        // Convert filter expression to execution plan
        Ok(FilterPlan {
            // Implementation details
        })
    }
    
    pub fn apply_to_metadata(
        &self,
        metadata: &Option<serde_json::Value>,
        filter: &FilterExpression,
    ) -> bool {
        // Apply filter to metadata
        // This is a simplified implementation
        if metadata.is_none() {
            return false;
        }
        
        // Actual filter logic would go here
        true
    }
    
    pub fn optimize_filter(&self, filter: FilterExpression) -> FilterExpression {
        // Optimize filter expression (e.g., push down predicates)
        filter
    }
}

/// Execution plan for filters
pub struct FilterPlan {
    // Implementation details
}

/// Result manager for handling search results
pub struct ResultManager {
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl ResultManager {
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
    
    /// Merge multiple result sets
    pub fn merge_results(&self, results: Vec<Vec<SearchResult>>) -> Result<Vec<SearchResult>> {
        let mut merged = Vec::new();
        for result_set in results {
            merged.extend(result_set);
        }
        Ok(merged)
    }
    
    /// Rank results by distance
    pub fn rank_by_distance(
        &self,
        mut results: Vec<SearchResult>,
        _distance_metric: &DistanceMetric,
    ) -> Result<Vec<SearchResult>> {
        results.sort_by(|a, b| {
            a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }
    
    /// Select top-k results
    pub fn select_top_k(&self, mut results: Vec<SearchResult>, k: usize) -> Result<Vec<SearchResult>> {
        results.truncate(k.min(results.len()));
        Ok(results)
    }
    
    /// Apply field configuration to results
    pub fn apply_field_config(
        &self,
        mut results: Vec<SearchResult>,
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>> {
        for result in &mut results {
            if !config.include_vectors {
                result.vector = None;
            }
            if !config.include_metadata {
                result.metadata = None;
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_search_config_default() {
        let config = SearchConfig::default();
        assert_eq!(config.top_k, 10);
        assert_eq!(config.max_parallel_files, 4);
        assert!(config.enable_progressive_search);
    }
    
    #[test]
    fn test_progressive_config_default() {
        let config = ProgressiveConfig::default();
        assert!(config.enable_binary);
        assert_eq!(config.final_top_k, 10);
    }
}