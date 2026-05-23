//! AXIS Type System - Clean separation of data types and indexing algorithms

use serde::{Deserialize, Serialize};

/// Cluster assignment for a vector
#[derive(Debug, Clone)]
pub struct ClusterAssignment {
    /// Unique identifier of the assigned vector.
    pub vector_id: u32,
    /// Identifier of the cluster this vector belongs to.
    pub cluster_id: u32,
    /// Similarity score between the vector and its cluster centroid.
    pub similarity: f32,
}

/// What kind of data are we indexing?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Data {
    /// Dense vectors (fixed dimension, most elements non-zero).
    DenseVector {
        /// Dimensionality of the dense vector space.
        dimension: usize,
    },

    /// Sparse vectors (many zero elements, variable dimension).
    SparseVector {
        /// Maximum dimension of the sparse vector space.
        max_dimension: usize,
    },

    /// Metadata fields for filtering
    Metadata,

    /// Full-text search data
    FullText,

    /// Unique identifiers
    Identifier,
}

/// How do we index the data?
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IndexAlgorithm {
    /// Hierarchical Navigable Small World - for dense vectors.
    HNSW {
        /// Number of bi-directional links per node (M parameter).
        m: u32,
        /// Size of the dynamic candidate list during construction.
        ef_construction: u32,
        /// Size of the dynamic candidate list during search.
        ef_search: u32,
        /// Maximum number of elements the index can hold.
        max_elements: usize,
    },

    /// Inverted File Index - for dense vectors with clustering.
    IVF {
        /// Number of Voronoi cells (clusters) to partition the space.
        nlist: u32,
        /// Number of clusters to probe during search.
        nprobe: u32,
        /// Optional sub-quantizer for compressed vector storage.
        quantizer: Option<Box<IndexAlgorithm>>,
    },

    /// Product Quantization - for compressed dense vectors.
    PQ {
        /// Number of subquantizers (splits the vector into m subspaces).
        m: u32,
        /// Number of bits per subquantizer code.
        nbits: u32,
        /// Number of training vectors for codebook construction.
        train_size: usize,
    },

    /// Locality Sensitive Hashing - for approximate search.
    LSH {
        /// Number of hash functions (projections) per table.
        n_projections: u32,
        /// Number of independent hash tables.
        n_hash_tables: u32,
        /// Width of hash buckets for quantizing projections.
        hash_width: f32,
    },

    /// BTree - for exact metadata indexing.
    BTree {
        /// Maximum number of keys per B-tree node.
        max_keys_per_node: usize,
    },

    /// Inverted Index - for full-text search.
    InvertedIndex {
        /// Text analysis pipeline configuration.
        analyzer: TextAnalyzer,
        /// Whether to store term positions for phrase queries.
        enable_positions: bool,
    },

    /// Skip List - for sorted data.
    SkipList {
        /// Maximum level of the skip list hierarchy.
        max_level: u32,
        /// Probability of promoting a node to the next level.
        probability: f32,
    },

    /// Bloom Filter - for membership testing.
    BloomFilter {
        /// Expected number of elements for sizing the filter.
        expected_elements: usize,
        /// Target false positive rate (e.g. 0.01 for 1%).
        false_positive_rate: f64,
    },

    /// Annoy (Approximate Nearest Neighbors Oh Yeah) - for fast tree-based search.
    Annoy {
        /// Number of random projection trees to build.
        n_trees: u32,
        /// Number of nodes to inspect during search (-1 for auto).
        search_k: i32,
        /// Maximum number of descendants in a leaf node.
        max_leaf_size: u32,
    },

    /// Enhanced Dense Retrieval - late interaction with query and document expansion.
    EDR {
        /// Number of query vectors for expansion.
        num_query_expansions: usize,
        /// Number of document vectors per document.
        num_document_vectors: usize,
        /// Maximum number of results to return.
        top_k: usize,
        /// Whether to use query expansion.
        enable_query_expansion: bool,
        /// Whether to use document expansion.
        enable_document_expansion: bool,
    },

    /// Global ID Index - for O(1) vector ID to storage location mapping.
    GlobalId {
        /// Maximum number of entries to keep in the LRU cache.
        cache_size: usize,
        /// Whether to persist the index to disk for crash recovery.
        persistence_enabled: bool,
    },
}

/// Text analysis configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextAnalyzer {
    /// Tokenizer that splits text into tokens.
    pub tokenizer: Tokenizer,
    /// Ordered list of filters applied to each token.
    pub filters: Vec<TokenFilter>,
    /// Optional language code for language-specific analysis.
    pub language: Option<String>,
}

/// Tokenizer strategy for splitting text into tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Tokenizer {
    /// Split tokens on whitespace boundaries only.
    Whitespace,
    /// Standard Unicode-aware tokenizer with punctuation handling.
    Standard,
    /// Character n-gram tokenizer with configurable window size.
    NGram {
        /// Minimum n-gram length.
        min: usize,
        /// Maximum n-gram length.
        max: usize,
    },
}

/// Token filter applied after tokenization for normalization and enrichment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TokenFilter {
    /// Convert all tokens to lowercase.
    Lowercase,
    /// Remove common stopwords from the token stream.
    Stopwords,
    /// Apply stemming to reduce tokens to their root form.
    Stemmer,
    /// Expand tokens using a synonym mapping table.
    Synonyms(Vec<(String, String)>),
}

/// Complete index specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexSpecification {
    /// What data are we indexing
    pub data_type: Data,

    /// How are we indexing it
    pub algorithm: IndexAlgorithm,

    /// Optional name for this index
    pub name: Option<String>,

    /// Is this the primary index for queries
    pub is_primary: bool,

    /// When to use this index (query selectivity threshold)
    pub selectivity_threshold: Option<f32>,
}

impl IndexSpecification {
    /// Create a new index specification
    pub fn new(data_type: Data, algorithm: IndexAlgorithm) -> Self {
        Self {
            data_type,
            algorithm,
            name: None,
            is_primary: false,
            selectivity_threshold: None,
        }
    }

    /// Check if this index supports clustering optimization
    pub fn supports_clustering(&self) -> bool {
        matches!(
            self.data_type,
            Data::DenseVector { .. } | Data::SparseVector { .. }
        ) && matches!(
            self.algorithm,
            IndexAlgorithm::HNSW { .. }
                | IndexAlgorithm::IVF { .. }
                | IndexAlgorithm::PQ { .. }
                | IndexAlgorithm::LSH { .. }
                | IndexAlgorithm::Annoy { .. }
                | IndexAlgorithm::EDR { .. }
        )
    }

    /// Check if this index supports vector search
    pub fn supports_vector_search(&self) -> bool {
        matches!(
            self.data_type,
            Data::DenseVector { .. } | Data::SparseVector { .. }
        )
    }

    /// Check if this index supports metadata filtering
    pub fn supports_filtering(&self) -> bool {
        matches!(self.data_type, Data::Metadata)
            || matches!(
                self.algorithm,
                IndexAlgorithm::BTree { .. } | IndexAlgorithm::SkipList { .. }
            )
    }

    /// Check if this index supports full-text search
    pub fn supports_text_search(&self) -> bool {
        matches!(self.data_type, Data::FullText)
            && matches!(self.algorithm, IndexAlgorithm::InvertedIndex { .. })
    }
}

/// Index selection strategy based on query characteristics
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexSelectionStrategy {
    /// Available indexes
    pub indexes: Vec<IndexSpecification>,

    /// Query routing rules
    pub routing_rules: Vec<RoutingRule>,
}

/// Rule mapping query characteristics to the indexes that should service them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Condition for using this rule
    pub condition: QueryCondition,

    /// Which indexes to use
    pub use_indexes: Vec<usize>, // indices into indexes vec

    /// How to combine results
    pub combination: ResultCombination,
}

/// Predicate for conditional index routing based on query properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QueryCondition {
    /// Always apply this rule
    Always,

    /// Vector dimension matches
    VectorDimension(usize),

    /// Query has metadata filters
    HasFilters,

    /// Query selectivity above threshold
    SelectivityAbove(f32),

    /// Text query present
    HasTextQuery,

    /// Logical AND of multiple conditions.
    And(Vec<QueryCondition>),
    /// Logical OR of multiple conditions.
    Or(Vec<QueryCondition>),
    /// Logical negation of a condition.
    Not(Box<QueryCondition>),
}

/// Strategy for combining results from multiple indexes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResultCombination {
    /// Use first index results only
    First,

    /// Merge results by score
    MergeByScore,

    /// Intersect results (AND)
    Intersect,

    /// Union results (OR)
    Union,

    /// Re-rank first index results using second
    Rerank,
}

/// AXIS configuration for the entire system
#[derive(Debug, Clone)]
pub struct AxisConfig {
    /// Maximum memory usage in bytes
    pub max_memory_bytes: u64,

    /// Default index algorithm to use
    pub default_algorithm: IndexAlgorithm,

    /// Auto-migration enabled
    pub enable_auto_migration: bool,

    /// Monitoring configuration
    pub monitoring_enabled: bool,

    /// Performance thresholds
    pub performance_thresholds: PerformanceThresholds,

    /// Strategy configuration
    pub strategy_config: StrategyConfig,

    /// Migration configuration
    pub migration_config: AxisMigrationConfig,

    /// Monitoring configuration
    pub monitoring_config: AxisMonitoringConfig,
}

/// Performance thresholds for monitoring
#[derive(Debug, Clone)]
pub struct PerformanceThresholds {
    /// Maximum acceptable query latency in milliseconds.
    pub max_latency_ms: u64,
    /// Minimum acceptable recall rate (0.0 to 1.0).
    pub min_recall: f32,
    /// Maximum acceptable memory usage ratio (0.0 to 1.0).
    pub max_memory_usage: f64,
}

/// Strategy configuration
#[derive(Debug, Clone)]
pub struct StrategyConfig {
    /// Whether to use ML models for adaptive index selection.
    pub use_ml_models: bool,
    /// Minimum dataset size before ML-based strategy kicks in.
    pub min_training_size: usize,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            use_ml_models: false,
            min_training_size: 10000,
        }
    }
}

/// Backwards-compat alias for [`AxisMigrationConfig`].
pub type MigrationConfig = AxisMigrationConfig;

/// Migration configuration
#[derive(Debug, Clone)]
pub struct AxisMigrationConfig {
    /// Minimum performance improvement ratio to trigger a migration.
    pub improvement_threshold: f32,
    /// Maximum number of index migrations running simultaneously.
    pub max_concurrent_migrations: usize,
}

impl Default for AxisMigrationConfig {
    fn default() -> Self {
        Self {
            improvement_threshold: 0.1,
            max_concurrent_migrations: 2,
        }
    }
}

impl Default for AxisConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 1024 * 1024 * 1024, // 1GB
            default_algorithm: IndexAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
                max_elements: 1000000,
            },
            enable_auto_migration: true,
            monitoring_enabled: true,
            performance_thresholds: PerformanceThresholds {
                max_latency_ms: 100,
                min_recall: 0.9,
                max_memory_usage: 0.8,
            },
            strategy_config: StrategyConfig::default(),
            migration_config: AxisMigrationConfig::default(),
            monitoring_config: AxisMonitoringConfig::default(),
        }
    }
}

/// Migration decision for index transitions
#[derive(Debug, Clone)]
pub struct MigrationDecision {
    /// Current index algorithm being migrated from.
    pub from_algorithm: IndexAlgorithm,
    /// Target index algorithm to migrate to.
    pub to_algorithm: IndexAlgorithm,
    /// Reason triggering this migration decision.
    pub reason: MigrationReason,
    /// Estimated computational cost of the migration.
    pub estimated_cost: f64,
    /// Priority level for scheduling the migration.
    pub priority: MigrationPriority,
}

/// Reasons for migration
#[derive(Debug, Clone)]
pub enum MigrationReason {
    /// Query latency or throughput degradation.
    Performance,
    /// Memory usage exceeding configured thresholds.
    Memory,
    /// Dataset size growth requiring a different index structure.
    DataGrowth,
    /// Shift in query patterns favoring a different algorithm.
    QueryPatternChange,
}

/// Migration priority levels
#[derive(Debug, Clone)]
pub enum MigrationPriority {
    /// Background migration with no urgency.
    Low,
    /// Standard priority for routine optimizations.
    Medium,
    /// Elevated priority due to noticeable performance impact.
    High,
    /// Immediate migration required to restore service quality.
    Critical,
}

/// Alert threshold configuration
#[derive(Debug, Clone)]
pub struct AlertThresholds {
    /// Warning threshold for query latency in milliseconds.
    pub latency_ms: u64,
    /// Warning threshold for memory usage ratio (0.0 to 1.0).
    pub memory_usage: f64,
    /// Warning threshold for error rate ratio (0.0 to 1.0).
    pub error_rate: f64,
    /// Critical alert threshold for query latency in milliseconds.
    pub max_query_latency_ms: u64,
    /// Minimum acceptable query throughput (queries per second).
    pub min_query_throughput: f64,
    /// Critical alert threshold for error rate ratio.
    pub max_error_rate: f64,
}

/// Backwards-compat alias for [`AxisMonitoringConfig`].
pub type MonitoringConfig = AxisMonitoringConfig;

/// Monitoring configuration
#[derive(Debug, Clone)]
pub struct AxisMonitoringConfig {
    /// Whether monitoring is active.
    pub enabled: bool,
    /// Interval in seconds between health checks.
    pub interval_seconds: u64,
    /// Interval in seconds between metrics collection cycles.
    pub metrics_interval_seconds: u64,
    /// Warning-level thresholds for monitored metrics.
    pub thresholds: AlertThresholds,
    /// Critical-level thresholds that trigger alerts.
    pub alert_thresholds: AlertThresholds,
}

impl Default for AxisMonitoringConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 60,
            metrics_interval_seconds: 60,
            thresholds: AlertThresholds {
                latency_ms: 100,
                memory_usage: 0.8,
                error_rate: 0.05,
                max_query_latency_ms: 100,
                min_query_throughput: 10.0,
                max_error_rate: 0.05,
            },
            alert_thresholds: AlertThresholds {
                latency_ms: 100,
                memory_usage: 0.8,
                error_rate: 0.05,
                max_query_latency_ms: 100,
                min_query_throughput: 10.0,
                max_error_rate: 0.05,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_capabilities() {
        let hnsw_index = IndexSpecification::new(
            Data::DenseVector { dimension: 128 },
            IndexAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
                max_elements: 1_000_000,
            },
        );

        assert!(hnsw_index.supports_clustering());
        assert!(hnsw_index.supports_vector_search());
        assert!(!hnsw_index.supports_filtering());
        assert!(!hnsw_index.supports_text_search());
    }

    #[test]
    fn test_metadata_index() {
        let btree_index = IndexSpecification::new(
            Data::Metadata,
            IndexAlgorithm::BTree {
                max_keys_per_node: 100,
            },
        );

        assert!(!btree_index.supports_clustering());
        assert!(!btree_index.supports_vector_search());
        assert!(btree_index.supports_filtering());
        assert!(!btree_index.supports_text_search());
    }
}
