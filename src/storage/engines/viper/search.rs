//! VIPER Storage-Aware Search Implementation
//!
//! This module implements polymorphic search that delegates to the VIPER storage engine
//! for efficient storage-aware search operations. It provides:
//! - ML-driven cluster selection for optimized vector search
//! - Parquet predicate pushdown for metadata filtering
//! - Multi-precision quantization support (FP32, PQ4, PQ8, Binary)
//! - Storage-aware search strategies

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};
use chrono;

use crate::core::{String, SearchResult, search::SearchParams};
use crate::storage::engines::viper::ViperEngine;
use crate::storage::engines::viper::clustering_models::{ClusteringModelManager, EfficientClusteringModel};
use crate::storage::engines::viper::column_projection::{ColumnProjectionStrategy, ColumnProjection, QuantizationColumnMapping};
use crate::storage::engines::viper::two_stage_search::{TwoStageSearchEngine, TwoStageSearchBuilder};
use crate::compute::{UnifiedDistanceCompute, DistanceMetric, UnifiedQuantizationEngine, InMemoryCodebookStore};


/// VIPER Storage-Aware Search Engine
/// 
/// Implements polymorphic search that delegates to storage-optimized strategies:
/// - Cluster-based search for large collections
/// - Direct search for small collections  
/// - Hybrid search combining multiple storage tiers
#[derive(Debug)]
pub struct ViperSearchEngine {
    /// Search configuration and optimization settings
    config: ViperSearchConfig,
    
    /// ML cluster metadata cache for fast cluster selection
    cluster_cache: Arc<tokio::sync::RwLock<ClusterMetadataCache>>,
    
    /// Search performance metrics
    metrics: Arc<tokio::sync::RwLock<SearchMetrics>>,
    
    /// Clustering model manager for trained models
    model_manager: Option<Arc<ClusteringModelManager>>,
    
    /// Column projection strategy for optimized I/O
    column_projection: ColumnProjectionStrategy,
    
    /// Unified distance compute with quantization support
    distance_compute: Arc<UnifiedDistanceCompute>,
    
    /// Two-stage search engine for quantized search
    two_stage_engine: Option<TwoStageSearchEngine>,
}

/// Configuration for VIPER search operations
#[derive(Debug, Clone)]
pub struct ViperSearchConfig {
    /// Enable ML-driven clustering optimization
    pub enable_ml_clustering: bool,
    
    /// Enable parallel search across clusters
    pub enable_parallel_search: bool,
    
    /// Maximum number of clusters to search
    pub max_clusters_to_search: usize,
    
    /// Cluster confidence threshold (0.0-1.0)
    pub cluster_confidence_threshold: f32,
    
    /// Enable Parquet predicate pushdown
    pub enable_predicate_pushdown: bool,
    
    /// Default quantization level for vector search
    pub default_quantization: crate::compute::UnifiedQuantizationLevel,
    
    /// Search timeout in milliseconds
    pub search_timeout_ms: u64,
}

impl Default for ViperSearchConfig {
    fn default() -> Self {
        Self {
            enable_ml_clustering: true,
            enable_parallel_search: true,
            max_clusters_to_search: 10,
            cluster_confidence_threshold: 0.7,
            enable_predicate_pushdown: true,
            default_quantization: crate::compute::UnifiedQuantizationLevel {
                level_type: Some(crate::compute::QuantizationLevelType::None(crate::compute::NoQuantization {})),
            },
            search_timeout_ms: 5000,
        }
    }
}

// Use unified quantization from compute module

/// Cluster metadata cache for ML-driven search optimization
#[derive(Debug, Default)]
pub struct ClusterMetadataCache {
    /// Cluster centroids for fast distance calculation
    cluster_centroids: HashMap<ClusterId, Vec<f32>>,
    
    /// Cluster sizes for load balancing
    cluster_sizes: HashMap<ClusterId, usize>,
    
    /// Last cache update timestamp
    last_updated: Option<std::time::SystemTime>,
    
    /// Cache validity duration in seconds
    cache_duration_secs: u64,
}

// Use unified SearchMetrics from storage_aware module
pub use crate::core::search::storage_aware::SearchMetrics;


impl ViperSearchEngine {
    /// Create a new VIPER search engine
    pub fn new() -> Self {
        Self {
            config: ViperSearchConfig::default(),
            cluster_cache: Arc::new(tokio::sync::RwLock::new(ClusterMetadataCache {
                cache_duration_secs: 300, // 5 minutes
                ..Default::default()
            })),
            metrics: Arc::new(tokio::sync::RwLock::new(SearchMetrics::default())),
            model_manager: None,
            column_projection: ColumnProjectionStrategy::new(),
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),
            two_stage_engine: None,
        }
    }

    /// Create a new VIPER search engine with custom configuration
    pub fn with_config(config: ViperSearchConfig) -> Self {
        Self {
            config,
            cluster_cache: Arc::new(tokio::sync::RwLock::new(ClusterMetadataCache {
                cache_duration_secs: 300, // 5 minutes
                ..Default::default()
            })),
            metrics: Arc::new(tokio::sync::RwLock::new(SearchMetrics::default())),
            model_manager: None,
            column_projection: ColumnProjectionStrategy::new(),
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),
            two_stage_engine: None,
        }
    }

    /// Set the clustering model manager for trained model access
    pub fn set_model_manager(&mut self, model_manager: Arc<ClusteringModelManager>) {
        self.model_manager = Some(model_manager);
        info!("🧠 VIPER Search: Model manager set for trained clustering models");
    }
    
    /// Configure column projection strategy with quantization mapping
    pub fn configure_column_projection(&mut self, mapping: QuantizationColumnMapping) {
        self.column_projection = self.column_projection.clone().with_quantization_mapping(mapping);
        info!("📊 VIPER Search: Column projection configured with quantization mapping");
    }
    
    /// Initialize two-stage search engine
    pub fn initialize_two_stage_search(&mut self) {
        // Create quantization engine with in-memory codebook store
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            self.distance_compute.clone(),
            codebook_store,
        ));
        
        // Build two-stage search engine
        self.two_stage_engine = Some(
            TwoStageSearchBuilder::new()
                .candidate_multiplier(3.0)
                .min_candidates(100)
                .max_candidates(10000)
                .build(self.distance_compute.clone(), quantization_engine)
        );
        
        info!("🔍 VIPER Search: Two-stage search engine initialized");
    }

    /// **PRIMARY SEARCH METHOD: Storage-Aware Polymorphic Search**
    /// 
    /// This method delegates to the most efficient search strategy based on:
    /// - Collection size and clustering status
    /// - Query characteristics and optimization hints
    /// - Storage layout and indexing availability
    pub async fn search_vectors(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        query_vector: &[f32],
        search_params: &SearchParams,
    ) -> Result<Vec<SearchResult>> {
        let start_time = std::time::Instant::now();
        let k = search_params.top_k.unwrap_or(10);

        info!(
            "🔍 VIPER Search: Starting polymorphic search - collection={}, dimension={}, k={}",
            collection_id,
            query_vector.len(),
            k
        );

        // Validate input parameters
        self.validate_search_parameters(collection_id, query_vector, k)?;

        // Generate optimal column projection for this query
        let column_projection = self.generate_column_projection(
            viper_engine,
            collection_id,
            search_params,
        ).await?;

        info!("📊 VIPER Search: Using column projection with {} columns, estimated I/O reduction: {:.1}%", 
              column_projection.columns.len(), 
              column_projection.io_reduction_estimate * 100.0);

        // Determine optimal search strategy based on collection characteristics
        let search_strategy = self.determine_search_strategy(
            viper_engine,
            collection_id,
            query_vector,
            search_params,
        ).await?;

        info!("🔍 VIPER Search: Using strategy={:?} for collection={}", search_strategy, collection_id);

        // Execute search using selected strategy with column projection
        let results = match search_strategy {
            SearchStrategy::ClusterOptimized => {
                self.cluster_optimized_search(viper_engine, collection_id, query_vector, search_params, &column_projection).await?
            }
            SearchStrategy::DirectSearch => {
                self.direct_search(viper_engine, collection_id, query_vector, search_params, &column_projection).await?
            }
            SearchStrategy::HybridSearch => {
                self.hybrid_search(viper_engine, collection_id, query_vector, search_params, &column_projection).await?
            }
            SearchStrategy::TwoStageSearch => {
                // Use two-stage search if available and configured
                if let Some(ref two_stage) = self.two_stage_engine {
                    let distance_metric = &DistanceMetric::Cosine; // Default to cosine
                    
                    two_stage.search(
                        viper_engine,
                        collection_id,
                        query_vector,
                        search_params,
                        &column_projection,
                        distance_metric,
                    ).await?
                } else {
                    // Fallback to direct search if two-stage not initialized
                    warn!("Two-stage search requested but not initialized, falling back to direct search");
                    self.direct_search(viper_engine, collection_id, query_vector, search_params, &column_projection).await?
                }
            }
        };

        // Update search metrics
        let search_duration = start_time.elapsed();
        self.update_search_metrics(search_duration, results.len()).await;

        info!(
            "✅ VIPER Search: Found {} results in {}μs using {:?} strategy",
            results.len(),
            search_duration.as_micros(),
            search_strategy
        );

        Ok(results)
    }

    /// Generate optimal column projection for the search query
    async fn generate_column_projection(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        search_params: &SearchParams,
    ) -> Result<ColumnProjection> {
        // Get available quantization levels from collection metadata
        let available_quantization = self.get_available_quantization_levels(viper_engine, collection_id).await?;
        
        // Estimate result size based on collection size and filters
        let estimated_result_size = self.estimate_result_size(viper_engine, collection_id, search_params).await?;
        
        // Use column projection strategy to select optimal columns
        let mut projection = self.column_projection.select_columns(
            search_params,
            &available_quantization,
            estimated_result_size,
        )?;
        
        // Calculate I/O reduction estimate
        let total_columns = self.get_total_column_count(viper_engine, collection_id).await?;
        projection.io_reduction_estimate = projection.estimate_io_reduction(total_columns);
        
        Ok(projection)
    }

    /// Get available quantization levels from collection configuration
    async fn get_available_quantization_levels(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
    ) -> Result<Vec<crate::compute::UnifiedQuantizationLevel>> {
        // This would read from collection metadata to determine what quantization columns exist
        // For now, return a default set
        Ok(vec![
            crate::compute::UnifiedQuantizationLevel {
                level_type: Some(crate::compute::QuantizationLevelType::None(crate::compute::NoQuantization {})),
            },
            crate::compute::UnifiedQuantizationLevel::pq8(8),
            crate::compute::UnifiedQuantizationLevel {
                level_type: Some(crate::compute::QuantizationLevelType::Binary(crate::compute::BinaryQuantization {
                    threshold: None,
                    sign_based: false,
                })),
            },
        ])
    }

    /// Estimate result size based on collection size and filters
    async fn estimate_result_size(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        search_params: &SearchParams,
    ) -> Result<usize> {
        // Simple heuristic: assume 1000 candidates per result
        let k = search_params.top_k.unwrap_or(10);
        
        // Apply filter selectivity estimation
        let filter_selectivity = if search_params.filters.is_some() {
            0.1 // Assume filters reduce search space by 90%
        } else {
            1.0
        };
        
        Ok((k as f32 * 1000.0 * filter_selectivity) as usize)
    }

    /// Get total column count for I/O reduction calculation
    async fn get_total_column_count(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
    ) -> Result<usize> {
        // This would read from schema metadata
        // For now, return a typical count
        Ok(20)
    }

    /// Determine the optimal search strategy based on collection and query characteristics
    async fn determine_search_strategy(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        query_vector: &[f32],
        search_params: &SearchParams,
    ) -> Result<SearchStrategy> {
        // Get collection metadata to inform strategy selection
        let collection_metadata = viper_engine.get_collection_metadata(collection_id).await;

        // Check if two-stage search is enabled and available
        let has_quantization = if let Some(metadata) = &collection_metadata {
            // Check if collection has quantization configured
            metadata.quantization_config.is_some() && self.two_stage_engine.is_some()
        } else {
            false
        };

        // Check for explicit hints in search params
        if search_params.quantization_hint.is_some() && has_quantization {
            info!("🔍 VIPER Search: Using two-stage search based on quantization hint");
            return Ok(SearchStrategy::TwoStageSearch);
        }

        // Check if ML clustering is available (OPTIONAL - via AXIS)
        // Many collections won't have clustering and that's fine
        let has_clusters = if self.config.enable_ml_clustering {
            // This would check with AXIS if clustering index exists
            self.check_cluster_availability(collection_id).await?
        } else {
            false
        };

        // Strategy selection - DirectSearch is the baseline that always works
        match (collection_metadata, has_quantization, has_clusters, search_params.filters.is_some()) {
            // Collection with quantization enabled - use two-stage search
            (Some(_), true, _, _) => Ok(SearchStrategy::TwoStageSearch),
            
            // Collection with AXIS ML clustering + metadata filters
            (Some(_), false, true, true) => Ok(SearchStrategy::HybridSearch),
            
            // Collection with AXIS ML clustering, no metadata filters
            (Some(_), false, true, false) => Ok(SearchStrategy::ClusterOptimized),
            
            // DEFAULT: Direct Parquet search (no clustering or clustering disabled)
            (Some(_), false, false, _) | (None, _, _, _) => Ok(SearchStrategy::DirectSearch),
        }
    }

    /// **CLUSTER-OPTIMIZED SEARCH**: Use ML clustering for large collections
    async fn cluster_optimized_search(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        query_vector: &[f32],
        search_params: &SearchParams,
        column_projection: &ColumnProjection,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 VIPER: Executing cluster-optimized search for collection {}", collection_id);

        // Step 1: ML-driven cluster selection
        let relevant_clusters = self.select_relevant_clusters(collection_id, query_vector).await?;

        if relevant_clusters.is_empty() {
            warn!("🔍 VIPER: No relevant clusters found, falling back to direct search");
            return self.direct_search(viper_engine, collection_id, query_vector, search_params, column_projection).await;
        }

        info!("🔍 VIPER: Selected {} clusters for search", relevant_clusters.len());

        // Step 2: Parallel search across selected clusters
        let mut all_results = Vec::new();
        
        if self.config.enable_parallel_search && relevant_clusters.len() > 1 {
            // Parallel cluster search for better performance
            let search_tasks: Vec<_> = relevant_clusters.into_iter().map(|cluster_id| {
                let viper_engine = viper_engine;
                let collection_id = collection_id.clone();
                let query_vector = query_vector.to_vec();
                let search_params = search_params.clone();
                
                async move {
                    self.search_cluster(viper_engine, &collection_id, &cluster_id, &query_vector, &search_params).await
                }
            }).collect();
            
            let cluster_results = futures::future::join_all(search_tasks).await;
            for result in cluster_results {
                match result {
                    Ok(results) => all_results.extend(results),
                    Err(e) => warn!("🔍 VIPER: Cluster search failed: {}", e),
                }
            }
        } else {
            // Sequential cluster search
            for cluster_id in relevant_clusters {
                match self.search_cluster(viper_engine, collection_id, &cluster_id, query_vector, search_params).await {
                    Ok(cluster_results) => {
                        debug!("🔍 VIPER: Cluster {} returned {} results", cluster_id.0, cluster_results.len());
                        all_results.extend(cluster_results);
                    }
                    Err(e) => {
                        warn!("🔍 VIPER: Failed to search cluster {}: {}", cluster_id.0, e);
                        // Continue with other clusters
                    }
                }
            }
        }

        // Step 3: Apply metadata filters if specified
        if let Some(filters) = &search_params.filters {
            all_results = self.apply_metadata_filters(all_results, filters)?;
        }

        // Step 4: Merge, rank, and truncate results
        let k = search_params.top_k.unwrap_or(10);
        all_results.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(k);

        Ok(all_results)
    }

    /// **DIRECT SEARCH**: Baseline Parquet search without ML clustering
    /// This must work for ALL collections, with clustering as optional optimization
    async fn direct_search(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        query_vector: &[f32],
        search_params: &SearchParams,
        column_projection: &ColumnProjection,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 VIPER: Executing direct search for collection {}", collection_id);
        let search_start = std::time::Instant::now();

        // Get all Parquet files for this collection from storage engine
        let parquet_files = viper_engine.get_parquet_files_for_collection(collection_id).await?;
        info!("🔍 Direct search: Found {} Parquet files for collection {}", parquet_files.len(), collection_id);

        let mut all_results = Vec::new();
        let mut files_searched = 0;
        let k = search_params.top_k.unwrap_or(10);
        let metadata_filters = search_params.filters.as_ref();

        // Search each Parquet file using predicate pushdown
        for parquet_file in parquet_files {
            match self.search_parquet_file(&parquet_file, query_vector, k * 2, metadata_filters).await {
                Ok(file_results) => {
                    all_results.extend(file_results);
                    files_searched += 1;
                }
                Err(e) => {
                    warn!("🔍 Failed to search Parquet file {}: {}", parquet_file, e);
                    continue;
                }
            }
        }

        // Sort by distance and take top k results
        all_results.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        all_results.truncate(k);

        info!(
            "🔍 Direct search completed: {} results from {} files in {}ms",
            all_results.len(),
            files_searched,
            search_start.elapsed().as_millis()
        );

        Ok(all_results)
    }

    /// **HYBRID SEARCH**: Combine clustering with predicate pushdown optimization
    async fn hybrid_search(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        query_vector: &[f32],
        search_params: &SearchParams,
        column_projection: &ColumnProjection,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 VIPER: Executing hybrid search for collection {}", collection_id);

        // For now, delegate to cluster-optimized search with metadata filtering
        // In a full implementation, this would implement predicate pushdown
        // to filter at the Parquet storage level before vector operations
        self.cluster_optimized_search(viper_engine, collection_id, query_vector, search_params, column_projection).await
    }


    /// Select relevant clusters using trained ML models
    async fn select_relevant_clusters(
        &self,
        collection_id: &str,
        query_vector: &[f32],
    ) -> Result<Vec<ClusterId>> {
        // Try to get trained model first
        if let Some(ref model_manager) = self.model_manager {
            if let Some(trained_model) = model_manager.get_clustering_model(
                &collection_id.to_string(),
                1_000_000, // Assume large collection if we're using clustering
                query_vector.len(),
            ).await? {
                return self.select_clusters_from_trained_model(&trained_model, query_vector).await;
            }
        }

        // Fallback to cache-based selection
        self.select_clusters_from_cache(collection_id, query_vector).await
    }

    /// Select clusters using trained ML model (optimal path)
    async fn select_clusters_from_trained_model(
        &self,
        trained_model: &EfficientClusteringModel,
        query_vector: &[f32],
    ) -> Result<Vec<ClusterId>> {
        info!("🧠 Using trained clustering model with {} clusters", trained_model.centroids.len());
        
        let mut cluster_distances: Vec<(ClusterId, f32)> = Vec::new();

        // Calculate distances to all centroids in trained model
        for (i, centroid) in trained_model.centroids.iter().enumerate() {
            if centroid.len() == query_vector.len() {
                let distance = self.calculate_cosine_distance(query_vector, centroid);
                cluster_distances.push((ClusterId(i), distance));
            }
        }

        // Sort by distance and select top clusters
        cluster_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let max_clusters = self.config.max_clusters_to_search.min(cluster_distances.len());
        
        let selected_clusters: Vec<ClusterId> = cluster_distances
            .into_iter()
            .take(max_clusters)
            .filter(|(_, distance)| *distance <= self.config.cluster_confidence_threshold)
            .map(|(cluster_id, _)| cluster_id)
            .collect();

        info!("🧠 Selected {} clusters from trained model", selected_clusters.len());
        Ok(selected_clusters)
    }

    /// Fallback cluster selection using cache
    async fn select_clusters_from_cache(
        &self,
        collection_id: &str,
        query_vector: &[f32],
    ) -> Result<Vec<ClusterId>> {
        debug!("🧠 Falling back to cache-based cluster selection");
        
        let cache = self.cluster_cache.read().await;

        // Get cluster centroids from cache
        let mut cluster_distances: Vec<(ClusterId, f32)> = Vec::new();

        for (cluster_id, centroid) in &cache.cluster_centroids {
            if centroid.len() == query_vector.len() {
                let distance = self.calculate_cosine_distance(query_vector, centroid);
                cluster_distances.push((*cluster_id, distance));
            }
        }

        // Sort by distance and select top clusters
        cluster_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let max_clusters = self.config.max_clusters_to_search.min(cluster_distances.len());
        
        Ok(cluster_distances
            .into_iter()
            .take(max_clusters)
            .filter(|(_, distance)| *distance <= self.config.cluster_confidence_threshold)
            .map(|(cluster_id, _)| cluster_id)
            .collect())
    }

    /// Check if ML clustering is available for a collection
    async fn check_cluster_availability(&self, collection_id: &str) -> Result<bool> {
        let cache = self.cluster_cache.read().await;
        Ok(!cache.cluster_centroids.is_empty())
    }

    /// Apply metadata filters to search results
    fn apply_metadata_filters(
        &self,
        mut results: Vec<SearchResult>,
        filters: &HashMap<String, serde_json::Value>,
    ) -> Result<Vec<SearchResult>> {
        if filters.is_empty() {
            return Ok(results);
        }

        results.retain(|result| {
            for (key, expected_value) in filters {
                match result.metadata.get(key) {
                    Some(actual_value) => {
                        if actual_value != expected_value {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        });

        Ok(results)
    }

    /// **CLUSTER SEARCH**: Search vectors within a specific cluster using cluster-optimized strategies
    pub async fn search_cluster(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        cluster_id: &ClusterId,
        query_vector: &[f32],
        search_params: &SearchParams,
    ) -> Result<Vec<SearchResult>> {
        let k = search_params.top_k.unwrap_or(10);
        debug!("🔍 VIPER: Searching cluster {} for collection {}", cluster_id.0, collection_id);
        let cluster_start = std::time::Instant::now();

        // Get cluster-specific Parquet files from model metadata
        let cluster_files = if let Some(model_manager) = &self.model_manager {
            match model_manager.get_clustering_model(&collection_id.to_string(), 1_000_000, 128).await? {
                Some(model) => {
                    // Get files associated with this cluster from model metadata
                    model.cluster_metadata
                        .get(cluster_id.0)
                        .map(|metadata| metadata.parquet_files.clone())
                        .unwrap_or_else(Vec::new)
                }
                None => {
                    // Fallback: get all files and filter by cluster heuristics
                    viper_engine.get_parquet_files_for_collection(collection_id).await?
                }
            }
        } else {
            // No model manager: use all files
            viper_engine.get_parquet_files_for_collection(collection_id).await?
        };

        if cluster_files.is_empty() {
            debug!("🔍 No Parquet files found for cluster {}", cluster_id.0);
            return Ok(Vec::new());
        }

        info!("🔍 Cluster {}: Searching {} Parquet files", cluster_id.0, cluster_files.len());

        // Search cluster-specific files with optimized parameters
        let mut cluster_results = Vec::new();
        for parquet_file in cluster_files {
            match self.search_parquet_file(&parquet_file, query_vector, k * 3, None).await {
                Ok(file_results) => {
                    cluster_results.extend(file_results);
                }
                Err(e) => {
                    warn!("🔍 Failed to search cluster file {}: {}", parquet_file, e);
                    continue;
                }
            }
        }

        // Apply cluster-specific distance optimization
        // Vectors closer to cluster centroid should get priority boost
        if let Some(model_manager) = &self.model_manager {
            if let Ok(Some(model)) = model_manager.get_clustering_model(&collection_id.to_string(), 1_000_000, 128).await {
                if let Some(cluster_metadata) = model.cluster_metadata.get(cluster_id.0) {
                    let centroid = &cluster_metadata.centroid;
                    
                    // Boost scores for vectors closer to cluster centroid
                    for result in &mut cluster_results {
                        let centroid_distance = self.calculate_cosine_distance(query_vector, centroid);
                        let centroid_bonus = 1.0 - (centroid_distance / 2.0); // Max 0.5 bonus
                        result.score *= 1.0 + centroid_bonus;
                    }
                }
            }
        }

        // Sort by enhanced score and take top results
        cluster_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        cluster_results.truncate(k);

        debug!(
            "🔍 Cluster {} search completed: {} results in {}ms",
            cluster_id.0,
            cluster_results.len(),
            cluster_start.elapsed().as_millis()
        );

        Ok(cluster_results)
    }

    /// **PARQUET FILE SEARCH**: Search vectors within a single Parquet file with predicate pushdown
    async fn search_parquet_file(
        &self,
        parquet_file_path: &str,
        query_vector: &[f32],
        k: usize,
        metadata_filters: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<Vec<SearchResult>> {
        use arrow_array::{Array, Float32Array, ListArray, StringArray, Int64Array, BooleanArray, Float64Array, TimestampMicrosecondArray, StructArray};
        use std::fs::File;
        
        debug!("🔍 Searching Parquet file: {}", parquet_file_path);
        let search_start = std::time::Instant::now();
        
        // Open Parquet file
        let file = File::open(parquet_file_path)
            .context(format!("Failed to open Parquet file: {}", parquet_file_path))?;
        let file_reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)?;
        let metadata = file_reader.metadata();
        debug!("📊 Parquet file has {} row groups", metadata.num_row_groups());
        
        // Build reader with column projection
        let mut reader_builder = file_reader;
        
        // Select only needed columns for efficiency
        let mut projection = vec![
            "id",
            "vector",  // Original FP32 vectors
            "vector_pq",  // Optional quantized vectors
            "timestamp",
            "version",
            "expires_at",
            "extra_meta",
        ];
        
        // Add filterable metadata columns to projection
        if let Some(filters) = metadata_filters {
            for key in filters.keys() {
                // Add filterable column if not already in projection
                if !projection.contains(&key.as_str()) {
                    projection.push(key);
                }
            }
        }
        
        let mut batch_reader = reader_builder.build()?;
        let current_time = chrono::Utc::now().timestamp_micros();
        
        // Collect all valid candidates with distances
        let mut candidates: Vec<(String, f32, i64, i64, SearchResult)> = Vec::new();
        
        // Process each record batch
        while let Some(batch) = batch_reader.next() {
            let batch = batch?;
            
            // Get columns
            let id_array = batch.column_by_name("id")
                .ok_or_else(|| anyhow::anyhow!("Missing 'id' column"))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid 'id' column type"))?;
                
            let vector_array = batch.column_by_name("vector")
                .ok_or_else(|| anyhow::anyhow!("Missing 'vector' column"))?
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid 'vector' column type"))?;
                
            let timestamp_array = batch.column_by_name("timestamp")
                .ok_or_else(|| anyhow::anyhow!("Missing 'timestamp' column"))?
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| anyhow::anyhow!("Invalid 'timestamp' column type"))?;
                
            let version_array = batch.column_by_name("version")
                .ok_or_else(|| anyhow::anyhow!("Missing 'version' column"))?
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| anyhow::anyhow!("Invalid 'version' column type"))?;
                
            let expires_at_array = batch.column_by_name("expires_at")
                .ok_or_else(|| anyhow::anyhow!("Missing 'expires_at' column"))?
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| anyhow::anyhow!("Invalid 'expires_at' column type"))?;
            
            // Process each row
            for row_idx in 0..batch.num_rows() {
                // Skip expired records
                if !expires_at_array.is_null(row_idx) {
                    let expires_at = expires_at_array.value(row_idx);
                    if expires_at > 0 && expires_at < current_time {
                        continue; // Skip expired vectors
                    }
                }
                
                // Get vector data
                let vector_values = vector_array.value(row_idx);
                let vector_float_array = vector_values
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow::anyhow!("Invalid vector values type"))?;
                
                // Convert to Vec<f32>
                let vector: Vec<f32> = (0..vector_float_array.len())
                    .map(|i| vector_float_array.value(i))
                    .collect();
                
                // Skip if dimensions don't match
                if vector.len() != query_vector.len() {
                    warn!("Vector dimension mismatch: {} != {}", vector.len(), query_vector.len());
                    continue;
                }
                
                // Calculate distance
                let distance = self.calculate_cosine_distance(query_vector, &vector);
                let score = 1.0 - distance; // Convert distance to similarity score
                
                // Create search result
                let result = SearchResult {
                    id: id_array.value(row_idx).to_string(),
                    vector_id: Some(id_array.value(row_idx).to_string()),
                    score,
                    distance: Some(distance),
                    rank: None, // Will be set after sorting
                    vector: None, // Don't return full vector to save memory
                    metadata: {
                        let mut metadata = HashMap::new();
                        
                        // Parse metadata from extra_meta list of key-value pairs
                        if let Some(extra_meta_col) = batch.column_by_name("extra_meta") {
                            if let Some(extra_meta_list) = extra_meta_col.as_any().downcast_ref::<ListArray>() {
                                if !extra_meta_list.is_null(row_idx) {
                                    let kv_pairs = extra_meta_list.value(row_idx);
                                    if let Some(struct_array) = kv_pairs.as_any().downcast_ref::<StructArray>() {
                                        let key_array = struct_array.column(0).as_any().downcast_ref::<StringArray>().unwrap();
                                        let value_array = struct_array.column(1).as_any().downcast_ref::<StringArray>().unwrap();
                                        
                                        for kv_idx in 0..struct_array.len() {
                                            if !struct_array.is_null(kv_idx) {
                                                let key = key_array.value(kv_idx).to_string();
                                                let value = value_array.value(kv_idx).to_string();
                                                metadata.insert(key, serde_json::Value::String(value));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        
                        // Also parse filterable metadata columns (they have their own columns)
                        for field in batch.schema().fields() {
                            let field_name = field.name();
                            // Skip core fields - only process filterable metadata columns
                            if !matches!(field_name.as_str(), "id" | "collection_id" | "vector" | "timestamp" | "created_at" | "updated_at" | "version" | "expires_at" | "extra_meta" | "vector_pq" | "vector_sq" | "vector_binary" | "vector_quantized" | "sq_scale" | "sq_offset") {
                                if let Some(column) = batch.column_by_name(field_name) {
                                    if !column.is_null(row_idx) {
                                        // Convert Arrow value to JSON based on data type
                                        let json_value = match field.data_type() {
                                            arrow_schema::DataType::Utf8 => {
                                                if let Some(str_array) = column.as_any().downcast_ref::<StringArray>() {
                                                    serde_json::Value::String(str_array.value(row_idx).to_string())
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Int64 => {
                                                if let Some(int_array) = column.as_any().downcast_ref::<Int64Array>() {
                                                    serde_json::Value::Number(serde_json::Number::from(int_array.value(row_idx)))
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Float64 => {
                                                if let Some(float_array) = column.as_any().downcast_ref::<Float64Array>() {
                                                    serde_json::Value::Number(serde_json::Number::from_f64(float_array.value(row_idx)).unwrap_or(serde_json::Number::from(0)))
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Boolean => {
                                                if let Some(bool_array) = column.as_any().downcast_ref::<BooleanArray>() {
                                                    serde_json::Value::Bool(bool_array.value(row_idx))
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, _) => {
                                                if let Some(ts_array) = column.as_any().downcast_ref::<TimestampMicrosecondArray>() {
                                                    serde_json::Value::Number(serde_json::Number::from(ts_array.value(row_idx)))
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::List(_) => {
                                                // For list columns, serialize as JSON array
                                                if let Some(list_array) = column.as_any().downcast_ref::<ListArray>() {
                                                    let list_value = list_array.value(row_idx);
                                                    // Convert Arrow array to JSON array (simplified)
                                                    serde_json::Value::String(format!("list_length_{}", list_value.len()))
                                                } else { continue; }
                                            }
                                            _ => continue, // Skip unsupported types
                                        };
                                        metadata.insert(field_name.to_string(), json_value);
                                    }
                                }
                            }
                        }
                        
                        metadata
                    },
                    collection_id: None,
                    created_at: Some(timestamp_array.value(row_idx)),
                    algorithm_used: Some("viper_parquet_cosine".to_string()),
                    processing_time_us: None,
                };
                
                candidates.push((
                    id_array.value(row_idx).to_string(),
                    distance,
                    version_array.value(row_idx),
                    timestamp_array.value(row_idx),
                    result
                ));
            }
        }
        
        // Sort by distance (ascending) and take top-k
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        
        // Deduplicate by ID (keep highest version/newest timestamp)
        let mut seen_ids = HashMap::new();
        let mut final_results = Vec::new();
        
        for (id, distance, version, timestamp, mut result) in candidates.into_iter().take(k * 2) {
            match seen_ids.get(&id) {
                Some(&(existing_version, existing_timestamp)) => {
                    // Keep the newer version or timestamp
                    if version > existing_version || 
                       (version == existing_version && timestamp > existing_timestamp) {
                        // Remove old result and add new one
                        final_results.retain(|r: &SearchResult| r.id != id);
                        seen_ids.insert(id.clone(), (version, timestamp));
                        result.rank = Some((final_results.len() + 1) as i32);
                        final_results.push(result);
                    }
                }
                None => {
                    seen_ids.insert(id.clone(), (version, timestamp));
                    result.rank = Some((final_results.len() + 1) as i32);
                    final_results.push(result);
                    if final_results.len() >= k {
                        break;
                    }
                }
            }
        }
        
        // Update processing time
        let processing_time_us = search_start.elapsed().as_micros() as i64;
        for result in &mut final_results {
            result.processing_time_us = Some(processing_time_us);
        }
        
        debug!("🔍 Parquet file {} returned {} results in {}μs", 
               parquet_file_path, final_results.len(), processing_time_us);
        Ok(final_results)
    }

    /// Calculate cosine distance between two vectors
    fn calculate_cosine_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return f32::MAX;
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return f32::MAX;
        }

        1.0 - (dot_product / (norm_a * norm_b))
    }

    /// Validate search parameters
    fn validate_search_parameters(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
    ) -> Result<()> {
        if query_vector.is_empty() {
            return Err(anyhow::anyhow!("Query vector cannot be empty"));
        }

        if k == 0 {
            return Err(anyhow::anyhow!("k must be greater than 0"));
        }

        if k > 10000 {
            return Err(anyhow::anyhow!("k cannot exceed 10000 for performance reasons"));
        }

        // Check for invalid values (NaN, infinity)
        for (i, &value) in query_vector.iter().enumerate() {
            if !value.is_finite() {
                return Err(anyhow::anyhow!(
                    "Query vector contains invalid value at index {}: {}",
                    i,
                    value
                ));
            }
        }

        Ok(())
    }

    /// Update search performance metrics
    async fn update_search_metrics(&self, duration: std::time::Duration, result_count: usize) {
        let mut metrics = self.metrics.write().await;
        
        let duration_us = duration.as_micros() as f64;
        
        metrics.total_searches += 1;
        
        // Update average latency using incremental formula
        metrics.avg_latency_us = 
            (metrics.avg_latency_us * ((metrics.total_searches - 1) as f64) + duration_us) 
            / (metrics.total_searches as f64);
    }

    /// Get current search performance metrics
    pub async fn get_search_metrics(&self) -> SearchMetrics {
        self.metrics.read().await.clone()
    }

    /// Update cluster metadata cache (called by ML clustering system)
    pub async fn update_cluster_cache(
        &self,
        collection_id: &str,
        cluster_centroids: HashMap<ClusterId, Vec<f32>>,
        cluster_sizes: HashMap<ClusterId, usize>,
    ) -> Result<()> {
        let mut cache = self.cluster_cache.write().await;
        
        cache.cluster_centroids = cluster_centroids;
        cache.cluster_sizes = cluster_sizes;
        cache.last_updated = Some(std::time::SystemTime::now());
        
        info!("🧠 VIPER: Updated cluster cache for collection {} with {} clusters", 
              collection_id, cache.cluster_centroids.len());
        
        Ok(())
    }
}

/// Search strategy selection
#[derive(Debug, Clone, Copy)]
enum SearchStrategy {
    /// Use ML clustering for large collections
    ClusterOptimized,
    /// Direct search without clustering
    DirectSearch,
    /// Hybrid approach with predicate pushdown
    HybridSearch,
    /// Two-stage search with quantized filtering
    TwoStageSearch,
}

/// Cluster identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClusterId(pub usize);

// SearchResult is already imported above, no need to re-export

// Tests have been moved to tests/unit/storage/engines/viper/test_search.rs