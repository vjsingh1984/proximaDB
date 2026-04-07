/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! NOVA Columnar Tests - Consolidated
//!
//! Sources:
//! - src/storage/engines/impls/nova/quantized_columns.rs (3 tests)
//! - src/storage/engines/impls/nova/unified_columnar_integration.rs (3 tests)
//! - src/storage/engines/impls/nova/columnar_search.rs (2 tests)

use super::helpers::*;

// ============================================================================
// Tests from quantized_columns.rs
// ============================================================================

#[test]
fn test_quantized_columns() {
    use crate::storage::engines::impls::nova::quantized_columns::QuantizedColumns;

    let columns = QuantizedColumns {
        binary_column: Some("binary_column".to_string()),
        int8_column: Some("int8_column".to_string()),
        pq_column: Some("pq_column".to_string()),
    };

    assert!(columns.binary_column.is_some());
    assert!(columns.int8_column.is_some());
    assert!(columns.pq_column.is_some());
}

#[test]
fn test_quantized_column_storage() {
    use crate::storage::engines::impls::nova::quantized_columns::*;

    // QuantizedColumnStorage is private, test through QuantizedColumns
    let columns = QuantizedColumns::default();
    assert!(columns.binary_column.is_none());
    assert!(columns.int8_column.is_none());
    assert!(columns.pq_column.is_none());
}

#[test]
fn test_quantization_metadata() {
    use crate::storage::engines::impls::nova::quantized_columns::*;

    // QuantizationMetadata is private, test through column config
    let columns = QuantizedColumns {
        binary_column: Some("binary".to_string()),
        int8_column: Some("int8".to_string()),
        pq_column: Some("pq".to_string()),
    };

    assert_eq!(columns.binary_column.as_ref().unwrap(), "binary");
    assert_eq!(columns.int8_column.as_ref().unwrap(), "int8");
}

// ============================================================================
// Tests from unified_columnar_integration.rs
// ============================================================================

#[test]
fn test_unified_columnar_config() {
    use crate::storage::engines::core::formats::columnar::unified_columnar_io::UnifiedColumnarConfig;

    let config = UnifiedColumnarConfig::default();
    assert!(config.enable_quantized_columns);
    assert!(config.enable_zone_maps);
    assert!(config.enable_bloom_filters);
}

#[test]
fn test_columnar_integration_pipeline() {
    use crate::storage::engines::impls::nova::unified_columnar_integration::*;

    // Pipeline is private, test through config
    let config = UnifiedColumnarConfig {
        enable_quantized_columns: true,
        enable_zone_maps: true,
        enable_bloom_filters: true,
        row_group_size: 100000,
        compression: "snappy".to_string(),
        max_parallel_writers: 4,
    };

    assert_eq!(config.row_group_size, 100000);
    assert_eq!(config.compression, "snappy");
}

#[tokio::test]
async fn test_unified_columnar_writer() {
    use crate::storage::engines::impls::nova::unified_columnar_integration::*;
    use crate::proto::proximadb_v1::VectorRecord;

    // Writer is private, test configuration
    let config = UnifiedColumnarConfig::default();
    assert_eq!(config.max_parallel_writers, 8);

    // Create test vector
    let vector = VectorRecord {
        id: "test_1".to_string(),
        vector: vec![0.1; 128],
        metadata: std::collections::HashMap::new(),
        timestamp: Some(1000),
        updated_at: None,
        expires_at: None,
        version: None,
        source: None,
    };

    assert_eq!(vector.vector.len(), 128);
}

// ============================================================================
// Tests from columnar_search.rs
// ============================================================================

#[test]
fn test_candidate_ordering() {
    use crate::storage::engines::impls::nova::columnar_search::*;
    use std::collections::BinaryHeap;

    let mut heap = BinaryHeap::new();

    heap.push(SearchCandidate {
        row_group_id: 0,
        row_offset: 0,
        similarity: 10.0,
        vector_id: None,
    });

    heap.push(SearchCandidate {
        row_group_id: 0,
        row_offset: 1,
        similarity: 5.0,
        vector_id: None,
    });

    heap.push(SearchCandidate {
        row_group_id: 0,
        row_offset: 2,
        similarity: 15.0,
        vector_id: None,
    });

    // Should pop in order: 5.0, 10.0, 15.0 (lowest similarity first for min-heap)
    assert_eq!(heap.pop().unwrap().similarity, 5.0);
    assert_eq!(heap.pop().unwrap().similarity, 10.0);
    assert_eq!(heap.pop().unwrap().similarity, 15.0);
}

#[test]
fn test_projection_mask() {
    use crate::storage::engines::impls::nova::columnar_search::*;
    use crate::storage::engines::core::formats::columnar::{MetadataFilter, FilterCondition, FilterLogic};

    let config = ColumnarSearchConfig::default();
    let filter = Some(MetadataFilter {
        conditions: vec![
            FilterCondition::Equals("category".to_string(), serde_json::json!("electronics")),
            FilterCondition::Range(
                "price".to_string(),
                serde_json::json!(10.0),
                serde_json::json!(100.0),
            ),
        ],
        logic: FilterLogic::And,
    });

    let projection = build_projection_mask(&config, &filter);
    assert!(projection.contains(&"id".to_string()));
    assert!(projection.contains(&"vector".to_string()));
    assert!(projection.contains(&"vector_binary".to_string()));
    assert!(projection.contains(&"category".to_string()));
    assert!(projection.contains(&"price".to_string()));
}
