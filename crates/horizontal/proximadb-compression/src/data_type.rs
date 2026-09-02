//! Compression data-type classification — hoisted from `proximadb-metrics`
//! (TD-DECOMP-81) so storage-tier compression code can name it without a
//! control-tier dependency. Semantically it belongs here, beside
//! [`CompressionAlgorithm`].

/// Type of data being compressed — selects the metrics bucket and the
/// algorithm-tuning defaults.
#[derive(Debug, Clone)]
pub enum CompressionData {
    /// Dense float vector payloads.
    Vector,
    /// Key-value / filterable metadata columns.
    Metadata,
    /// Index structures (IVF, graph links).
    Index,
    /// Bloom filter bitmaps.
    BloomFilter,
    /// Unspecified / mixed payload.
    Mixed,
}
