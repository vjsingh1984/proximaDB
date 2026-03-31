//! Index type definitions

/// General index type categories
#[derive(Debug, Clone)]
pub enum Index {
    /// Vector similarity index
    Vector(VectorIndex),
    /// Metadata filter index
    Metadata(MetadataIndex),
    /// Full-text search index
    FullText,
    /// Composite index combining multiple types
    Composite,
}

/// Vector index algorithm types
#[derive(Debug, Clone)]
pub enum VectorIndex {
    /// Hierarchical Navigable Small World
    HNSW {
        /// Maximum number of connections per node per layer
        m: u32,
        /// Size of the dynamic candidate list during index construction
        ef_construction: u32,
        /// Size of the dynamic candidate list during search
        ef_search: u32,
    },
    /// Inverted File Index
    IVF {
        /// Number of Voronoi cells (clusters)
        nlist: u32,
        /// Number of cells to probe during search
        nprobe: u32,
    },
    /// Product Quantization
    PQ {
        /// Number of sub-quantizers
        m: u32,
        /// Bits per sub-quantizer code
        nbits: u32,
    },
    /// Flat (exhaustive) search
    Flat,
    /// Locality Sensitive Hashing
    LSH {
        /// Number of hash tables
        num_tables: u32,
        /// Number of hash bits per table
        num_bits: u32,
    },
    /// Annoy index
    Annoy {
        /// Number of random projection trees
        num_trees: u32,
    },
    /// Full-text search
    FullText,
}

/// Metadata index types for efficient filtering
#[derive(Debug, Clone)]
pub enum MetadataIndex {
    /// Hash index for equality queries
    Hash,
    /// B-tree for range queries
    BTree,
    /// Bitmap index for low-cardinality fields
    Bitmap,
    /// Inverted index for text search
    Inverted,
}
