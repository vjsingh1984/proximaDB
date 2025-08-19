//! AXIS Type System - Clean separation of data types and indexing algorithms

use serde::{Deserialize, Serialize};

/// Cluster assignment for a vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterAssignment {
    pub vector_id: u32,
    pub cluster_id: u32,
    pub similarity: f32,
}

/// What kind of data are we indexing?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataType {
    /// Dense vectors (fixed dimension, most elements non-zero)
    DenseVector { dimension: usize },
    
    /// Sparse vectors (many zero elements, variable dimension)
    SparseVector { max_dimension: usize },
    
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
    /// Hierarchical Navigable Small World - for dense vectors
    HNSW {
        m: u32,                // Number of bi-directional links
        ef_construction: u32,  // Size of dynamic candidate list
        ef_search: u32,       // Size of dynamic candidate list during search
        max_elements: usize,  // Maximum number of elements
    },
    
    /// Inverted File Index - for dense vectors with clustering
    IVF {
        nlist: u32,    // Number of clusters
        nprobe: u32,   // Number of clusters to search
        quantizer: Option<Box<IndexAlgorithm>>, // Optional quantization
    },
    
    /// Product Quantization - for compressed dense vectors
    PQ {
        m: u32,        // Number of subquantizers
        nbits: u32,    // Number of bits per subquantizer
        train_size: usize, // Number of vectors for training
    },
    
    /// Locality Sensitive Hashing - for approximate search
    LSH {
        n_projections: u32,  // Number of hash functions
        n_hash_tables: u32,  // Number of hash tables
        hash_width: f32,     // Width of hash buckets
    },
    
    /// BTree - for exact metadata indexing
    BTree {
        max_keys_per_node: usize,
    },
    
    /// Inverted Index - for full-text search
    InvertedIndex {
        analyzer: TextAnalyzer,
        enable_positions: bool,
    },
    
    /// Skip List - for sorted data
    SkipList {
        max_level: u32,
        probability: f32,
    },
    
    /// Bloom Filter - for membership testing
    BloomFilter {
        expected_elements: usize,
        false_positive_rate: f64,
    },
    
    /// Annoy (Approximate Nearest Neighbors Oh Yeah) - for fast tree-based search
    Annoy {
        n_trees: u32,        // Number of trees to build
        search_k: i32,       // Number of nodes to inspect (-1 = auto)
        max_leaf_size: u32,  // Maximum number of descendants in a leaf
    },
}

/// Text analysis configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextAnalyzer {
    pub tokenizer: Tokenizer,
    pub filters: Vec<TokenFilter>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Tokenizer {
    Whitespace,
    Standard,
    NGram { min: usize, max: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TokenFilter {
    Lowercase,
    Stopwords,
    Stemmer,
    Synonyms(Vec<(String, String)>),
}

/// Complete index specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexSpecification {
    /// What data are we indexing
    pub data_type: DataType,
    
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
    pub fn new(data_type: DataType, algorithm: IndexAlgorithm) -> Self {
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
        matches!(self.data_type, DataType::DenseVector { .. } | DataType::SparseVector { .. }) &&
        matches!(self.algorithm, 
            IndexAlgorithm::HNSW { .. } | 
            IndexAlgorithm::IVF { .. } | 
            IndexAlgorithm::PQ { .. } |
            IndexAlgorithm::LSH { .. } |
            IndexAlgorithm::Annoy { .. }
        )
    }
    
    /// Check if this index supports vector search
    pub fn supports_vector_search(&self) -> bool {
        matches!(self.data_type, DataType::DenseVector { .. } | DataType::SparseVector { .. })
    }
    
    /// Check if this index supports metadata filtering
    pub fn supports_filtering(&self) -> bool {
        matches!(self.data_type, DataType::Metadata) ||
        matches!(self.algorithm, IndexAlgorithm::BTree { .. } | IndexAlgorithm::SkipList { .. })
    }
    
    /// Check if this index supports full-text search
    pub fn supports_text_search(&self) -> bool {
        matches!(self.data_type, DataType::FullText) &&
        matches!(self.algorithm, IndexAlgorithm::InvertedIndex { .. })
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Condition for using this rule
    pub condition: QueryCondition,
    
    /// Which indexes to use
    pub use_indexes: Vec<usize>, // indices into indexes vec
    
    /// How to combine results
    pub combination: ResultCombination,
}

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
    
    /// Combine conditions
    And(Vec<QueryCondition>),
    Or(Vec<QueryCondition>),
    Not(Box<QueryCondition>),
}

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

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_index_capabilities() {
        let hnsw_index = IndexSpecification::new(
            DataType::DenseVector { dimension: 128 },
            IndexAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
                max_elements: 1_000_000,
            }
        );
        
        assert!(hnsw_index.supports_clustering());
        assert!(hnsw_index.supports_vector_search());
        assert!(!hnsw_index.supports_filtering());
        assert!(!hnsw_index.supports_text_search());
    }
    
    #[test]
    fn test_metadata_index() {
        let btree_index = IndexSpecification::new(
            DataType::Metadata,
            IndexAlgorithm::BTree { max_keys_per_node: 100 }
        );
        
        assert!(!btree_index.supports_clustering());
        assert!(!btree_index.supports_vector_search());
        assert!(btree_index.supports_filtering());
        assert!(!btree_index.supports_text_search());
    }
}