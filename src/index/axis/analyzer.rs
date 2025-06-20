// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Collection Analyzer - Analyzes collection characteristics for strategy selection

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{
    AccessFrequencyMetrics, CollectionCharacteristics, MetadataComplexity, PerformanceMetrics,
    QueryDistribution, QueryPatternAnalysis, QueryPatternType, TemporalPattern,
};
use crate::core::{CollectionId, VectorRecord};

/// Analyzer for collection characteristics and behavior patterns
pub struct CollectionAnalyzer {
    /// Query pattern tracker
    query_tracker: Arc<RwLock<QueryPatternTracker>>,

    /// Performance metrics collector
    performance_collector: Arc<PerformanceMetricsCollector>,

    /// Vector characteristics analyzer
    vector_analyzer: Arc<VectorCharacteristicsAnalyzer>,

    /// Metadata complexity analyzer
    metadata_analyzer: Arc<MetadataComplexityAnalyzer>,

    /// Temporal pattern detector
    temporal_detector: Arc<TemporalPatternDetector>,
}

/// Query pattern tracking
struct QueryPatternTracker {
    /// Query history per collection
    query_history: HashMap<CollectionId, Vec<QueryEvent>>,

    /// Query statistics
    query_stats: HashMap<CollectionId, QueryStatistics>,

    /// Maximum history size
    max_history_size: usize,
}

/// Individual query event
#[derive(Debug, Clone)]
struct QueryEvent {
    pub query_type: QueryType,
    pub timestamp: DateTime<Utc>,
    pub latency_ms: f64,
    pub k_value: Option<usize>,
    pub metadata_filters_count: usize,
    pub result_count: usize,
    pub success: bool,
}

/// Types of queries
#[derive(Debug, Clone, Copy, PartialEq)]
enum QueryType {
    PointLookup,
    SimilaritySearch,
    FilteredSearch,
    Hybrid,
}

/// Query statistics for a collection
#[derive(Debug, Clone, Default)]
struct QueryStatistics {
    pub total_queries: u64,
    pub point_queries: u64,
    pub similarity_queries: u64,
    pub filtered_queries: u64,
    pub hybrid_queries: u64,
    pub average_k: f32,
    pub average_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub success_rate: f32,
    pub last_updated: DateTime<Utc>,
}

/// Performance metrics collector
struct PerformanceMetricsCollector {
    /// Current metrics per collection
    current_metrics: Arc<RwLock<HashMap<CollectionId, PerformanceMetrics>>>,

    /// Historical metrics for trend analysis
    historical_metrics: Arc<RwLock<HashMap<CollectionId, Vec<TimestampedMetrics>>>>,
}

/// Timestamped performance metrics
#[derive(Debug, Clone)]
struct TimestampedMetrics {
    pub metrics: PerformanceMetrics,
    pub timestamp: DateTime<Utc>,
}

/// Vector characteristics analyzer
struct VectorCharacteristicsAnalyzer {
    /// Cached characteristics per collection
    cached_characteristics: Arc<RwLock<HashMap<CollectionId, VectorCharacteristics>>>,
}

/// Vector characteristics
#[derive(Debug, Clone)]
struct VectorCharacteristics {
    pub vector_count: u64,
    pub dimension: usize,
    pub average_sparsity: f32,
    pub sparsity_variance: f32,
    pub dimension_variance: Vec<f32>,
    pub growth_rate: f32,
    pub last_analyzed: DateTime<Utc>,
}

/// Metadata complexity analyzer
struct MetadataComplexityAnalyzer {
    /// Cached complexity analysis per collection
    cached_complexity: Arc<RwLock<HashMap<CollectionId, MetadataComplexity>>>,
}

/// Temporal pattern detector
struct TemporalPatternDetector {
    /// Access patterns per collection
    access_patterns: Arc<RwLock<HashMap<CollectionId, AccessPattern>>>,
}

/// Access pattern information
#[derive(Debug, Clone)]
struct AccessPattern {
    pub hourly_distribution: [f32; 24],
    pub daily_distribution: [f32; 7],
    pub recent_activity: Vec<ActivityBurst>,
    pub temporal_pattern: TemporalPattern,
    pub last_updated: DateTime<Utc>,
}

/// Activity burst detection
#[derive(Debug, Clone)]
struct ActivityBurst {
    pub start_time: DateTime<Utc>,
    pub duration: Duration,
    pub intensity: f32,
    pub query_count: u64,
}

impl CollectionAnalyzer {
    /// Create new collection analyzer
    pub async fn new() -> Result<Self> {
        Ok(Self {
            query_tracker: Arc::new(RwLock::new(QueryPatternTracker::new(10000))),
            performance_collector: Arc::new(PerformanceMetricsCollector::new()),
            vector_analyzer: Arc::new(VectorCharacteristicsAnalyzer::new()),
            metadata_analyzer: Arc::new(MetadataComplexityAnalyzer::new()),
            temporal_detector: Arc::new(TemporalPatternDetector::new()),
        })
    }

    /// Analyze collection characteristics
    pub async fn analyze_collection(
        &self,
        collection_id: &CollectionId,
    ) -> Result<CollectionCharacteristics> {
        // Get vector characteristics
        let vector_chars = self.vector_analyzer.analyze_vectors(collection_id).await?;

        // Get query patterns
        let query_patterns = self
            .query_tracker
            .read()
            .await
            .analyze_patterns(collection_id);

        // Get performance metrics
        let performance_metrics = self
            .performance_collector
            .get_metrics(collection_id)
            .await
            .unwrap_or_default();

        // Get access frequency
        let access_frequency = self.calculate_access_frequency(collection_id).await?;

        // Get metadata complexity
        let metadata_complexity = self
            .metadata_analyzer
            .analyze_complexity(collection_id)
            .await?;

        Ok(CollectionCharacteristics {
            collection_id: collection_id.clone(),
            vector_count: vector_chars.vector_count,
            average_sparsity: vector_chars.average_sparsity,
            sparsity_variance: vector_chars.sparsity_variance,
            dimension_variance: vector_chars.dimension_variance,
            query_patterns,
            performance_metrics,
            growth_rate: vector_chars.growth_rate,
            access_frequency,
            metadata_complexity,
        })
    }

    /// Record a query event for pattern analysis
    pub async fn record_query(
        &self,
        collection_id: &CollectionId,
        query_type: QueryPatternType,
        latency_ms: f64,
        k_value: Option<usize>,
        metadata_filters_count: usize,
        result_count: usize,
        success: bool,
    ) -> Result<()> {
        let event = QueryEvent {
            query_type: self.convert_query_type(query_type),
            timestamp: Utc::now(),
            latency_ms,
            k_value,
            metadata_filters_count,
            result_count,
            success,
        };

        let mut tracker = self.query_tracker.write().await;
        tracker.record_query(collection_id, event);

        Ok(())
    }

    /// Update performance metrics
    pub async fn update_performance_metrics(
        &self,
        collection_id: &CollectionId,
        metrics: PerformanceMetrics,
    ) -> Result<()> {
        self.performance_collector
            .update_metrics(collection_id, metrics)
            .await;
        Ok(())
    }

    /// Analyze vector characteristics from sample
    pub async fn analyze_vector_sample(
        &self,
        collection_id: &CollectionId,
        vectors: &[VectorRecord],
    ) -> Result<()> {
        self.vector_analyzer
            .analyze_sample(collection_id, vectors)
            .await;
        Ok(())
    }

    /// Calculate access frequency metrics
    async fn calculate_access_frequency(
        &self,
        collection_id: &CollectionId,
    ) -> Result<AccessFrequencyMetrics> {
        let tracker = self.query_tracker.read().await;
        let stats = tracker
            .query_stats
            .get(collection_id)
            .cloned()
            .unwrap_or_default();

        // Calculate metrics from query statistics
        let time_window_hours = 1.0;
        let queries_in_window = stats.total_queries as f64;

        Ok(AccessFrequencyMetrics {
            reads_per_second: queries_in_window / (time_window_hours * 3600.0),
            writes_per_second: 0.0,          // TODO: Track write operations
            read_write_ratio: f64::INFINITY, // Mostly reads
            peak_qps: queries_in_window / (time_window_hours * 3600.0) * 2.0, // Estimate peak
        })
    }

    /// Convert query pattern type to internal query type
    fn convert_query_type(&self, pattern_type: QueryPatternType) -> QueryType {
        match pattern_type {
            QueryPatternType::PointLookup => QueryType::PointLookup,
            QueryPatternType::SimilaritySearch => QueryType::SimilaritySearch,
            QueryPatternType::FilteredSearch => QueryType::FilteredSearch,
            QueryPatternType::Analytical => QueryType::Hybrid,
        }
    }
}

impl QueryPatternTracker {
    /// Create new query pattern tracker
    fn new(max_history_size: usize) -> Self {
        Self {
            query_history: HashMap::new(),
            query_stats: HashMap::new(),
            max_history_size,
        }
    }

    /// Record a query event
    fn record_query(&mut self, collection_id: &CollectionId, event: QueryEvent) {
        // Add to history
        let history = self
            .query_history
            .entry(collection_id.clone())
            .or_insert_with(Vec::new);
        history.push(event.clone());

        // Maintain history size
        if history.len() > self.max_history_size {
            history.remove(0);
        }

        // Update statistics
        let stats = self
            .query_stats
            .entry(collection_id.clone())
            .or_insert_with(QueryStatistics::default);

        stats.total_queries += 1;

        match event.query_type {
            QueryType::PointLookup => stats.point_queries += 1,
            QueryType::SimilaritySearch => stats.similarity_queries += 1,
            QueryType::FilteredSearch => stats.filtered_queries += 1,
            QueryType::Hybrid => stats.hybrid_queries += 1,
        }

        if let Some(k) = event.k_value {
            stats.average_k = (stats.average_k * (stats.total_queries - 1) as f32 + k as f32)
                / stats.total_queries as f32;
        }

        // Update latency metrics
        stats.average_latency_ms = (stats.average_latency_ms * (stats.total_queries - 1) as f64
            + event.latency_ms)
            / stats.total_queries as f64;

        // Update success rate
        let successful_queries = if event.success { 1.0 } else { 0.0 };
        stats.success_rate = (stats.success_rate * (stats.total_queries - 1) as f32
            + successful_queries)
            / stats.total_queries as f32;

        stats.last_updated = Utc::now();
    }

    /// Analyze query patterns for a collection
    fn analyze_patterns(&self, collection_id: &CollectionId) -> QueryPatternAnalysis {
        let stats = self
            .query_stats
            .get(collection_id)
            .cloned()
            .unwrap_or_default();

        let total = stats.total_queries as f32;
        if total == 0.0 {
            return QueryPatternAnalysis {
                total_queries: 0,
                point_query_percentage: 0.0,
                similarity_search_percentage: 0.0,
                metadata_filter_percentage: 0.0,
                average_k: 10.0,
                query_distribution: QueryDistribution {
                    uniform: true,
                    hotspot_percentage: 0.0,
                    temporal_pattern: TemporalPattern::Uniform,
                },
            };
        }

        QueryPatternAnalysis {
            total_queries: stats.total_queries,
            point_query_percentage: stats.point_queries as f32 / total,
            similarity_search_percentage: stats.similarity_queries as f32 / total,
            metadata_filter_percentage: stats.filtered_queries as f32 / total,
            average_k: stats.average_k,
            query_distribution: QueryDistribution {
                uniform: true,                              // TODO: Analyze actual distribution
                hotspot_percentage: 0.1,                    // TODO: Calculate hotspots
                temporal_pattern: TemporalPattern::Uniform, // TODO: Detect temporal patterns
            },
        }
    }
}

impl PerformanceMetricsCollector {
    /// Create new performance metrics collector
    fn new() -> Self {
        Self {
            current_metrics: Arc::new(RwLock::new(HashMap::new())),
            historical_metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Update performance metrics for a collection
    async fn update_metrics(&self, collection_id: &CollectionId, metrics: PerformanceMetrics) {
        // Update current metrics
        let mut current = self.current_metrics.write().await;
        current.insert(collection_id.clone(), metrics.clone());
        drop(current);

        // Add to historical metrics
        let mut historical = self.historical_metrics.write().await;
        let history = historical
            .entry(collection_id.clone())
            .or_insert_with(Vec::new);

        history.push(TimestampedMetrics {
            metrics,
            timestamp: Utc::now(),
        });

        // Keep only recent history (last 24 hours)
        let cutoff = Utc::now() - Duration::hours(24);
        history.retain(|m| m.timestamp > cutoff);
    }

    /// Get current performance metrics
    async fn get_metrics(&self, collection_id: &CollectionId) -> Option<PerformanceMetrics> {
        let current = self.current_metrics.read().await;
        current.get(collection_id).cloned()
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self {
            average_query_latency_ms: 50.0,
            p99_query_latency_ms: 200.0,
            queries_per_second: 100.0,
            index_size_mb: 1024.0,
            memory_usage_mb: 2048.0,
            cache_hit_rate: 0.8,
        }
    }
}

impl VectorCharacteristicsAnalyzer {
    /// Create new vector characteristics analyzer
    fn new() -> Self {
        Self {
            cached_characteristics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Analyze vector characteristics for a collection
    async fn analyze_vectors(&self, collection_id: &CollectionId) -> Result<VectorCharacteristics> {
        let cached = self.cached_characteristics.read().await;
        if let Some(chars) = cached.get(collection_id) {
            // Return cached if recent (within 1 hour)
            if Utc::now() - chars.last_analyzed < Duration::hours(1) {
                return Ok(chars.clone());
            }
        }
        drop(cached);

        // TODO: Analyze actual vectors from storage
        // For now, return default characteristics
        let characteristics = VectorCharacteristics {
            vector_count: 10000,
            dimension: 384,
            average_sparsity: 0.3,
            sparsity_variance: 0.1,
            dimension_variance: vec![0.5; 384],
            growth_rate: 0.1, // 10% growth
            last_analyzed: Utc::now(),
        };

        // Cache the result
        let mut cached = self.cached_characteristics.write().await;
        cached.insert(collection_id.clone(), characteristics.clone());

        Ok(characteristics)
    }

    /// Analyze a sample of vectors
    async fn analyze_sample(&self, collection_id: &CollectionId, vectors: &[VectorRecord]) {
        if vectors.is_empty() {
            return;
        }

        let dimension = vectors[0].vector.len();
        let mut total_sparsity = 0.0;
        let mut sparsity_values = Vec::new();

        // Calculate sparsity statistics
        for vector in vectors {
            let zero_count = vector.vector.iter().filter(|&&x| x == 0.0).count();
            let sparsity = zero_count as f32 / vector.vector.len() as f32;
            total_sparsity += sparsity;
            sparsity_values.push(sparsity);
        }

        let average_sparsity = total_sparsity / vectors.len() as f32;

        // Calculate sparsity variance
        let sparsity_variance = sparsity_values
            .iter()
            .map(|&s| (s - average_sparsity).powi(2))
            .sum::<f32>()
            / vectors.len() as f32;

        // Calculate dimension variance
        let mut dimension_means = vec![0.0; dimension];
        for vector in vectors {
            for (i, &value) in vector.vector.iter().enumerate() {
                if i < dimension {
                    dimension_means[i] += value;
                }
            }
        }

        for mean in dimension_means.iter_mut() {
            *mean /= vectors.len() as f32;
        }

        let mut dimension_variance = vec![0.0; dimension];
        for vector in vectors {
            for (i, &value) in vector.vector.iter().enumerate() {
                if i < dimension {
                    let diff = value - dimension_means[i];
                    dimension_variance[i] += diff * diff;
                }
            }
        }

        for variance in dimension_variance.iter_mut() {
            *variance /= vectors.len() as f32;
        }

        // Update cached characteristics
        let characteristics = VectorCharacteristics {
            vector_count: vectors.len() as u64,
            dimension,
            average_sparsity,
            sparsity_variance,
            dimension_variance,
            growth_rate: 0.0, // Cannot calculate from single sample
            last_analyzed: Utc::now(),
        };

        let mut cached = self.cached_characteristics.write().await;
        cached.insert(collection_id.clone(), characteristics);
    }
}

impl MetadataComplexityAnalyzer {
    /// Create new metadata complexity analyzer
    fn new() -> Self {
        Self {
            cached_complexity: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Analyze metadata complexity for a collection
    async fn analyze_complexity(&self, collection_id: &CollectionId) -> Result<MetadataComplexity> {
        let cached = self.cached_complexity.read().await;
        if let Some(complexity) = cached.get(collection_id) {
            return Ok(complexity.clone());
        }
        drop(cached);

        // TODO: Analyze actual metadata from storage
        // For now, return default complexity
        let complexity = MetadataComplexity {
            field_count: 5,
            average_field_cardinality: 100.0,
            nested_depth: 2,
            filter_selectivity: 0.1,
        };

        // Cache the result
        let mut cached = self.cached_complexity.write().await;
        cached.insert(collection_id.clone(), complexity.clone());

        Ok(complexity)
    }
}

impl TemporalPatternDetector {
    /// Create new temporal pattern detector
    fn new() -> Self {
        Self {
            access_patterns: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Detect temporal patterns for a collection
    async fn detect_patterns(&self, collection_id: &CollectionId) -> TemporalPattern {
        let patterns = self.access_patterns.read().await;
        if let Some(pattern) = patterns.get(collection_id) {
            return pattern.temporal_pattern.clone();
        }

        // Default to uniform pattern
        TemporalPattern::Uniform
    }
}
