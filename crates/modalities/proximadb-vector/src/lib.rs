//! # ProximaDB Vector Modality
//!
//! This crate contains vector search engines and indexing, quantization, ANN, scoring,
//! and vector operations services for the ProximaDB vector database.
//!
//! ## Architecture
//!
//! The vector modality is organized into several key modules:
//!
//! - **`distance`** - Distance metrics (L2, Cosine, Dot Product, Manhattan) with SIMD acceleration
//! - **`quantization`** - Vector quantization (Scalar, Product, Binary)
//! - **`index`** - Vector indexing algorithms (HNSW, IVF, PQ, Annoy, LSH)
//! - **`search`** - Vector similarity search and ANN algorithms
//! - **`service`** - Vector service implementing VectorQueryService trait (Phase 3)
//!
//! ## Foundation
//!
//! This crate serves as the foundation for vector operations across ProximaDB,
//! providing reusable contracts and implementations for:
//!
//! - Storage engines that need vector similarity search
//! - Query executors that need vector operations
//! - Index builders that need quantization and distance metrics
//!
//! ## Phase 3: Modality Runtime Extraction
//!
//! As of Phase 3, this crate provides a root-independent vector query runtime through the `service` module:
//! - `VectorServiceImpl` implements the stable `VectorQueryService` trait
//! - Scores caller-supplied canonical `ProximaRecord` values without fabricating results
//! - Keeps the server production path on `VectorOperationsService` until AXIS is exposed behind a narrow contract
//! - Supports gradual migration through trait object injection without creating a separate durable vector store
//!
//! ## Dependencies
//!
//! - `proximadb-kernel` - Core error types and foundational contracts
//! - `proximadb-proto` - Protocol buffer types for VectorRecord
//! - `proximadb-query-filter` - Filter expression contracts
//! - `proximadb-vector-query` - Vector query service contract
//! - `arrow` - Columnar data structures for vector operations

pub mod distance;
pub mod index;
pub mod quantization;
pub mod search;
pub mod service;

// Re-export common types for convenience
pub use distance::{
    DistanceComputeProvider, DistanceMetric, DistanceMode, MetricProperties, SimilarityResult,
    UnifiedDistanceCompute, cosine_distance, dot_product, euclidean_distance, manhattan_distance,
};

pub use quantization::{
    QuantizationConfig, QuantizationLevel, QuantizationType, QuantizedVectorData,
};

pub use index::{
    FlatIndex, IndexConfig, IndexError, IndexParameters, IndexStats, IndexType, Neighbor,
    VectorIndex,
};

pub use search::{
    BruteForceSearch, SearchConfig, SearchError, SearchParams, SearchParams as VectorSearchParams,
    SearchStats, VectorSearchEngine,
};

// Re-export Phase 3 service implementation
pub use service::VectorServiceImpl;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_module_imports() {
        let _metric = DistanceMetric::Euclidean;
        let _engine = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    }

    #[test]
    fn test_distance_calculations() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];

        let euclidean = euclidean_distance(&a, &b);
        assert!((euclidean - 5.196).abs() < 0.01);

        let cosine = cosine_distance(&a, &a);
        assert!((cosine - 0.0).abs() < 1e-6);

        let dot = dot_product(&a, &b);
        assert_eq!(dot, 32.0);

        let manhattan = manhattan_distance(&a, &b);
        assert_eq!(manhattan, 9.0);
    }

    #[test]
    fn test_engine() {
        let engine = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];

        let result = engine.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
        assert!(result.raw_distance > 0.0);
    }
}
