// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Adaptive Index Engine - Intelligence core for AXIS

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::index::axis::{
    AxisConfig, CollectionAnalyzer,
    management::strategy::{OptimizationConfig, OptimizationGoal, CollectionStatistics, QueryPatterns, IndexStrategyBuilder},
    types::{Data, IndexAlgorithm, IndexSpecification, IndexSelectionStrategy},
};
use super::migration_engine::{MigrationDecision, MigrationComplexity};


/// Engine for analyzing collections and recommending optimal indexing strategies
pub struct AdaptiveIndexEngine {
    /// Collection analyzer for data characteristics
    collection_analyzer: Arc<CollectionAnalyzer>,

    /// Strategy selector for choosing optimal indexing approach
    strategy_selector: Arc<IndexStrategySelector>,

    /// Performance predictor using ML models
    performance_predictor: Arc<PerformancePredictor>,

    /// Decision history for learning
    decision_history: Arc<RwLock<Vec<StrategyDecision>>>,

    /// Configuration
    config: AxisConfig,
}

impl std::fmt::Debug for AdaptiveIndexEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdaptiveIndexEngine")
            .field("config", &self.config)
            .finish()
    }
}

/// Collection characteristics for strategy selection
#[derive(Debug, Clone)]
pub struct CollectionCharacteristics {
    pub collection_id: String,
    pub vector_count: u64,
    pub average_sparsity: f32,
    pub sparsity_variance: f32,
    pub dimension_variance: Vec<f32>,
    pub query_patterns: QueryPatternAnalysis,
    pub performance_metrics: PerformanceMetrics,
    pub growth_rate: f32,
    pub access_frequency: AccessFrequencyMetrics,
    pub metadata_complexity: MetadataComplexity,
}

/// Query pattern analysis
#[derive(Debug, Clone)]
pub struct QueryPatternAnalysis {
    pub total_queries: u64,
    pub point_query_percentage: f32,
    pub similarity_search_percentage: f32,
    pub metadata_filter_percentage: f32,
    pub average_k: f32,
    pub query_distribution: QueryDistribution,
}

impl QueryPatternAnalysis {
    pub fn primarily_point_queries(&self) -> bool {
        self.point_query_percentage > 0.7
    }

    pub fn primarily_similarity_search(&self) -> bool {
        self.similarity_search_percentage > 0.7
    }

    pub fn ann_heavy(&self) -> bool {
        self.similarity_search_percentage > 0.5
    }
}

/// Query distribution patterns
#[derive(Debug, Clone)]
pub struct QueryDistribution {
    pub uniform: bool,
    pub hotspot_percentage: f32,
    pub temporal_pattern: TemporalPattern,
}

/// Temporal access patterns
#[derive(Debug, Clone)]
pub enum TemporalPattern {
    Uniform,
    Recent,                        // Most queries for recent data
    Periodic(std::time::Duration), // Periodic access pattern
    Bursty,                        // Sudden bursts of activity
}

/// Current performance metrics
#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub average_query_latency_ms: f64,
    pub p99_query_latency_ms: f64,
    pub queries_per_second: f64,
    pub index_size_mb: f64,
    pub memory_usage_mb: f64,
    pub cache_hit_rate: f64,
}

/// Access frequency metrics
#[derive(Debug, Clone)]
pub struct AccessFrequencyMetrics {
    pub reads_per_second: f64,
    pub writes_per_second: f64,
    pub read_write_ratio: f64,
    pub peak_qps: f64,
}

/// Metadata complexity analysis
#[derive(Debug, Clone)]
pub struct MetadataComplexity {
    pub field_count: usize,
    pub average_field_cardinality: f64,
    pub nested_depth: usize,
    pub filter_selectivity: f64,
}

/// Strategy selection engine
pub struct IndexStrategySelector {
    /// Optimization configurations for different scenarios
    optimization_configs: std::collections::HashMap<StrategyType, OptimizationConfig>,
}

/// Strategy types for different collection profiles
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum StrategyType {
    SmallDense,
    LargeDense,
    SmallSparse,
    LargeSparse,
    Mixed,
    MetadataHeavy,
    HighThroughput,
    Analytical,
}

/// Strategy configuration template
#[derive(Debug, Clone)]
pub struct StrategyConfigTemplate {
    pub strategy_type: StrategyType,
    pub optimization_config: OptimizationConfig,
    pub applicability_conditions: ApplicabilityConditions,
}

/// Conditions for strategy applicability
#[derive(Debug, Clone)]
pub struct ApplicabilityConditions {
    pub min_vector_count: Option<u64>,
    pub max_vector_count: Option<u64>,
    pub min_sparsity: Option<f32>,
    pub max_sparsity: Option<f32>,
    pub required_query_pattern: Option<QueryPatternType>,
}

/// Query pattern types
#[derive(Debug, Clone, PartialEq)]
pub enum QueryPatternType {
    PointLookup,
    SimilaritySearch,
    FilteredSearch,
    Analytical,
}

/// Performance predictor using ML models
pub struct PerformancePredictor {
    /// Prediction models
    models: Arc<RwLock<PredictionModels>>,
}

/// ML models for performance prediction
pub struct PredictionModels {
    pub latency_model: Option<Box<dyn LatencyPredictor + Send + Sync>>,
    pub throughput_model: Option<Box<dyn ThroughputPredictor + Send + Sync>>,
    pub resource_model: Option<Box<dyn ResourcePredictor + Send + Sync>>,
}

/// Latency prediction trait
pub trait LatencyPredictor {
    fn predict(&self, features: &PredictionFeatures) -> Result<LatencyPrediction>;
}

/// Throughput prediction trait
pub trait ThroughputPredictor {
    fn predict(&self, features: &PredictionFeatures) -> Result<ThroughputPrediction>;
}

/// Resource usage prediction trait
pub trait ResourcePredictor {
    fn predict(&self, features: &PredictionFeatures) -> Result<ResourcePrediction>;
}

/// Features for ML prediction
#[derive(Debug, Clone)]
pub struct PredictionFeatures {
    pub vector_count: f64,
    pub sparsity: f64,
    pub dimension: f64,
    pub query_rate: f64,
    pub index_type: String,
}

/// Latency prediction result
#[derive(Debug, Clone)]
pub struct LatencyPrediction {
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
}

/// Throughput prediction result
#[derive(Debug, Clone)]
pub struct ThroughputPrediction {
    pub max_qps: f64,
    pub sustainable_qps: f64,
}

/// Resource usage prediction
#[derive(Debug, Clone)]
pub struct ResourcePrediction {
    pub memory_mb: f64,
    pub cpu_cores: f64,
    pub disk_mb: f64,
}

/// Strategy decision record
#[derive(Debug, Clone)]
pub struct StrategyDecision {
    pub collection_id: String,
    pub timestamp: DateTime<Utc>,
    pub characteristics: CollectionCharacteristics,
    pub recommended_strategy: IndexSelectionStrategy,
    pub decision_reason: String,
    pub expected_improvement: f64,
}

impl AdaptiveIndexEngine {
    /// Create new adaptive index engine
    pub async fn new(config: AxisConfig) -> Result<Self> {
        let collection_analyzer = Arc::new(CollectionAnalyzer::new().await?);
        let strategy_selector = Arc::new(IndexStrategySelector::new());
        let performance_predictor = Arc::new(PerformancePredictor::new().await?);

        Ok(Self {
            collection_analyzer,
            strategy_selector,
            performance_predictor,
            decision_history: Arc::new(RwLock::new(Vec::new())),
            config,
        })
    }

    /// Analyze collection characteristics
    pub async fn analyze_collection(
        &self,
        collection_id: &str,
    ) -> Result<CollectionCharacteristics> {
        self.collection_analyzer
            .analyze_collection(collection_id)
            .await
    }

    /// Recommend optimal indexing strategy
    pub async fn recommend_strategy(
        &self,
        characteristics: &CollectionCharacteristics,
    ) -> Result<IndexSelectionStrategy> {
        // Determine strategy type based on characteristics
        let strategy_type = self.determine_strategy_type(characteristics);

        // Get base strategy from selector
        let base_strategy = self
            .strategy_selector
            .select_strategy(strategy_type, characteristics)
            .await?;

        // Refine using ML predictions if enabled
        if self.config.strategy_config.use_ml_models
            && characteristics.vector_count >= self.config.strategy_config.min_training_size as u64
        {
            let refined_strategy = self
                .refine_with_ml_predictions(base_strategy, characteristics)
                .await?;

            return Ok(refined_strategy);
        }

        Ok(base_strategy)
    }

    /// Determine if migration is beneficial
    pub async fn should_migrate(
        &self,
        collection_id: &str,
        characteristics: &CollectionCharacteristics,
    ) -> Result<MigrationDecision> {
        // Get current strategy (mock for now)
        let current_strategy = self.get_current_strategy(collection_id).await?;

        // Get optimal strategy
        let optimal_strategy = self.recommend_strategy(characteristics).await?;

        // Check if strategies are the same
        if current_strategy.indexes == optimal_strategy.indexes {
            return Ok(MigrationDecision::Stay {
                reason: "Already using optimal strategy".to_string(),
            });
        }

        // Calculate improvement potential
        let improvement = self
            .calculate_improvement_potential(&current_strategy, &optimal_strategy, characteristics)
            .await?;

        // Check against threshold
        if improvement < self.config.migration_config.improvement_threshold as f64 {
            return Ok(MigrationDecision::Stay {
                reason: format!(
                    "Improvement ({:.2}%) below threshold ({:.2}%)",
                    improvement * 100.0,
                    self.config.migration_config.improvement_threshold as f64 * 100.0
                ),
            });
        }

        // Estimate migration complexity and duration
        let complexity = self.estimate_migration_complexity(
            &current_strategy,
            &optimal_strategy,
            characteristics,
        );

        let duration = self.estimate_migration_duration(characteristics, complexity);

        // Record decision
        let decision = StrategyDecision {
            collection_id: collection_id.to_string(),
            timestamp: Utc::now(),
            characteristics: characteristics.clone(),
            recommended_strategy: optimal_strategy.clone(),
            decision_reason: format!("Expected improvement: {:.2}%", improvement * 100.0),
            expected_improvement: improvement,
        };

        let mut history = self.decision_history.write().await;
        history.push(decision);

        // Convert complexity score to MigrationComplexity enum
        let migration_complexity = if complexity < 0.3 {
            MigrationComplexity::Low
        } else if complexity < 0.7 {
            MigrationComplexity::Medium
        } else {
            MigrationComplexity::High
        };

        Ok(MigrationDecision::Migrate {
            from: current_strategy,
            to: optimal_strategy,
            estimated_improvement: improvement as f32,
            complexity: migration_complexity,
            estimated_duration: duration,
        })
    }

    /// Determine strategy type from characteristics
    fn determine_strategy_type(&self, characteristics: &CollectionCharacteristics) -> StrategyType {
        match (
            characteristics.vector_count,
            characteristics.average_sparsity,
        ) {
            (count, sparsity) if count < 10_000 && sparsity < 0.1 => StrategyType::SmallDense,
            (count, sparsity) if count >= 10_000 && sparsity < 0.1 => StrategyType::LargeDense,
            (count, sparsity) if count < 10_000 && sparsity > 0.7 => StrategyType::SmallSparse,
            (count, sparsity) if count >= 10_000 && sparsity > 0.7 => StrategyType::LargeSparse,
            _ => StrategyType::Mixed,
        }
    }

    /// Refine strategy using ML predictions
    async fn refine_with_ml_predictions(
        &self,
        base_strategy: IndexSelectionStrategy,
        _characteristics: &CollectionCharacteristics,
    ) -> Result<IndexSelectionStrategy> {
        // TODO: Implement ML-based refinement
        Ok(base_strategy)
    }

    /// Calculate improvement potential
    async fn calculate_improvement_potential(
        &self,
        current: &IndexSelectionStrategy,
        optimal: &IndexSelectionStrategy,
        _characteristics: &CollectionCharacteristics,
    ) -> Result<f64> {
        // Simple heuristic for now
        // TODO: Use ML models for accurate prediction

        // Calculate improvement based on index specifications
        let current_has_vector = current.indexes.iter()
            .any(|idx| matches!(idx.data_type, Data::DenseVector { .. } | Data::SparseVector { .. }));
        let optimal_has_vector = optimal.indexes.iter()
            .any(|idx| matches!(idx.data_type, Data::DenseVector { .. } | Data::SparseVector { .. }));
        
        let improvement = match (current_has_vector, optimal_has_vector) {
            (false, true) => 0.6,  // 60% improvement adding vector index
            (true, true) => 0.2,   // 20% improvement optimizing existing
            _ => 0.1,              // Default 10% improvement
        };

        Ok(improvement)
    }

    /// Estimate migration complexity
    fn estimate_migration_complexity(
        &self,
        current: &IndexSelectionStrategy,
        target: &IndexSelectionStrategy,
        characteristics: &CollectionCharacteristics,
    ) -> f64 {
        // Complexity based on data size and strategy difference
        let size_factor = (characteristics.vector_count as f64).log10() / 10.0;
        let strategy_difference = self.calculate_strategy_difference(current, target);

        size_factor * strategy_difference
    }

    /// Calculate difference between strategies
    fn calculate_strategy_difference(
        &self,
        current: &IndexSelectionStrategy,
        target: &IndexSelectionStrategy,
    ) -> f64 {
        // Simple heuristic based on number of index differences
        let current_count = current.indexes.len();
        let target_count = target.indexes.len();
        let count_diff = (current_count as f64 - target_count as f64).abs();
        
        // Normalize to 0-1 range
        (count_diff / 10.0).min(1.0)
    }

    /// Estimate migration duration
    fn estimate_migration_duration(
        &self,
        characteristics: &CollectionCharacteristics,
        complexity: f64,
    ) -> std::time::Duration {
        // Estimate based on vector count and complexity
        let base_time_per_vector_ms = 0.1; // 0.1ms per vector
        let total_ms = characteristics.vector_count as f64 * base_time_per_vector_ms * complexity;

        std::time::Duration::from_millis(total_ms as u64)
    }

    /// Get current strategy (mock implementation)
    async fn get_current_strategy(&self, _collection_id: &str) -> Result<IndexSelectionStrategy> {
        // TODO: Get from actual storage
        // For now, return a simple default strategy with identifier index
        use crate::index::axis::types::{QueryCondition, ResultCombination, RoutingRule};
        
        Ok(IndexSelectionStrategy {
            indexes: vec![
                IndexSpecification {
                    data_type: Data::Metadata, // Using Metadata type for ID indexing
                    algorithm: IndexAlgorithm::BTree { max_keys_per_node: 256 },
                    name: Some("default_id".to_string()),
                    is_primary: false,
                    selectivity_threshold: None,
                },
            ],
            routing_rules: vec![
                RoutingRule {
                    condition: QueryCondition::Always,
                    use_indexes: vec![0],
                    combination: ResultCombination::First,
                },
            ],
        })
    }
}

impl IndexStrategySelector {
    /// Create new strategy selector
    pub fn new() -> Self {
        let mut configs = std::collections::HashMap::new();

        // Initialize optimization configs for different scenarios
        configs.insert(
            StrategyType::SmallDense,
            OptimizationConfig {
                goal: OptimizationGoal::MinLatency,
                max_memory_gb: Some(2.0),
                target_latency_ms: Some(10.0),
                min_accuracy: Some(0.95),
            },
        );

        configs.insert(
            StrategyType::LargeDense,
            OptimizationConfig {
                goal: OptimizationGoal::Balanced,
                max_memory_gb: Some(16.0),
                target_latency_ms: Some(50.0),
                min_accuracy: Some(0.95),
            },
        );

        configs.insert(
            StrategyType::SmallSparse,
            OptimizationConfig {
                goal: OptimizationGoal::MinMemory,
                max_memory_gb: Some(1.0),
                target_latency_ms: Some(20.0),
                min_accuracy: Some(0.90),
            },
        );

        configs.insert(
            StrategyType::LargeSparse,
            OptimizationConfig {
                goal: OptimizationGoal::MaxThroughput,
                max_memory_gb: Some(32.0),
                target_latency_ms: Some(100.0),
                min_accuracy: Some(0.95),
            },
        );

        configs.insert(
            StrategyType::Mixed,
            OptimizationConfig::default(),
        );

        configs.insert(
            StrategyType::MetadataHeavy,
            OptimizationConfig {
                goal: OptimizationGoal::MinLatency,
                max_memory_gb: Some(8.0),
                target_latency_ms: Some(5.0),
                min_accuracy: None, // Exact match for metadata
            },
        );

        configs.insert(
            StrategyType::HighThroughput,
            OptimizationConfig {
                goal: OptimizationGoal::MaxThroughput,
                max_memory_gb: Some(64.0),
                target_latency_ms: Some(200.0),
                min_accuracy: Some(0.90),
            },
        );

        configs.insert(
            StrategyType::Analytical,
            OptimizationConfig {
                goal: OptimizationGoal::Balanced,
                max_memory_gb: None,
                target_latency_ms: Some(1000.0),
                min_accuracy: Some(0.99),
            },
        );

        Self {
            optimization_configs: configs,
        }
    }

    /// Select strategy based on type and characteristics
    pub async fn select_strategy(
        &self,
        strategy_type: StrategyType,
        characteristics: &CollectionCharacteristics,
    ) -> Result<IndexSelectionStrategy> {
        let optimization_config = self.optimization_configs
            .get(&strategy_type)
            .ok_or_else(|| anyhow::anyhow!("No config for strategy type: {:?}", strategy_type))?;

        // Convert characteristics to builder format
        let collection_stats = CollectionStatistics {
            total_vectors: characteristics.vector_count as usize,
            vector_dimension: 128, // Default, should be in characteristics
            avg_vector_sparsity: characteristics.average_sparsity,
            has_metadata: characteristics.metadata_complexity.field_count > 0,
            metadata_cardinality: std::collections::HashMap::new(),
            has_text_fields: false,
            update_frequency: characteristics.access_frequency.writes_per_second as f32,
        };

        let query_patterns = QueryPatterns {
            avg_queries_per_second: characteristics.access_frequency.reads_per_second as f32,
            filter_usage_ratio: characteristics.query_patterns.metadata_filter_percentage,
            text_search_ratio: 0.0,
            typical_k: characteristics.query_patterns.average_k as usize,
            recall_requirement: 0.95,
        };

        // Build strategy using the new builder
        IndexStrategyBuilder::new(collection_stats, query_patterns)
            .with_optimization(optimization_config.clone())
            .build()
    }
}

impl PerformancePredictor {
    /// Create new performance predictor
    pub async fn new() -> Result<Self> {
        Ok(Self {
            models: Arc::new(RwLock::new(PredictionModels {
                latency_model: None,
                throughput_model: None,
                resource_model: None,
            })),
        })
    }
}

impl Default for ApplicabilityConditions {
    fn default() -> Self {
        Self {
            min_vector_count: None,
            max_vector_count: None,
            min_sparsity: None,
            max_sparsity: None,
            required_query_pattern: None,
        }
    }
}
