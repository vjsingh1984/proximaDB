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
use futures::future;
use chrono;

use crate::core::{CollectionId, SearchResult, VectorRecord};
use crate::storage::engines::viper::ViperEngine;
use crate::storage::engines::viper::types::*;
use crate::storage::engines::viper::clustering_models::{ClusteringModelManager, EfficientClusteringModel};

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
    pub default_quantization: QuantizationLevel,
    
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
            default_quantization: QuantizationLevel::FP32,
            search_timeout_ms: 5000,
        }
    }
}

/// Vector quantization levels supported by VIPER
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizationLevel {
    /// Full 32-bit floating point precision (100% accuracy)
    FP32,
    /// 8-bit product quantization (faster, ~95% accuracy)
    PQ8,
    /// 4-bit product quantization (4x faster, ~90% accuracy)
    PQ4,
    /// Binary quantization (16x faster, ~80% accuracy)
    Binary,
    /// Scalar quantization to 8-bit integers
    INT8,
}

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

/// Search performance metrics
#[derive(Debug, Default, Clone)]
pub struct SearchMetrics {
    /// Total number of searches performed
    pub total_searches: u64,
    
    /// Average search latency in microseconds
    pub avg_latency_us: f64,
    
    /// Total clusters searched
    pub total_clusters_searched: u64,
    
    /// ML clustering hit rate
    pub ml_clustering_hit_rate: f32,
    
    /// Predicate pushdown effectiveness
    pub predicate_pushdown_reduction: f32,
}

/// Search hints for optimization
#[derive(Debug, Clone)]
pub struct SearchHints {
    /// Preferred quantization level
    pub quantization_level: Option<QuantizationLevel>,
    
    /// Enable cluster optimization
    pub enable_clustering: bool,
    
    /// Enable metadata filtering optimization
    pub enable_metadata_filtering: bool,
    
    /// Custom optimization parameters
    pub custom_params: HashMap<String, serde_json::Value>,
}

impl Default for SearchHints {
    fn default() -> Self {
        Self {
            quantization_level: None,
            enable_clustering: true,
            enable_metadata_filtering: true,
            custom_params: HashMap::new(),
        }
    }
}

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
        }
    }

    /// Set the clustering model manager for trained model access
    pub fn set_model_manager(&mut self, model_manager: Arc<ClusteringModelManager>) {
        self.model_manager = Some(model_manager);
        info!("🧠 VIPER Search: Model manager set for trained clustering models");
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
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        metadata_filters: Option<&HashMap<String, serde_json::Value>>,
        search_hints: Option<&SearchHints>,
    ) -> Result<Vec<SearchResult>> {
        let start_time = std::time::Instant::now();

        info!(
            "🔍 VIPER Search: Starting polymorphic search - collection={}, dimension={}, k={}",
            collection_id,
            query_vector.len(),
            k
        );

        // Validate input parameters
        self.validate_search_parameters(collection_id, query_vector, k)?;

        // Determine optimal search strategy based on collection characteristics
        let search_strategy = self.determine_search_strategy(
            viper_engine,
            collection_id,
            query_vector,
            k,
            metadata_filters,
            search_hints,
        ).await?;

        info!("🔍 VIPER Search: Using strategy={:?} for collection={}", search_strategy, collection_id);

        // Execute search using selected strategy
        let results = match search_strategy {
            SearchStrategy::ClusterOptimized => {
                self.cluster_optimized_search(viper_engine, collection_id, query_vector, k, metadata_filters, search_hints).await?
            }
            SearchStrategy::DirectSearch => {
                self.direct_search(viper_engine, collection_id, query_vector, k, metadata_filters).await?
            }
            SearchStrategy::HybridSearch => {
                self.hybrid_search(viper_engine, collection_id, query_vector, k, metadata_filters, search_hints).await?
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

    /// Determine the optimal search strategy based on collection and query characteristics
    async fn determine_search_strategy(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        metadata_filters: Option<&HashMap<String, serde_json::Value>>,
        search_hints: Option<&SearchHints>,
    ) -> Result<SearchStrategy> {
        // Get collection metadata to inform strategy selection
        let collection_metadata = viper_engine.get_collection_metadata(collection_id).await;

        // Check if ML clustering is available and beneficial
        let has_clusters = if self.config.enable_ml_clustering {
            self.check_cluster_availability(collection_id).await?
        } else {
            false
        };

        // Strategy selection logic based on collection characteristics
        match (collection_metadata, has_clusters, metadata_filters.is_some()) {
            // Large collection with clusters and metadata filters
            (Some(_), true, true) => Ok(SearchStrategy::HybridSearch),
            
            // Large collection with clusters, no metadata filters
            (Some(_), true, false) => Ok(SearchStrategy::ClusterOptimized),
            
            // Small collection or no clusters available
            (Some(_), false, _) | (None, _, _) => Ok(SearchStrategy::DirectSearch),
        }
    }

    /// **CLUSTER-OPTIMIZED SEARCH**: Use ML clustering for large collections
    async fn cluster_optimized_search(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        metadata_filters: Option<&HashMap<String, serde_json::Value>>,
        search_hints: Option<&SearchHints>,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 VIPER: Executing cluster-optimized search for collection {}", collection_id);

        // Step 1: ML-driven cluster selection
        let relevant_clusters = self.select_relevant_clusters(collection_id, query_vector).await?;

        if relevant_clusters.is_empty() {
            warn!("🔍 VIPER: No relevant clusters found, falling back to direct search");
            return self.direct_search(viper_engine, collection_id, query_vector, k, metadata_filters).await;
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
                
                async move {
                    self.search_cluster(viper_engine, &collection_id, &cluster_id, &query_vector, k * 2).await
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
                match self.search_cluster(viper_engine, collection_id, &cluster_id, query_vector, k * 2).await {
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
        if let Some(filters) = metadata_filters {
            all_results = self.apply_metadata_filters(all_results, filters)?;
        }

        // Step 4: Merge, rank, and truncate results
        all_results.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(k);

        Ok(all_results)
    }

    /// **DIRECT SEARCH**: Parquet-based vector search across all storage tiers
    async fn direct_search(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        metadata_filters: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 VIPER: Executing direct search for collection {}", collection_id);
        let search_start = std::time::Instant::now();

        // Get all Parquet files for this collection from storage engine
        let parquet_files = viper_engine.get_parquet_files_for_collection(collection_id).await?;
        info!("🔍 Direct search: Found {} Parquet files for collection {}", parquet_files.len(), collection_id);

        let mut all_results = Vec::new();
        let mut files_searched = 0;

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
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        metadata_filters: Option<&HashMap<String, serde_json::Value>>,
        search_hints: Option<&SearchHints>,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 VIPER: Executing hybrid search for collection {}", collection_id);

        // For now, delegate to cluster-optimized search with metadata filtering
        // In a full implementation, this would implement predicate pushdown
        // to filter at the Parquet storage level before vector operations
        self.cluster_optimized_search(viper_engine, collection_id, query_vector, k, metadata_filters, search_hints).await
    }


    /// Select relevant clusters using trained ML models
    async fn select_relevant_clusters(
        &self,
        collection_id: &CollectionId,
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
        collection_id: &CollectionId,
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
    async fn check_cluster_availability(&self, collection_id: &CollectionId) -> Result<bool> {
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
        collection_id: &CollectionId,
        cluster_id: &ClusterId,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<SearchResult>> {
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
        debug!("🔍 Searching Parquet file: {}", parquet_file_path);
        
        // TODO: Implement actual Parquet file reading and vector distance calculation
        // This would involve:
        // 1. Opening the Parquet file with Arrow
        // 2. Reading vector columns (List<Float32>)
        // 3. Applying metadata filters as Parquet predicates
        // 4. Computing distances using SIMD operations
        // 5. Returning top-k results with scores
        
        // For now, return mock results to demonstrate the control flow
        let mock_results = vec![
            SearchResult {
                id: format!("vec_{}_{}", parquet_file_path.replace("/", "_"), 1),
                vector_id: None,
                score: 0.95,
                distance: Some(0.05),
                rank: Some(1),
                vector: None,
                metadata: std::collections::HashMap::new(),
                collection_id: None,
                created_at: Some(chrono::Utc::now().timestamp_millis()),
                algorithm_used: Some("viper_parquet_search".to_string()),
                processing_time_us: Some(1000),
            },
            SearchResult {
                id: format!("vec_{}_{}", parquet_file_path.replace("/", "_"), 2),
                vector_id: None,
                score: 0.90,
                distance: Some(0.10),
                rank: Some(2),
                vector: None,
                metadata: std::collections::HashMap::new(),
                collection_id: None,
                created_at: Some(chrono::Utc::now().timestamp_millis()),
                algorithm_used: Some("viper_parquet_search".to_string()),
                processing_time_us: Some(1500),
            },
        ];
        
        debug!("🔍 Parquet file {} returned {} mock results", parquet_file_path, mock_results.len());
        Ok(mock_results)
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
        collection_id: &CollectionId,
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
        collection_id: &CollectionId,
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
}

/// Cluster identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClusterId(pub usize);

// SearchResult is already imported above, no need to re-export

// Tests have been moved to tests/unit/storage/engines/viper/test_search.rs