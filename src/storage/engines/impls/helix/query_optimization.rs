//! Query optimization features for HELIX engine
//!
//! This module provides predictive prefetching and result caching to improve
//! query performance through intelligent resource management.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::core::search::results::OptimizedSearchRecord;
use crate::core::metadata_types::TypedMetadata;

/// Query pattern for tracking and prediction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPattern {
    /// Query vector hash
    pub query_hash: u64,
    /// Hilbert key of query
    pub hilbert_key: Option<u64>,
    /// Files accessed during query
    pub accessed_files: Vec<String>,
    /// Query timestamp (epoch milliseconds)
    pub timestamp_ms: u64,
    /// Query latency
    pub latency_ms: u64,
    /// Result count
    pub result_count: usize,
}

/// Predictive prefetcher using query history
pub struct PredictivePrefetcher {
    /// Query history buffer
    query_history: Arc<RwLock<VecDeque<QueryPattern>>>,
    /// Prefetch queue (file paths to prefetch)
    prefetch_queue: Arc<RwLock<Vec<String>>>,
    /// Access frequency map
    access_frequency: Arc<RwLock<HashMap<String, f32>>>,
    /// Maximum history size
    max_history_size: usize,
    /// Prediction confidence threshold
    confidence_threshold: f32,
}

impl PredictivePrefetcher {
    /// Create a new predictive prefetcher
    pub fn new(max_history_size: usize, confidence_threshold: f32) -> Self {
        Self {
            query_history: Arc::new(RwLock::new(VecDeque::with_capacity(max_history_size))),
            prefetch_queue: Arc::new(RwLock::new(Vec::new())),
            access_frequency: Arc::new(RwLock::new(HashMap::new())),
            max_history_size,
            confidence_threshold,
        }
    }

    /// Record a query pattern for learning
    pub async fn record_query(&self, pattern: QueryPattern) {
        let mut history = self.query_history.write().await;
        
        // Update access frequency for files
        let mut frequency = self.access_frequency.write().await;
        for file in &pattern.accessed_files {
            *frequency.entry(file.clone()).or_insert(0.0) += 1.0;
        }
        
        // Add to history (maintain max size)
        if history.len() >= self.max_history_size {
            history.pop_front();
        }
        history.push_back(pattern);
    }

    /// Predict which files to prefetch based on query
    pub async fn predict_prefetch(&self, query_hilbert: Option<u64>) -> Vec<String> {
        let history = self.query_history.read().await;
        let frequency = self.access_frequency.read().await;
        
        if history.is_empty() {
            return Vec::new();
        }
        
        // Simple prediction: find similar queries and their accessed files
        let mut candidate_files = HashMap::new();
        
        if let Some(query_key) = query_hilbert {
            // Find queries with similar Hilbert keys
            for pattern in history.iter() {
                if let Some(pattern_key) = pattern.hilbert_key {
                    let distance = (query_key as i64 - pattern_key as i64).abs();
                    
                    // Consider queries within Hilbert distance threshold
                    if distance < 1000 {
                        for file in &pattern.accessed_files {
                            *candidate_files.entry(file.clone()).or_insert(0.0) += 
                                1.0 / (1.0 + distance as f32);
                        }
                    }
                }
            }
        }
        
        // Rank candidates by score
        let mut ranked_files: Vec<_> = candidate_files
            .into_iter()
            .map(|(file, score)| {
                // Combine similarity score with access frequency
                let freq = frequency.get(&file).copied().unwrap_or(0.0);
                let combined_score = score * 0.7 + (freq / history.len() as f32) * 0.3;
                (file, combined_score)
            })
            .filter(|(_, score)| *score >= self.confidence_threshold)
            .collect();
        
        ranked_files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        // Return top files to prefetch
        ranked_files
            .into_iter()
            .take(3) // Prefetch top 3 files
            .map(|(file, _)| file)
            .collect()
    }

    /// Execute prefetch for given files
    pub async fn execute_prefetch(&self, files: Vec<String>) {
        if files.is_empty() {
            return;
        }
        
        debug!("Prefetching {} files based on prediction", files.len());
        *self.prefetch_queue.write().await = files;
        
        // Note: Actual file reading would be done by the cache system
        // This just marks files for prefetching
    }

    /// Get prefetch queue
    pub async fn get_prefetch_queue(&self) -> Vec<String> {
        self.prefetch_queue.read().await.clone()
    }

    /// Clear prefetch queue
    pub async fn clear_prefetch_queue(&self) {
        self.prefetch_queue.write().await.clear();
    }
}

/// Smart result cache with invalidation
pub struct SmartResultCache {
    /// LRU cache for query results
    cache: Arc<RwLock<crate::utils::cache::LruCache<u64, CachedResult>>>,
    /// Dependency tracking for invalidation
    invalidation_tracker: Arc<RwLock<HashMap<String, Vec<u64>>>>, // file -> query hashes
    /// TTL manager for time-based eviction
    default_ttl: Duration,
}

/// Cached query result
#[derive(Debug, Clone)]
struct CachedResult {
    /// Search results
    results: Vec<OptimizedSearchRecord>,
    /// Cache timestamp
    cached_at: Instant,
    /// Files accessed for this query
    accessed_files: Vec<String>,
}

impl SmartResultCache {
    /// Create a new result cache
    pub fn new(capacity: usize, default_ttl_secs: u64) -> Self {
        Self {
            cache: Arc::new(RwLock::new(crate::utils::cache::LruCache::new(
                if capacity == 0 { 100 } else { capacity }
            ))),
            invalidation_tracker: Arc::new(RwLock::new(HashMap::new())),
            default_ttl: Duration::from_secs(default_ttl_secs),
        }
    }

    /// Get cached result if available and valid
    pub async fn get(&self, query_hash: u64) -> Option<Vec<OptimizedSearchRecord>> {
        let mut cache = self.cache.write().await;
        
        if let Some(cached) = cache.get(&query_hash) {
            // Check TTL
            if cached.cached_at.elapsed() < self.default_ttl {
                debug!("Cache hit for query hash {}", query_hash);
                return Some(cached.results.clone());
            } else {
                // Expired, remove from cache
                cache.pop(&query_hash);
                self.remove_from_tracker(query_hash).await;
            }
        }
        
        None
    }

    /// Cache a query result
    pub async fn put(
        &self,
        query_hash: u64,
        results: Vec<OptimizedSearchRecord>,
        accessed_files: Vec<String>,
    ) {
        let cached_result = CachedResult {
            results,
            cached_at: Instant::now(),
            accessed_files: accessed_files.clone(),
        };
        
        // Add to cache
        self.cache.write().await.put(query_hash, cached_result);
        
        // Update invalidation tracker
        let mut tracker = self.invalidation_tracker.write().await;
        for file in accessed_files {
            tracker.entry(file).or_insert_with(Vec::new).push(query_hash);
        }
        
        debug!("Cached result for query hash {}", query_hash);
    }

    /// Invalidate cache entries that depend on a file
    pub async fn invalidate_file(&self, file_path: &str) {
        let mut tracker = self.invalidation_tracker.write().await;
        
        if let Some(query_hashes) = tracker.remove(file_path) {
            let mut cache = self.cache.write().await;
            let num_entries = query_hashes.len();
            for hash in query_hashes {
                cache.pop(&hash);
            }
            
            info!("Invalidated {} cache entries for file {}", 
                  num_entries, file_path);
        }
    }

    /// Invalidate all cache entries
    pub async fn clear(&self) {
        self.cache.write().await.clear();
        self.invalidation_tracker.write().await.clear();
        info!("Cleared all cache entries");
    }

    /// Get cache statistics
    pub async fn get_stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        let tracker = self.invalidation_tracker.read().await;
        
        CacheStats {
            entries: cache.len(),
            tracked_files: tracker.len(),
            total_dependencies: tracker.values().map(|v| v.len()).sum(),
        }
    }

    // Private helper to remove from tracker
    async fn remove_from_tracker(&self, query_hash: u64) {
        let mut tracker = self.invalidation_tracker.write().await;
        tracker.retain(|_, hashes| {
            hashes.retain(|&h| h != query_hash);
            !hashes.is_empty()
        });
    }
}

/// Cache statistics
#[derive(Debug)]
pub struct CacheStats {
    pub entries: usize,
    pub tracked_files: usize,
    pub total_dependencies: usize,
}

/// Query optimizer combining prefetching and caching
pub struct QueryOptimizer {
    /// Predictive prefetcher
    prefetcher: Arc<PredictivePrefetcher>,
    /// Result cache
    cache: Arc<SmartResultCache>,
    /// Query history for optimization
    query_stats: Arc<RwLock<QueryStats>>,
}

/// Query statistics for optimization decisions
#[derive(Debug, Default, Clone)]
struct QueryStats {
    total_queries: u64,
    cache_hits: u64,
    cache_misses: u64,
    prefetch_hits: u64,
    prefetch_misses: u64,
    avg_latency_ms: f64,
}

impl QueryOptimizer {
    /// Create a new query optimizer
    pub fn new(
        max_history: usize,
        cache_capacity: usize,
        cache_ttl_secs: u64,
    ) -> Self {
        Self {
            prefetcher: Arc::new(PredictivePrefetcher::new(max_history, 0.3)),
            cache: Arc::new(SmartResultCache::new(cache_capacity, cache_ttl_secs)),
            query_stats: Arc::new(RwLock::new(QueryStats::default())),
        }
    }

    /// Optimize query execution
    pub async fn optimize_query(
        &self,
        query_hash: u64,
        query_hilbert: Option<u64>,
    ) -> QueryOptimizationHints {
        let mut stats = self.query_stats.write().await;
        stats.total_queries += 1;
        
        // Check cache first
        let cached_result = self.cache.get(query_hash).await;
        if cached_result.is_some() {
            stats.cache_hits += 1;
        } else {
            stats.cache_misses += 1;
        }
        
        // Predict files to prefetch
        let prefetch_files = self.prefetcher.predict_prefetch(query_hilbert).await;
        
        // Execute prefetch
        self.prefetcher.execute_prefetch(prefetch_files.clone()).await;
        
        QueryOptimizationHints {
            cached_result,
            prefetch_files,
            use_progressive_search: stats.avg_latency_ms > 50.0,
            skip_levels: Vec::new(), // TODO: Implement level skipping logic
        }
    }

    /// Record query execution for learning
    pub async fn record_execution(
        &self,
        query_hash: u64,
        query_hilbert: Option<u64>,
        results: Vec<OptimizedSearchRecord>,
        accessed_files: Vec<String>,
        latency_ms: u64,
    ) {
        // Update statistics
        {
            let mut stats = self.query_stats.write().await;
            let alpha = 0.1; // Exponential moving average factor
            stats.avg_latency_ms = stats.avg_latency_ms * (1.0 - alpha) + 
                                   latency_ms as f64 * alpha;
        }
        
        // Record query pattern
        let pattern = QueryPattern {
            query_hash,
            hilbert_key: query_hilbert,
            accessed_files: accessed_files.clone(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            latency_ms,
            result_count: results.len(),
        };
        self.prefetcher.record_query(pattern).await;
        
        // Cache results if query was expensive
        if latency_ms > 20 {
            self.cache.put(query_hash, results, accessed_files).await;
        }
    }

    /// Invalidate cache for modified files
    pub async fn invalidate_files(&self, files: &[String]) {
        for file in files {
            self.cache.invalidate_file(file).await;
        }
    }

    /// Get optimization statistics
    pub async fn get_stats(&self) -> (QueryStats, CacheStats) {
        let query_stats = self.query_stats.read().await.clone();
        let cache_stats = self.cache.get_stats().await;
        (query_stats, cache_stats)
    }
}

/// Query optimization hints
pub struct QueryOptimizationHints {
    /// Cached result if available
    pub cached_result: Option<Vec<OptimizedSearchRecord>>,
    /// Files to prefetch
    pub prefetch_files: Vec<String>,
    /// Whether to use progressive search
    pub use_progressive_search: bool,
    /// Levels to skip during search
    pub skip_levels: Vec<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_predictive_prefetcher() {
        let prefetcher = PredictivePrefetcher::new(100, 0.3);
        
        // Record some query patterns
        let pattern1 = QueryPattern {
            query_hash: 123,
            hilbert_key: Some(1000),
            accessed_files: vec!["file1.helix".to_string(), "file2.helix".to_string()],
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            latency_ms: 25,
            result_count: 10,
        };
        
        prefetcher.record_query(pattern1).await;
        
        // Predict for similar query
        let predictions = prefetcher.predict_prefetch(Some(1050)).await;
        assert!(!predictions.is_empty());
    }

    #[tokio::test]
    async fn test_result_cache() {
        let cache = SmartResultCache::new(100, 60);
        
        // Cache a result
        let results = vec![OptimizedSearchRecord::new("test".to_string(), 0.9)
            .with_similarity(0.1)
            .with_vector(vec![1.0, 2.0, 3.0])
            .with_metadata(TypedMetadata::new())
            .with_version_info(0, 0)];
        
        cache.put(123, results.clone(), vec!["file1.helix".to_string()]).await;
        
        // Retrieve cached result
        let cached = cache.get(123).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 1);
        
        // Invalidate file
        cache.invalidate_file("file1.helix").await;
        
        // Should be gone
        let cached = cache.get(123).await;
        assert!(cached.is_none());
    }
}