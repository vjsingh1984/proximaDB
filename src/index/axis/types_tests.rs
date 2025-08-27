//! Unit tests for AXIS Type System

use super::types::*;

#[test]
fn test_data_type_creation() {
    let dense_vector = Data::DenseVector { dimension: 128 };
    assert!(matches!(dense_vector, Data::DenseVector { dimension: 128 }));

    let sparse_vector = Data::SparseVector { max_dimension: 1000 };
    assert!(matches!(sparse_vector, Data::SparseVector { max_dimension: 1000 }));

    let metadata = Data::Metadata;
    assert!(matches!(metadata, Data::Metadata));

    let full_text = Data::FullText;
    assert!(matches!(full_text, Data::FullText));

    let identifier = Data::Identifier;
    assert!(matches!(identifier, Data::Identifier));
}

#[test]
fn test_index_algorithm_creation() {
    let hnsw = IndexAlgorithm::HNSW {
        m: 16,
        ef_construction: 200,
        ef_search: 50,
        max_elements: 1_000_000,
    };
    assert!(matches!(hnsw, IndexAlgorithm::HNSW { m: 16, .. }));

    let ivf = IndexAlgorithm::IVF {
        nlist: 100,
        nprobe: 10,
        quantizer: Some(Box::new(IndexAlgorithm::PQ {
            m: 8,
            nbits: 8,
            train_size: 100_000,
        })),
    };
    assert!(matches!(ivf, IndexAlgorithm::IVF { nlist: 100, .. }));

    let pq = IndexAlgorithm::PQ {
        m: 8,
        nbits: 8,
        train_size: 100_000,
    };
    assert!(matches!(pq, IndexAlgorithm::PQ { m: 8, .. }));

    let btree = IndexAlgorithm::BTree {
        max_keys_per_node: 256,
    };
    assert!(matches!(btree, IndexAlgorithm::BTree { max_keys_per_node: 256 }));

    let bloom = IndexAlgorithm::BloomFilter {
        expected_elements: 1_000_000,
        false_positive_rate: 0.01,
    };
    assert!(matches!(bloom, IndexAlgorithm::BloomFilter { expected_elements: 1_000_000, .. }));
}

#[test]
fn test_index_specification_creation() {
    let spec = IndexSpecification {
        // data_type removed -  Data::DenseVector { dimension: 128 },
        algorithm: IndexAlgorithm::HNSW {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            max_elements: 1_000_000,
        },
        name: Some("vector_index".to_string()),
        is_primary: true,
        selectivity_threshold: Some(0.1),
    };

    assert!(spec.is_primary);
    assert_eq!(spec.name, Some("vector_index".to_string()));
    assert_eq!(spec.selectivity_threshold, Some(0.1));
    assert!(matches!(spec.data_type, Data::DenseVector { .. }));
    assert!(matches!(spec.algorithm, IndexAlgorithm::HNSW { .. }));
}

#[test]
fn test_serialization_deserialization() {
    let spec = IndexSpecification {
        // data_type removed -  Data::DenseVector { dimension: 128 },
        algorithm: IndexAlgorithm::HNSW {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            max_elements: 1_000_000,
        },
        name: Some("vector_index".to_string()),
        is_primary: true,
        selectivity_threshold: Some(0.1),
    };

    let serialized = serde_json::to_string(&spec).unwrap();
    assert!(serialized.contains_hash("DenseVector"));
    assert!(serialized.contains_hash("HNSW"));

    let deserialized: IndexSpecification = serde_json::from_str(&serialized).unwrap();
    assert!(matches!(deserialized.data_type, Data::DenseVector { .. }));
    assert!(matches!(deserialized.algorithm, IndexAlgorithm::HNSW { .. }));
    assert_eq!(deserialized.name, Some("vector_index".to_string()));
    assert!(deserialized.is_primary);
}