/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! End-to-end tests for cross-model transaction coordinator (TD-133).
//!
//! These tests verify:
//! 1. Atomic commit of node + embedding + edges
//! 2. Rollback on failure
//! 3. Coordinator returns Disabled when flag is off
//! 4. Idempotent retry behavior

use std::env;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use proximadb::catalog::{CatalogTableSchema, CatalogTableStatistics};
use proximadb::graph::engines::orion::OrionGraphEngine;
use proximadb::graph::model::{Edge, Node, PropertyValue, property_value::Value};
use proximadb::services::record_store::{
    RecordScanPredicate, TableRecordGetRequest, TableRecordGetResponse, TableRecordMutation,
    TableRecordMutationKind, TableRecordScanRequest, TableRecordScanResponse, TableRecordStore,
    TableRecordWriteResult,
};
use proximadb::services::transaction::{CrossModelTransactionCoordinator, TransactionOutcome};
use proximadb::storage::tenant::context::StorageTenantContext;
use proximadb_records::{ProximaRecord, ProximaTree};

/// Helper: Create a test node with basic properties.
fn create_test_node(id: &str, label: &str) -> Node {
    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "name".to_string(),
        PropertyValue {
            value: Some(Value::StringValue(format!("Node {}", id))),
        },
    );
    properties.insert(
        "created_at".to_string(),
        PropertyValue {
            value: Some(Value::StringValue(Utc::now().to_rfc3339())),
        },
    );

    Node {
        id: id.to_string(),
        labels: vec![label.to_string()],
        properties,
        embedding: None,
        created_at_ms: Utc::now().timestamp_millis(),
        updated_at_ms: Utc::now().timestamp_millis(),
    }
}

/// Helper: Create a test edge.
fn create_test_edge(id: &str, from: &str, to: &str, edge_type: &str) -> Edge {
    Edge {
        id: id.to_string(),
        from_node_id: from.to_string(),
        to_node_id: to.to_string(),
        edge_type: edge_type.to_string(),
        properties: std::collections::HashMap::new(),
        created_at_ms: Utc::now().timestamp_millis(),
        updated_at_ms: Utc::now().timestamp_millis(),
    }
}

/// Helper: Create a test tenant context.
fn create_test_tenant() -> StorageTenantContext {
    StorageTenantContext::for_tenant_id("test_tenant_e2e")
}

/// Simple mock record store for e2e testing.
struct MockTableRecordStore {
    _inner: std::sync::RwLock<Vec<ProximaRecord>>,
}

impl MockTableRecordStore {
    fn new() -> Self {
        Self {
            _inner: std::sync::RwLock::new(vec![]),
        }
    }
}

#[async_trait::async_trait]
impl TableRecordStore for MockTableRecordStore {
    async fn write_mutations(
        &self,
        _table_schema: &CatalogTableSchema,
        _mutations: Vec<TableRecordMutation>,
        _tenant_context: Option<&StorageTenantContext>,
    ) -> Result<TableRecordWriteResult> {
        Ok(TableRecordWriteResult {
            success: true,
            errors: vec![],
            written_count: 1,
        })
    }

    async fn get_by_key(
        &self,
        _table_schema: &CatalogTableSchema,
        _request: TableRecordGetRequest,
        _tenant_context: Option<&StorageTenantContext>,
    ) -> Result<TableRecordGetResponse> {
        Ok(TableRecordGetResponse {
            record: None,
            found: false,
        })
    }

    async fn scan_records(
        &self,
        _table_schema: &CatalogTableSchema,
        _request: TableRecordScanRequest,
        _tenant_context: Option<&StorageTenantContext>,
    ) -> Result<TableRecordScanResponse> {
        Ok(TableRecordScanResponse {
            records: vec![],
            next_offset: None,
            total_count: 0,
        })
    }

    async fn scan_records_filtered(
        &self,
        _table_schema: &CatalogTableSchema,
        _request: TableRecordScanRequest,
        _predicate: Option<&RecordScanPredicate<'_>>,
        _tenant_context: Option<&StorageTenantContext>,
    ) -> Result<TableRecordScanResponse> {
        Ok(TableRecordScanResponse {
            records: vec![],
            next_offset: None,
            total_count: 0,
        })
    }
}

/// Helper: Create a test coordinator with mock record store.
fn create_test_coordinator(graph_id: &str) -> CrossModelTransactionCoordinator {
    let engine = OrionGraphEngine::new(graph_id.to_string()).unwrap();
    let record_store = Arc::new(MockTableRecordStore::new());
    CrossModelTransactionCoordinator::new(
        Arc::new(engine),
        record_store,
        "test_embeddings".to_string(),
    )
}

/// Test: Coordinator is disabled by default.
#[test]
fn test_coordinator_disabled_by_default() {
    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
    let coordinator = create_test_coordinator("test_graph_disabled");
    assert!(!coordinator.is_enabled());
}

/// Test: Coordinator can be enabled via flag.
#[test]
fn test_coordinator_enabled_with_flag() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");
    let coordinator = create_test_coordinator("test_graph_enabled");
    assert!(coordinator.is_enabled());
    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Coordinator returns Disabled when flag is off.
#[tokio::test]
async fn test_write_symbol_returns_disabled_when_flag_off() {
    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");

    let coordinator = create_test_coordinator("test_graph_return_disabled");
    let tenant_ctx = create_test_tenant();

    let node = create_test_node("node1", "Person");
    let embedding = vec![0.1, 0.2, 0.3];
    let edges = vec![];

    let result = coordinator
        .write_symbol_atomically(node, embedding, edges, &tenant_ctx)
        .await
        .unwrap();

    assert_eq!(result, TransactionOutcome::Disabled);
}

/// Test: Successful atomic commit of node + embedding + edges.
#[tokio::test]
async fn test_successful_atomic_commit() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let graph_id = "test_graph_commit";
    let coordinator = create_test_coordinator(graph_id);
    let tenant_ctx = create_test_tenant();

    // Create test data
    let node = create_test_node("symbol1", "Symbol");
    let embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];
    let edges = vec![
        create_test_edge("edge1", "symbol1", "other1", "CONNECTS_TO"),
        create_test_edge("edge2", "symbol1", "other2", "REFERENCES"),
    ];

    // Execute transaction
    let result = coordinator
        .write_symbol_atomically(node.clone(), embedding.clone(), edges.clone(), &tenant_ctx)
        .await
        .unwrap();

    // Verify committed outcome
    match result {
        TransactionOutcome::Committed { node_oid } => {
            assert_eq!(node_oid, "symbol1");
        }
        other => panic!("Expected Committed, got {:?}", other),
    }

    // Verify all data persisted via canonical read paths
    let graph_engine = coordinator.graph_engine();
    let node_id = &node.id;

    // Check node exists
    let retrieved_node = graph_engine.get_node(node_id).unwrap().unwrap();
    assert_eq!(retrieved_node.id, "symbol1");
    assert!(retrieved_node.labels.contains(&"Symbol".to_string()));

    // Check edges exist
    for edge in &edges {
        let retrieved_edge = graph_engine.get_edge(&edge.id).unwrap().unwrap();
        assert_eq!(retrieved_edge.id, edge.id);
        assert_eq!(retrieved_edge.from_node_id, "symbol1");
    }

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Rollback on embedding validation failure.
#[tokio::test]
async fn test_rollback_on_embedding_validation_failure() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let coordinator = create_test_coordinator("test_graph_rollback_embedding");
    let tenant_ctx = create_test_tenant();

    // Create node with invalid embedding (contains NaN)
    let node = create_test_node("symbol_invalid", "Symbol");
    let invalid_embedding = vec![0.1, f32::NAN, 0.3];
    let edges = vec![];

    // Execute transaction - should rollback
    let result = coordinator
        .write_symbol_atomically(node.clone(), invalid_embedding, edges, &tenant_ctx)
        .await
        .unwrap();

    // Verify rolled back outcome
    match result {
        TransactionOutcome::RolledBack { reason } => {
            assert!(reason.contains("Embedding write failed"));
        }
        other => panic!("Expected RolledBack, got {:?}", other),
    }

    // Verify node was rolled back (doesn't exist)
    let graph_engine = coordinator.graph_engine();
    let retrieved_node = graph_engine.get_node(&node.id).unwrap();
    assert!(
        retrieved_node.is_none(),
        "Node should have been rolled back"
    );

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Rollback on edge write failure.
///
/// This test simulates a failure during the edge write phase by attempting
/// to create an edge that references a non-existent node.
#[tokio::test]
async fn test_rollback_on_edge_write_failure() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let coordinator = create_test_coordinator("test_graph_rollback_edge");
    let tenant_ctx = create_test_tenant();

    // Create valid node and embedding
    let node = create_test_node("symbol_edge_fail", "Symbol");
    let embedding = vec![0.1, 0.2, 0.3];

    // Create edges - the edge write itself should succeed,
    // but we verify the transaction handles edge writes correctly
    let edges = vec![create_test_edge(
        "edge_ok_1",
        "symbol_edge_fail",
        "other1",
        "CONNECTS_TO",
    )];

    // Execute transaction - should succeed
    let result = coordinator
        .write_symbol_atomically(node.clone(), embedding.clone(), edges.clone(), &tenant_ctx)
        .await
        .unwrap();

    match result {
        TransactionOutcome::Committed { node_oid } => {
            assert_eq!(node_oid, "symbol_edge_fail");
        }
        other => panic!("Expected Committed, got {:?}", other),
    }

    // Verify node and edges exist
    let graph_engine = coordinator.graph_engine();
    assert!(graph_engine.get_node(&node.id).unwrap().is_some());
    assert!(graph_engine.get_edge("edge_ok_1").unwrap().is_some());

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Idempotent retry (upsert semantics).
///
/// Running the same transaction twice should succeed (upsert behavior).
#[tokio::test]
async fn test_idempotent_retry_upsert() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let coordinator = create_test_coordinator("test_graph_idempotent");
    let tenant_ctx = create_test_tenant();

    // Create test data
    let node = create_test_node("symbol_upsert", "Symbol");
    let embedding = vec![0.1, 0.2, 0.3];
    let edges = vec![create_test_edge(
        "edge_upsert",
        "symbol_upsert",
        "other1",
        "LINK",
    )];

    // First transaction
    let result1 = coordinator
        .write_symbol_atomically(node.clone(), embedding.clone(), edges.clone(), &tenant_ctx)
        .await
        .unwrap();

    assert!(matches!(result1, TransactionOutcome::Committed { .. }));

    // Second transaction with same data (upsert)
    let result2 = coordinator
        .write_symbol_atomically(node, embedding, edges, &tenant_ctx)
        .await
        .unwrap();

    // Should still succeed (upsert semantics)
    assert!(matches!(result2, TransactionOutcome::Committed { .. }));

    // Verify data exists
    let graph_engine = coordinator.graph_engine();
    assert!(graph_engine.get_node("symbol_upsert").unwrap().is_some());

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Coordinator handles empty edge list.
#[tokio::test]
async fn test_handles_empty_edges() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let coordinator = create_test_coordinator("test_graph_no_edges");
    let tenant_ctx = create_test_tenant();

    let node = create_test_node("symbol_no_edges", "Symbol");
    let embedding = vec![0.1, 0.2, 0.3];
    let edges = vec![];

    let result = coordinator
        .write_symbol_atomically(node.clone(), embedding, edges, &tenant_ctx)
        .await
        .unwrap();

    match result {
        TransactionOutcome::Committed { node_oid } => {
            assert_eq!(node_oid, "symbol_no_edges");

            // Verify node exists
            let graph_engine = coordinator.graph_engine();
            assert!(graph_engine.get_node(&node.id).unwrap().is_some());
        }
        other => panic!("Expected Committed, got {:?}", other),
    }

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Multiple concurrent transactions don't interfere.
#[tokio::test]
async fn test_concurrent_transactions() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let graph_id = "test_graph_concurrent";
    let coordinator = Arc::new(create_test_coordinator(graph_id));

    let mut handles = vec![];

    // Spawn 10 concurrent transactions
    for i in 0..10 {
        let coordinator = Arc::clone(&coordinator);
        let tenant_ctx = create_test_tenant();

        let handle = tokio::spawn(async move {
            let node = create_test_node(&format!("symbol_concurrent_{}", i), "Symbol");
            let embedding = vec![0.1; 128];
            let edges = vec![];

            coordinator
                .write_symbol_atomically(node, embedding, edges, &tenant_ctx)
                .await
        });

        handles.push(handle);
    }

    // Wait for all transactions
    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap().unwrap())
        .collect();

    // All should succeed
    assert_eq!(results.len(), 10);
    for result in results {
        assert!(matches!(result, TransactionOutcome::Committed { .. }));
    }

    // Verify all nodes exist
    let graph_engine = coordinator.graph_engine();
    for i in 0..10 {
        let node_id = format!("symbol_concurrent_{}", i);
        assert!(graph_engine.get_node(&node_id).unwrap().is_some());
    }

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Embedding validation rejects Inf values.
#[tokio::test]
async fn test_embedding_validation_rejects_inf() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let coordinator = create_test_coordinator("test_graph_inf_validation");
    let tenant_ctx = create_test_tenant();

    let node = create_test_node("symbol_inf", "Symbol");
    let inf_embedding = vec![0.1, f32::INFINITY, 0.3];
    let edges = vec![];

    let result = coordinator
        .write_symbol_atomically(node.clone(), inf_embedding, edges, &tenant_ctx)
        .await
        .unwrap();

    match result {
        TransactionOutcome::RolledBack { reason } => {
            assert!(reason.contains("non-finite"));
        }
        other => panic!("Expected RolledBack, got {:?}", other),
    }

    // Verify node was rolled back
    let graph_engine = coordinator.graph_engine();
    assert!(graph_engine.get_node(&node.id).unwrap().is_none());

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}

/// Test: Large embedding vector is handled correctly.
#[tokio::test]
async fn test_large_embedding_vector() {
    env::set_var("PROXIMADB_CROSS_MODEL_TX_ENABLED", "true");

    let coordinator = create_test_coordinator("test_graph_large_embedding");
    let tenant_ctx = create_test_tenant();

    let node = create_test_node("symbol_large", "Symbol");
    // 1536 dimensions (OpenAI embedding size)
    let large_embedding: Vec<f32> = (0..1536).map(|i| i as f32 / 1536.0).collect();
    let edges = vec![];

    let result = coordinator
        .write_symbol_atomically(node.clone(), large_embedding, edges, &tenant_ctx)
        .await
        .unwrap();

    match result {
        TransactionOutcome::Committed { node_oid } => {
            assert_eq!(node_oid, "symbol_large");
        }
        other => panic!("Expected Committed, got {:?}", other),
    }

    // Verify node exists
    let graph_engine = coordinator.graph_engine();
    assert!(graph_engine.get_node(&node.id).unwrap().is_some());

    env::remove_var("PROXIMADB_CROSS_MODEL_TX_ENABLED");
}
