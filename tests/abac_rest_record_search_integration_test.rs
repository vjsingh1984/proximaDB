// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-ABAC-5/6 (REST records slice): proves ABAC enforcement FIRES on the REST
//! record-search path (`search_records_with_tenant_context`), not just the
//! fusion/service seam.
//!
//! A `VectorOperationsService` is built with a durable ABAC enforcer; the REST
//! records-search entry point threads the request subject + tenant stable id into
//! `_inner`, which resolves the subject's read context and ANDs the security
//! predicate (`dept=eng`) into the search. So:
//! * an admitted subject (`alice`, dept=eng) sees only `eng` records — the `hr`
//!   record is filtered out;
//! * a denied subject (`mallory`, no attribute binding) gets an **empty** result
//!   (fail-closed);
//! * a `None` subject (the gRPC/Flight callers that still delegate with `None`)
//!   gets the unfiltered results (passthrough — today's behavior preserved).
//!
//! The binding is `Scope::Namespace(0)` because `resolve_vector_read_context`
//! builds `Target { namespace: 0, .. }` — a namespace-scoped permit matches
//! regardless of the collection's catalog object id (the test harness mints
//! name-shaped ids that don't parse as the numeric object id).
//!
//! Default-OFF: this file compiles only under `--features abac-policy`.

#![cfg(all(test, feature = "abac-policy"))]

#[path = "common/integration_test_helpers.rs"]
mod harness;

use harness::UnifiedTestEnvironment;
use proximadb::core::search::{ComparisonOperator, FilterExpression};
use proximadb::security::rls::AbacEnforcer;
use proximadb_abac::{
    InMemoryAttributeAuthority, InMemoryPolicyEpochs, InMemoryPredicateObjectStore,
};
use proximadb_catalog::fc_metamodel::{AttrValue, Effect, PolicyBinding, Scope};
use proximadb_data_model::ProximaValue;
use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaRecord, ProximaTreeNode};
use proximadb_runtime::RichSearchRequest;
use serde_json::json;
use std::sync::Arc;

const COLLECTION: &str = "abac_rest_record_search";
const DIM: u32 = 4;
const TENANT: u64 = 7;
const OID_ENG: &str = "abac/record/eng";
const OID_HR: &str = "abac/record/hr";

/// Admit `alice` (dept=eng) under a `dept=eng` row predicate, scoped to
/// `Namespace(0)` so it matches `resolve_vector_read_context`'s
/// `Target { namespace: 0, .. }` regardless of the collection's object id.
fn enforcer_for_alice() -> Arc<AbacEnforcer> {
    let mut authority = InMemoryAttributeAuthority::new();
    authority.upsert(
        proximadb_abac::AttributeBinding::new("alice", TENANT)
            .with_attr("dept", AttrValue::Str("eng".into())),
    );
    let mut store = InMemoryPredicateObjectStore::new();
    store.register(
        42,
        FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("eng"),
        },
    );
    let bindings = vec![PolicyBinding {
        object_id: 1,
        tenant_stable_id: TENANT,
        scope: Scope::Namespace(0),
        effect: Effect::Permit,
        predicate_ref: Some(42),
        field_mask: None,
    }];
    Arc::new(
        AbacEnforcer::new(
            Arc::new(authority),
            Arc::new(store),
            Arc::new(InMemoryPolicyEpochs::new()),
        )
        .with_bindings(bindings),
    )
}

fn record(oid: &str, dept: &str, vector: Vec<f32>) -> ProximaRecord {
    let mut rec = ProximaRecord {
        oid: oid.to_string(),
        record_version: 1,
        embeddings: vec![EmbeddingCell {
            model_id: "test".to_string(),
            modality: "dense_vector".to_string(),
            dim: DIM,
            values: EmbeddingValues::Fp32(vector),
            ..Default::default()
        }],
        ..Default::default()
    };
    rec.props.insert(
        "dept".to_string(),
        ProximaTreeNode::Value(ProximaValue::String(dept.to_string())),
    );
    rec
}

fn request() -> RichSearchRequest {
    RichSearchRequest {
        collection_id: COLLECTION.to_string(),
        query_vector: vec![1.0, 0.0, 0.0, 0.0],
        top_k: 10,
        filters: vec![],
    }
}

/// Build the service + collection + records once; the per-subject assertions
/// share it.
async fn fixture() -> (
    Arc<proximadb::services::VectorOperationsService>,
    Arc<proximadb::services::CollectionService>,
) {
    let env = UnifiedTestEnvironment::new().await.expect("test env");
    let (svc, collections) = env
        .vector_operations_service_with_abac(enforcer_for_alice())
        .await
        .expect("vector service with abac");
    UnifiedTestEnvironment::create_vector_collection(&collections, COLLECTION, DIM)
        .await
        .expect("create collection");
    svc.insert_vectors_direct(
        COLLECTION,
        Arc::new(vec![
            record(OID_ENG, "eng", vec![1.0, 0.0, 0.0, 0.0]),
            record(OID_HR, "hr", vec![0.99, 0.01, 0.0, 0.0]),
        ]),
    )
    .await
    .expect("insert records");
    (svc, collections)
}

/// An admitted subject sees only `dept=eng` records — the `hr` record is
/// filtered out by the pushed-down / post-filtered security predicate.
#[tokio::test]
async fn admitted_subject_sees_only_accessible_records_on_rest_records_path() {
    let (svc, _collections) = fixture().await;

    let resp = svc
        .search_records_with_tenant_context(request(), None, Some("alice"), Some(TENANT))
        .await
        .expect("abac search");

    let ids: Vec<&str> = resp.results.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains(&OID_ENG),
        "alice (dept=eng) must see the eng record; got {ids:?}"
    );
    assert!(
        !ids.contains(&OID_HR),
        "the hr record must be filtered out; got {ids:?}"
    );
}

/// A denied subject (no attribute binding) gets an EMPTY result — fail-closed.
#[tokio::test]
async fn denied_subject_gets_empty_results_on_rest_records_path() {
    let (svc, _collections) = fixture().await;

    let resp = svc
        .search_records_with_tenant_context(request(), None, Some("mallory"), Some(TENANT))
        .await
        .expect("abac search");

    assert!(
        resp.results.is_empty(),
        "mallory (unbound) must be denied — empty results (fail-closed); got {}",
        resp.results.len()
    );
}

/// A `None` subject (the gRPC/Flight callers that still delegate with `None`)
/// gets the unfiltered results — today's behavior is preserved until those
/// surfaces adopt the `_abac` variant.
#[tokio::test]
async fn none_subject_is_passthrough_on_rest_records_path() {
    let (svc, _collections) = fixture().await;

    let resp = svc
        .search_records_with_tenant_context(request(), None, None, None)
        .await
        .expect("abac search");

    let ids: Vec<&str> = resp.results.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains(&OID_ENG) && ids.contains(&OID_HR),
        "a None subject (gRPC/Flight path) must see both records (passthrough); got {ids:?}"
    );
}
