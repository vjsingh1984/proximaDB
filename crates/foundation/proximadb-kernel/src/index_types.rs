//! Index type definitions shared across storage and query layers.

/// General index type categories
#[derive(Debug, Clone)]
pub enum Index {
    Vector(VectorIndex),
    Metadata(MetadataIndex),
    FullText,
    Composite,
}

/// Vector index algorithm types
#[derive(Debug, Clone)]
pub enum VectorIndex {
    HNSW {
        m: u32,
        ef_construction: u32,
        ef_search: u32,
    },
    IVF {
        nlist: u32,
        nprobe: u32,
    },
    PQ {
        m: u32,
        nbits: u32,
    },
    Flat,
    LSH {
        num_tables: u32,
        num_bits: u32,
    },
    Annoy {
        num_trees: u32,
    },
    FullText,
}

/// Metadata index types for efficient filtering
#[derive(Debug, Clone)]
pub enum MetadataIndex {
    Hash,
    BTree,
    Bitmap,
    Inverted,
}
