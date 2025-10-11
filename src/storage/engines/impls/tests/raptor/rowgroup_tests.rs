/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! RAPTOR Row Group Tests - Consolidated
//!
//! This module contains all tests related to RAPTOR's smart row group sizing.
//! These tests verify the intelligent sizing algorithms that optimize row group
//! sizes based on vector dimensions, cloud I/O profiles, and query patterns.
//!
//! Sources:
//! - src/storage/engines/impls/raptor/smart_rowgroup_sizing.rs (4 tests)
//!
//! Total: 4 tests consolidated

use super::helpers::*;
use anyhow::Result;

use crate::storage::engines::impls::raptor::smart_rowgroup_sizing::*;

// ============================================================================
// Smart Row Group Sizing Tests (from smart_rowgroup_sizing.rs)
// ============================================================================

#[test]
fn test_openai_s3_sizing() {
    let sizer = CommonConfigurations::openai_s3();
    let result = sizer.calculate_optimal_rowgroup_size().unwrap();

    // Should be reasonable for S3 2MB I/O
    assert!(result.vectors_per_rowgroup >= 100);
    assert!(result.vectors_per_rowgroup <= 10000);
    assert!(result.total_rowgroup_bytes <= 4 * 1024 * 1024); // Max 4MB

    println!(
        "OpenAI/S3: {} vectors, {:.1}MB, efficiency: {:.2}",
        result.vectors_per_rowgroup,
        result.total_rowgroup_bytes as f32 / (1024.0 * 1024.0),
        result.io_efficiency_ratio
    );
}

#[test]
fn test_bert_gcs_sizing() {
    let sizer = CommonConfigurations::bert_gcs();
    let result = sizer.calculate_optimal_rowgroup_size().unwrap();

    // GCS prefers 4MB chunks
    assert!(result.total_rowgroup_bytes <= 6 * 1024 * 1024); // Max 6MB

    println!(
        "BERT/GCS: {} vectors, {:.1}MB, efficiency: {:.2}",
        result.vectors_per_rowgroup,
        result.total_rowgroup_bytes as f32 / (1024.0 * 1024.0),
        result.io_efficiency_ratio
    );
}

#[test]
fn test_dimension_scaling() {
    // Test how row group size scales with vector dimension
    println!("Testing semantic accuracy factor across dimensions:");
    for dimension in [128, 384, 768, 1536, 2048, 4096] {
        let sizer = SmartRowGroupSizer::for_s3_standard(dimension, 100);
        let result = sizer.calculate_optimal_rowgroup_size().unwrap();

        println!(
            "Dim {}: {} vectors, {:.1}KB per vector (semantic factor disabled - private method)",
            dimension,
            result.vectors_per_rowgroup,
            result.bytes_per_vector as f32 / 1024.0,
            // sizer.calculate_semantic_accuracy_factor() // Private method - disabled
        );
    }
}

#[test]
fn test_semantic_accuracy_rationale() {
    let high_dim_sizer = SmartRowGroupSizer::for_s3_standard(1536, 200); // OpenAI
    let low_dim_sizer = SmartRowGroupSizer::for_s3_standard(128, 100); // Word2Vec

    let high_result = high_dim_sizer.calculate_optimal_rowgroup_size().unwrap();
    let low_result = low_dim_sizer.calculate_optimal_rowgroup_size().unwrap();

    println!("High-dim (1536d): {}", high_result.rationale);
    println!("Low-dim (128d): {}", low_result.rationale);

    // High dimensional vectors should have smaller row groups for better precision
    assert!(
        high_result.vectors_per_rowgroup < low_result.vectors_per_rowgroup,
        "High-dim vectors should have smaller row groups for better semantic precision"
    );
}
