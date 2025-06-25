//! Index type definitions

use serde::{Deserialize, Serialize};

/// General index type categories
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum IndexType {
    /// Vector similarity index
    Vector(VectorIndexType),
    /// Metadata filter index
    Metadata(MetadataIndexType),
    /// Full-text search index
    FullText,
    /// Composite index combining multiple types
    Composite,
}

/// Vector index algorithm types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum VectorIndexType {
    /// Hierarchical Navigable Small World
    HNSW {
        m: u32,
        ef_construction: u32,
        ef_search: u32,
    },
    /// Inverted File Index
    IVF {
        nlist: u32,
        nprobe: u32,
    },
    /// Product Quantization
    PQ {
        m: u32,
        nbits: u32,
    },
    /// Flat (exhaustive) search
    Flat,
    /// Locality Sensitive Hashing
    LSH {
        num_tables: u32,
        num_bits: u32,
    },
    /// Annoy index
    Annoy {
        num_trees: u32,
    },
    /// Full-text search
    FullText,
}

/// Metadata index types for efficient filtering
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MetadataIndexType {
    /// Hash index for equality queries
    Hash,
    /// B-tree for range queries
    BTree,
    /// Bitmap index for low-cardinality fields
    Bitmap,
    /// Inverted index for text search
    Inverted,
}