/*
 * Copyright 2026 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! TD-134 e2e tests for KEU metering and per-repo RBAC

use proximadb::governance::{CollectionRbacExt, check_collection_access};
use proximadb::metrics::consumption_metrics;
use proximadb_proto::proximadb_v1::{CollectionConfig, DistanceMetric, StorageEngine};
use std::sync::Arc;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn test_rbac_permitted_principals_enforcement() {
    // Create a test collection with permitted_principals
    let mut collection = CollectionConfig {
        name: "secure_collection".to_string(),
        dimension: 384,
        distance_metric: Some(DistanceMetric::Cos),
        storage_engine: Some(StorageEngine::Sst),
        ..Default::default()
    };

    // Empty permitted_principals should allow all access
    assert!(check_collection_access(&collection, "user1").is_ok());
    assert!(check_collection_access(&collection, "user2").is_ok());

    // Set permitted_principals to specific users
    collection.permitted_principals = vec!["alice".to_string(), "bob".to_string()];

    // Allowed users should pass
    assert!(check_collection_access(&collection, "alice").is_ok());
    assert!(check_collection_access(&collection, "bob").is_ok());

    // Other users should be denied
    assert!(check_collection_access(&collection, "charlie").is_err());
    assert!(check_collection_access(&collection, "eve").is_err());

    // Extension trait should work
    assert!(collection.check_principal_access("alice").is_ok());
    assert!(collection.check_principal_access("charlie").is_err());
}

#[tokio::test]
async fn test_keu_metering_records_embeddings() {
    // Test that KEU metering records embedding operations
    use proximadb::observability::io_trace;

    // Simulate an embedding operation within a trace scope
    let snap = proximadb::observability::io_trace::scope(async {
        // Record KEU units for a tenant
        consumption_metrics::record_keu_units(
            Some("tenant-test"),
            "victor",
            "bge-small-en-v1.5",
            "embed",
            1000, // input tokens
            384,  // output tokens (dimension)
        );

        // Get the snapshot
        proximadb::observability::io_trace::snapshot()
    })
    .await
    .expect("snapshot should be available");

    // Verify the trace captured the embedding operation
    assert_eq!(snap.embedding_calls, 1);
    assert_eq!(snap.embedding_input_tokens, 1000);
    assert_eq!(snap.embedding_output_tokens, 384);
    assert_eq!(snap.total_embedding_tokens(), 1384);
}

#[tokio::test]
async fn test_keu_multiple_operations_aggregate() {
    // Test that multiple embedding operations aggregate correctly
    use proximadb::observability::io_trace;

    let snap = proximadb::observability::io_trace::scope(async {
        // Record multiple embedding operations
        consumption_metrics::record_keu_units(
            Some("tenant-a"),
            "victor",
            "bge-small",
            "embed",
            500,
            384,
        );
        consumption_metrics::record_keu_units(
            Some("tenant-a"),
            "openai",
            "text-embedding-3-small",
            "embed",
            1000,
            1536,
        );
        consumption_metrics::record_keu_units(
            Some("tenant-a"),
            "victor",
            "bge-small",
            "embed",
            750,
            384,
        );

        proximadb::observability::io_trace::snapshot()
    })
    .await
    .expect("snapshot should be available");

    // Verify aggregation
    assert_eq!(snap.embedding_calls, 3);
    assert_eq!(snap.embedding_input_tokens, 2250); // 500 + 1000 + 750
    assert_eq!(snap.embedding_output_tokens, 2304); // 384 + 1536 + 384
    assert_eq!(snap.total_embedding_tokens(), 4554);
}

#[tokio::test]
async fn test_keu_zero_tokens_ignored() {
    // Test that zero-token embeddings are ignored
    use proximadb::observability::io_trace;

    let snap = proximadb::observability::io_trace::scope(async {
        consumption_metrics::record_keu_units(
            Some("tenant-test"),
            "victor",
            "bge-small",
            "embed",
            0,
            0,
        );
        consumption_metrics::record_keu_units(
            Some("tenant-test"),
            "victor",
            "bge-small",
            "embed",
            100,
            100,
        );

        proximadb::observability::io_trace::snapshot()
    })
    .await
    .expect("snapshot should be available");

    // Only the non-zero operation should be counted
    assert_eq!(snap.embedding_calls, 1);
    assert_eq!(snap.embedding_input_tokens, 100);
    assert_eq!(snap.embedding_output_tokens, 100);
}

#[test]
fn test_rbac_error_messages() {
    let mut collection = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 384,
        distance_metric: Some(DistanceMetric::Cos),
        storage_engine: Some(StorageEngine::Sst),
        ..Default::default()
    };

    collection.permitted_principals = vec!["admin".to_string()];
    collection.name = "secret_data".to_string();

    let err = check_collection_access(&collection, "unauthorized_user").unwrap_err();

    // Error should contain collection and principal information
    let err_msg = format!("{}", err);
    assert!(err_msg.contains("secret_data"));
    assert!(err_msg.contains("unauthorized_user"));
}
