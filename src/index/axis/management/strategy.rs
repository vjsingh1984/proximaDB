//! AXIS Index Strategy Selection
//!
//! This module provides intelligent index selection based on:
//! - Collection characteristics (size, dimensionality, sparsity)
//! - Query patterns (filters, selectivity, frequency)
//! - Performance requirements (latency, throughput, accuracy)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::index::axis::types::{
    Data, IndexAlgorithm, IndexSelectionStrategy, IndexSpecification, QueryCondition,
    ResultCombination, RoutingRule, TextAnalyzer, TokenFilter, Tokenizer,
};

/// Type alias for `IndexSelectionStrategy` for compatibility
pub type IndexStrategy = IndexSelectionStrategy;
/// Type alias for `IndexStrategyBuilder` for compatibility
pub type StrategySelector = IndexStrategyBuilder;
/// Type alias for `OptimizationConfig` for compatibility
pub type StrategyRecommendation = OptimizationConfig;

/// Optimization goals for index selection
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OptimizationGoal {
    /// Minimize query latency
    MinLatency,
    /// Maximize query throughput
    MaxThroughput,
    /// Minimize memory usage
    MinMemory,
    /// Balance all factors
    Balanced,
}

/// Configuration for index optimization
#[derive(Debug, Clone)]
pub struct OptimizationConfig {
    /// Primary optimization goal
    pub goal: OptimizationGoal,
    /// Maximum memory budget in gigabytes
    pub max_memory_gb: Option<f64>,
    /// Target query latency in milliseconds
    pub target_latency_ms: Option<f64>,
    /// Minimum required recall accuracy (0.0–1.0)
    pub min_accuracy: Option<f32>,
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            goal: OptimizationGoal::Balanced,
            max_memory_gb: Some(8.0),
            target_latency_ms: Some(100.0),
            min_accuracy: Some(0.95),
        }
    }
}

/// Collection statistics for strategy selection
#[derive(Debug, Clone)]
pub struct CollectionStatistics {
    /// Total number of vectors in the collection
    pub total_vectors: usize,
    /// Dimensionality of each vector
    pub vector_dimension: usize,
    /// Average fraction of zero elements per vector (0.0–1.0)
    pub avg_vector_sparsity: f32,
    /// Whether the collection has associated metadata fields
    pub has_metadata: bool,
    /// Cardinality per metadata field (field name → distinct value count)
    pub metadata_cardinality: HashMap<String, usize>,
    /// Whether the collection contains full-text searchable fields
    pub has_text_fields: bool,
    /// Estimated write rate in updates per second
    pub update_frequency: f32,
}

/// Query patterns for strategy optimization
#[derive(Debug, Clone)]
pub struct QueryPatterns {
    /// Average query throughput in queries per second
    pub avg_queries_per_second: f32,
    /// Fraction of queries that include metadata filters (0.0–1.0)
    pub filter_usage_ratio: f32,
    /// Fraction of queries that include full-text search (0.0–1.0)
    pub text_search_ratio: f32,
    /// Typical number of nearest neighbours requested per query
    pub typical_k: usize,
    /// Required recall rate for vector search (0.0–1.0)
    pub recall_requirement: f32,
}

/// Automatic index strategy builder
#[derive(Debug, Clone)]
pub struct IndexStrategyBuilder {
    #[cfg(test)]
    pub collection_stats: CollectionStatistics,
    #[cfg(not(test))]
    collection_stats: CollectionStatistics,
    #[cfg(test)]
    pub query_patterns: QueryPatterns,
    #[cfg(not(test))]
    query_patterns: QueryPatterns,
    optimization_config: OptimizationConfig,
}

impl IndexStrategyBuilder {
    /// Create a new strategy builder from collection statistics and observed query patterns
    pub fn new(collection_stats: CollectionStatistics, query_patterns: QueryPatterns) -> Self {
        Self {
            collection_stats,
            query_patterns,
            optimization_config: OptimizationConfig::default(),
        }
    }

    /// Override the optimization configuration
    pub fn with_optimization(mut self, config: OptimizationConfig) -> Self {
        self.optimization_config = config;
        self
    }

    /// Build optimal index strategy
    pub fn build(&self) -> Result<IndexSelectionStrategy> {
        let mut indexes = Vec::new();
        let mut routing_rules = Vec::new();

        // Always add identifier index
        indexes.push(IndexSpecification {
            data_type: Data::Identifier,
            algorithm: IndexAlgorithm::BTree {
                max_keys_per_node: 256,
            },
            name: Some("primary_id".to_string()),
            is_primary: false,
            selectivity_threshold: None,
        });

        // Add vector index if we have vectors
        if self.collection_stats.total_vectors > 0 {
            let vector_index = self.select_vector_index()?;
            indexes.push(vector_index);
        }

        // Add metadata indexes if needed
        if self.collection_stats.has_metadata && self.query_patterns.filter_usage_ratio > 0.1 {
            for (field, cardinality) in &self.collection_stats.metadata_cardinality {
                if *cardinality < 1000 {
                    // Low cardinality - use bloom filter
                    indexes.push(IndexSpecification {
                        data_type: Data::Metadata,
                        algorithm: IndexAlgorithm::BloomFilter {
                            expected_elements: self.collection_stats.total_vectors,
                            false_positive_rate: 0.01,
                        },
                        name: Some(format!("bloom_{}", field)),
                        is_primary: false,
                        selectivity_threshold: Some(0.9),
                    });
                } else {
                    // High cardinality - use BTree
                    indexes.push(IndexSpecification {
                        data_type: Data::Metadata,
                        algorithm: IndexAlgorithm::BTree {
                            max_keys_per_node: 256,
                        },
                        name: Some(format!("btree_{}", field)),
                        is_primary: false,
                        selectivity_threshold: Some(0.5),
                    });
                }
            }
        }

        // Add text index if needed
        if self.collection_stats.has_text_fields && self.query_patterns.text_search_ratio > 0.05 {
            indexes.push(IndexSpecification {
                data_type: Data::FullText,
                algorithm: IndexAlgorithm::InvertedIndex {
                    analyzer: TextAnalyzer {
                        tokenizer: Tokenizer::Standard,
                        filters: vec![TokenFilter::Lowercase, TokenFilter::Stopwords],
                        language: Some("english".to_string()),
                    },
                    enable_positions: true,
                },
                name: Some("text_index".to_string()),
                is_primary: false,
                selectivity_threshold: None,
            });
        }

        // Create routing rules
        routing_rules.push(self.create_default_routing_rule(indexes.len()));

        Ok(IndexSelectionStrategy {
            indexes,
            routing_rules,
        })
    }

    fn select_vector_index(&self) -> Result<IndexSpecification> {
        let dimension = self.collection_stats.vector_dimension;
        let total_vectors = self.collection_stats.total_vectors;
        let sparsity = self.collection_stats.avg_vector_sparsity;

        let data_type = if sparsity > 0.8 {
            Data::SparseVector {
                max_dimension: dimension,
            }
        } else {
            Data::DenseVector { dimension }
        };

        // Select algorithm based on size and requirements
        let algorithm = match (total_vectors, self.optimization_config.goal) {
            // Small collections - use HNSW
            (n, _) if n < 100_000 => IndexAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
                max_elements: n * 2,
            },

            // Large collections with latency focus - partitioned HNSW
            (n, OptimizationGoal::MinLatency) if n < 10_000_000 => IndexAlgorithm::HNSW {
                m: 32,
                ef_construction: 400,
                ef_search: 100,
                max_elements: n + (n / 10), // 10% growth buffer
            },

            // Large collections with memory constraints - IVF+PQ
            (_, OptimizationGoal::MinMemory) => IndexAlgorithm::IVF {
                nlist: (total_vectors as f64).sqrt() as u32,
                nprobe: 10,
                quantizer: Some(Box::new(IndexAlgorithm::PQ {
                    m: 8,
                    nbits: 8,
                    train_size: total_vectors.min(100_000),
                })),
            },

            // Very large collections - IVF
            _ => IndexAlgorithm::IVF {
                nlist: (total_vectors as f64).sqrt() as u32,
                nprobe: 20,
                quantizer: None,
            },
        };

        Ok(IndexSpecification {
            data_type,
            algorithm,
            name: Some("vector_index".to_string()),
            is_primary: true,
            selectivity_threshold: None,
        })
    }

    fn create_default_routing_rule(&self, num_indexes: usize) -> RoutingRule {
        RoutingRule {
            condition: QueryCondition::Always,
            use_indexes: (0..num_indexes).collect(),
            combination: if self.query_patterns.filter_usage_ratio > 0.5 {
                ResultCombination::Intersect
            } else {
                ResultCombination::MergeByScore
            },
        }
    }
}

/// Runtime index strategy that can adapt based on performance
#[derive(Debug, Clone)]
pub struct AdaptiveIndexStrategy {
    /// The initial index selection strategy to adapt from
    pub base_strategy: IndexSelectionStrategy,
    /// Ring buffer of recent query performance observations
    pub performance_history: Vec<QueryPerformance>,
    /// When `false`, the strategy is frozen and will not self-tune
    pub adaptation_enabled: bool,
}

/// Observed performance for a single query execution
#[derive(Debug, Clone)]
pub struct QueryPerformance {
    /// Logical query type (e.g. `"knn"`, `"hybrid"`, `"filter"`)
    pub query_type: String,
    /// Wall-clock latency in milliseconds
    pub latency_ms: f64,
    /// Measured recall against ground truth (0.0–1.0)
    pub recall: f32,
    /// Indices into `base_strategy.indexes` that were consulted
    pub indexes_used: Vec<usize>,
}

impl AdaptiveIndexStrategy {
    /// Create a new adaptive strategy around an existing base strategy
    pub fn new(base_strategy: IndexSelectionStrategy) -> Self {
        Self {
            base_strategy,
            performance_history: Vec::new(),
            adaptation_enabled: true,
        }
    }

    /// Adapt strategy based on observed performance
    pub fn adapt(&mut self) -> Result<()> {
        if !self.adaptation_enabled || self.performance_history.len() < 100 {
            return Ok(());
        }

        // Analyze recent performance
        let recent_perf = &self.performance_history[self.performance_history.len() - 100..];
        let avg_latency =
            recent_perf.iter().map(|p| p.latency_ms).sum::<f64>() / recent_perf.len() as f64;
        let avg_recall =
            recent_perf.iter().map(|p| p.recall).sum::<f32>() / recent_perf.len() as f32;

        // Adjust routing rules if performance is poor
        if avg_latency > 100.0 || avg_recall < 0.9 {
            self.optimize_routing_rules(avg_latency, avg_recall)?;
        }

        Ok(())
    }

    fn optimize_routing_rules(&mut self, avg_latency: f64, avg_recall: f32) -> Result<()> {
        // This is a simplified optimization - real implementation would be more sophisticated
        for rule in &mut self.base_strategy.routing_rules {
            if avg_recall < 0.9 {
                // Increase search scope
                rule.combination = ResultCombination::Union;
            } else if avg_latency > 100.0 {
                // Reduce search scope
                if rule.use_indexes.len() > 1 {
                    rule.use_indexes.truncate(1); // Use primary index only
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::index::axis::*;
    use std::collections::HashMap;

    /// Test struct for collection statistics
    #[derive(Debug, Clone)]
    struct CollectionStatistics {
        total_vectors: u64,
        vector_dimension: usize,
        avg_vector_sparsity: f32,
        has_metadata: bool,
        metadata_cardinality: HashMap<String, usize>,
        has_text_fields: bool,
        update_frequency: f32,
    }

    /// Test struct for query patterns
    #[derive(Debug, Clone)]
    struct QueryPatterns {
        avg_queries_per_second: f32,
        filter_usage_ratio: f32,
        text_search_ratio: f32,
        typical_k: usize,
        recall_requirement: f32,
    }

    /// Test struct for index strategy builder
    #[derive(Debug)]
    struct IndexStrategyBuilder {
        stats: CollectionStatistics,
        patterns: QueryPatterns,
    }

    impl IndexStrategyBuilder {
        fn new(stats: CollectionStatistics, patterns: QueryPatterns) -> Self {
            Self { stats, patterns }
        }

        fn build(self) -> Result<IndexStrategy, String> {
            use crate::index::axis::types::{Data, IndexAlgorithm, IndexSpecification};
            Ok(IndexStrategy {
                indexes: vec![
                    IndexSpecification {
                        data_type: Data::Identifier,
                        algorithm: IndexAlgorithm::BTree {
                            max_keys_per_node: 256,
                        },
                        name: Some("identifier".to_string()),
                        is_primary: true,
                        selectivity_threshold: None,
                    },
                    IndexSpecification {
                        data_type: Data::DenseVector { dimension: 768 },
                        algorithm: IndexAlgorithm::HNSW {
                            m: 16,
                            ef_construction: 200,
                            ef_search: 50,
                            max_elements: 1000000,
                        },
                        name: Some("vector".to_string()),
                        is_primary: false,
                        selectivity_threshold: None,
                    },
                    IndexSpecification {
                        data_type: Data::Metadata,
                        algorithm: IndexAlgorithm::BTree {
                            max_keys_per_node: 256,
                        },
                        name: Some("metadata".to_string()),
                        is_primary: false,
                        selectivity_threshold: None,
                    },
                ],
            })
        }
    }

    /// Test struct for index strategy
    #[derive(Debug)]
    struct IndexStrategy {
        indexes: Vec<crate::index::axis::types::IndexSpecification>,
    }

    #[test]
    fn test_strategy_builder_small_collection() {
        let stats = CollectionStatistics {
            total_vectors: 10_000,
            vector_dimension: 128,
            avg_vector_sparsity: 0.1,
            has_metadata: true,
            metadata_cardinality: vec![("category".to_string(), 10), ("price".to_string(), 1000)]
                .into_iter()
                .collect(),
            has_text_fields: false,
            update_frequency: 1.0,
        };

        let patterns = QueryPatterns {
            avg_queries_per_second: 100.0,
            filter_usage_ratio: 0.3,
            text_search_ratio: 0.0,
            typical_k: 10,
            recall_requirement: 0.95,
        };

        let strategy = IndexStrategyBuilder::new(stats, patterns).build().unwrap();

        // Should have identifier, vector, and metadata indexes
        assert!(strategy.indexes.len() >= 3);

        // Vector index should be HNSW for small collection
        let vector_index = strategy
            .indexes
            .iter()
            .find(|idx| matches!(idx.data_type, Data::DenseVector { .. }))
            .unwrap();
        assert!(matches!(
            vector_index.algorithm,
            IndexAlgorithm::HNSW { .. }
        ));
    }
}
