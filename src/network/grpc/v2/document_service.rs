// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC V2 native document service implementation.
//!
//! Canonical `proximadb.v2.ProximaDocumentService` surface, mirroring the graph
//! (`graph_service.rs`) and record (`record_service.rs`) v2 services.
//!
//! ## Design
//!
//! - **ProximaValue-native wire, never `SqlValue`.** The document body is carried
//!   as the canonical v2 [`pv2::TypedValue`] (full ProximaValue coverage), so the
//!   rich type system — decimals, timestamps, UUIDs, vectors — round-trips
//!   losslessly. The v1 `SqlValue` envelope (and its lossy conversions) never
//!   appears on this path. See the design note in
//!   `neutral_type_and_map_policy` and the `conv` helpers below.
//! - **Neutral internal model.** Handlers map v2 messages to the neutral,
//!   ProximaTree-native [`DocumentRecord`] at the boundary only, then call the
//!   shared [`DocumentService`] via its `DocumentRecord`-native entry point
//!   (`insert_document_record`) — the engine never sees a wire type. This is the
//!   document analog of the graph neutral model.
//! - **Structural tenant isolation.** Each handler folds the request tenant
//!   (`x-tenant-id` / auth context) into the backing collection key via
//!   [`grpc_auth::tenant_id`]; isolation is a namespace on the storage key, never
//!   a per-request predicate.
//!
//! ## Deferred RPCs
//!
//! The first surface ships document CRUD (create / get / delete). Query, update,
//! and aggregate are deferred follow-ups, mirroring how the graph service staged
//! its advanced RPCs (TD-124).

use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::{debug, error};

use crate::network::grpc::auth as grpc_auth;
use crate::proto::proximadb_v2 as pv2;
use crate::proto::proximadb_v2::proxima_document_service_server::{
    ProximaDocumentService, ProximaDocumentServiceServer,
};
use crate::storage::document::{DocumentQueryParams, DocumentRecord, DocumentService};

/// gRPC V2 native document service.
pub struct ProximaDocumentServiceImpl {
    /// The shared document backing service — held directly so the service is
    /// constructible in tests from a standalone `DocumentService` (no ROOT
    /// `UnifiedHandlers` dependency).
    documents: Arc<DocumentService>,
}

impl ProximaDocumentServiceImpl {
    /// Create a new service over the shared document backing service.
    pub fn new(documents: Arc<DocumentService>) -> Self {
        Self { documents }
    }

    /// Convert to a tonic server.
    pub fn into_server(self) -> ProximaDocumentServiceServer<Self> {
        ProximaDocumentServiceServer::new(self)
    }

    /// Resolve the request tenant and compose the `(clean, scoped)` collection pair.
    ///
    /// Isolation is structural: `scoped` is the storage key `{tenant}/{collection}` (default
    /// tenant ⇒ bare, matching bare-created collections) used for every `DocumentService` call
    /// AND for the record's `collection_id` (they key the same canonical OID). `clean` is the
    /// tenant-clean name the caller sent, echoed back in responses so the `{tenant}/` prefix never
    /// leaks. Fail-closed on an invalid tenant. Replaces the former `{tenant}::` name fold.
    fn scoped_collection<T>(
        request: &Request<T>,
        collection_id: &str,
    ) -> Result<(String, String), Status> {
        let tenant = grpc_auth::resolved_tenant_id(request)?;
        let scoped =
            crate::storage::document::service::scoped_document_collection(&tenant, collection_id)
                .map_err(|e| Status::invalid_argument(format!("invalid tenant: {e}")))?;
        Ok((collection_id.to_string(), scoped))
    }
}

// ============================================================================
// v2 <-> neutral type mapping — handler boundary only. ProximaValue-native; the
// legacy v1 `SqlValue` is never touched on this path.
// ============================================================================
mod conv {
    use super::pv2;
    use std::collections::HashMap;
    use tonic::Status;

    use proximadb_records::proto_v2::{proxima_value_to_typed_value, typed_value_to_proxima};
    use proximadb_records::{ProximaTree, ProximaTreeNode};

    use crate::core::search::sql_value_filter::proxima_tree_to_value_map;
    use crate::proto::proximadb_v1 as pv1;
    use crate::storage::document::DocumentRecord;
    use crate::storage::document::canonical_adapter::tree_node_to_sql_value;

    /// v2 wire props (`TypedValue` map) -> neutral [`ProximaTree`]. Each field
    /// becomes a `Value` node; nested objects ride inside the `ProximaValue`
    /// (`TypedValueMap` -> `ProximaValue::Map`/`Struct`), matching the record
    /// service convention. Lossless — never detours through v1 `SqlValue`.
    pub fn props_to_tree(props: &HashMap<String, pv2::TypedValue>) -> Result<ProximaTree, Status> {
        props
            .iter()
            .map(|(k, tv)| {
                typed_value_to_proxima(tv)
                    .map(|pv| (k.clone(), ProximaTreeNode::Value(pv)))
                    .map_err(|e| Status::invalid_argument(format!("field '{k}': {e}")))
            })
            .collect()
    }

    /// neutral [`ProximaTree`] -> v2 wire props (`TypedValue` map). Nested objects
    /// are preserved as `ProximaValue::Struct` by `proxima_tree_to_value_map`.
    pub fn tree_to_props(tree: &ProximaTree) -> HashMap<String, pv2::TypedValue> {
        proxima_tree_to_value_map(tree)
            .iter()
            .map(|(k, pv)| (k.clone(), proxima_value_to_typed_value(pv)))
            .collect()
    }

    /// neutral [`DocumentRecord`] -> v2 `Document` wire message.
    pub fn record_to_v2(record: &DocumentRecord) -> pv2::Document {
        pv2::Document {
            collection_id: record.collection_id.clone(),
            id: record.id.clone(),
            props: tree_to_props(&record.props),
            version: record.version,
            schema_id: record.schema_id.clone(),
            document_type: record.document_type.clone(),
            updated_at_ms: record.updated_at_ns / 1_000_000,
        }
    }

    /// Like [`record_to_v2`], but echoes the tenant-CLEAN `collection_id` the caller sent instead
    /// of the record's stored (tenant-scoped `{tenant}/{collection}`) key — so the structural
    /// scope prefix never leaks back to the client.
    pub fn record_to_v2_clean(record: &DocumentRecord, clean_collection: &str) -> pv2::Document {
        let mut doc = record_to_v2(record);
        doc.collection_id = clean_collection.to_string();
        doc
    }

    /// v2 sort fields -> v1 proto `SortField`. Value-free (path + order only), so
    /// no `SqlValue` is involved; the enum discriminants line up 1:1.
    pub fn sort_to_v1(sort: &[pv2::DocumentSortField]) -> Vec<pv1::SortField> {
        sort.iter()
            .map(|s| pv1::SortField {
                path: s.path.clone(),
                order: s.order,
            })
            .collect()
    }

    /// v2 `DocumentFieldUpdate` -> v1 `DocumentUpdate`. The discriminants line up
    /// 1:1; the value is lifted from TypedValue -> ProximaValue -> SqlValue at the
    /// handler boundary, then passed to the backing DocumentService::update_document.
    /// This is a transitional path until DocumentService accepts ProximaValue directly.
    pub fn field_updates_to_v1(
        updates: &[pv2::DocumentFieldUpdate],
    ) -> Result<Vec<pv1::DocumentUpdate>, Status> {
        updates
            .iter()
            .map(|u| {
                let value = match &u.value {
                    None => None,
                    Some(tv) => {
                        let pv = typed_value_to_proxima(tv).map_err(|e| {
                            Status::invalid_argument(format!("update value for '{}': {e}", u.path))
                        })?;
                        // Transitional bridge: ProximaValue -> SqlValue (the backing
                        // service apply_update expects SqlValue). This is the only place
                        // v2 touches SqlValue — a follow-up can lift DocumentService to
                        // accept ProximaValue directly.
                        Some(tree_node_to_sql_value(&ProximaTreeNode::Value(pv)))
                    }
                };
                Ok(pv1::DocumentUpdate {
                    operation: u.operation - 1, // v2 discriminants are offset by 1
                    path: u.path.clone(),
                    value,
                })
            })
            .collect()
    }

    /// v2 `AggregationStage` -> v1 `AggregationStage`. Both carry a `stage` oneof
    /// with parallel variants (Group/Project/Sort/Limit/Skip); v2's discriminants
    /// and field shapes mirror v1, so this is a structural 1:1 copy. v1 also has
    /// Match/Unwind/Lookup which v2 defers — they are not produced here.
    pub fn aggregation_stage_to_v1(stage: &pv2::AggregationStage) -> pv1::AggregationStage {
        use pv1::aggregation_stage::Stage as V1Stage;
        use pv2::aggregation_stage::Stage as V2Stage;

        match &stage.stage {
            None => pv1::AggregationStage::default(),
            Some(V2Stage::Group(group)) => {
                let aggregations = group
                    .aggregations
                    .iter()
                    .map(|a| pv1::Aggregation {
                        output_field: a.output_field.clone(),
                        r#type: a.r#type,
                        input_path: a.input_path.clone(),
                    })
                    .collect();
                pv1::AggregationStage {
                    stage: Some(V1Stage::Group(pv1::GroupStage {
                        key: group.key.clone(),
                        aggregations,
                    })),
                }
            }
            Some(V2Stage::Project(project)) => pv1::AggregationStage {
                stage: Some(V1Stage::Project(pv1::ProjectStage {
                    fields: project.fields.clone(),
                    computed: project.computed.clone(),
                })),
            },
            Some(V2Stage::Sort(sort)) => {
                let fields = sort
                    .sort
                    .iter()
                    .map(|s| pv1::SortField {
                        path: s.path.clone(),
                        order: s.order,
                    })
                    .collect();
                pv1::AggregationStage {
                    stage: Some(V1Stage::Sort(pv1::SortStage { fields })),
                }
            }
            Some(V2Stage::Limit(limit)) => pv1::AggregationStage {
                stage: Some(V1Stage::Limit(pv1::LimitStage { limit: limit.limit })),
            },
            Some(V2Stage::Skip(skip)) => pv1::AggregationStage {
                stage: Some(V1Stage::Skip(pv1::SkipStage { skip: skip.skip })),
            },
        }
    }

    /// v1 `SqlObject` result -> v2 `AggregationResult` (ProximaValue-native). The
    /// aggregation executor returns SqlObject (v1 legacy surface); we convert to
    /// ProximaTree via sql_object_to_proxima_tree, then to TypedValue map.
    pub fn sql_object_to_aggregation_result(
        obj: &crate::proto::proximadb_v1::SqlObject,
    ) -> pv2::AggregationResult {
        let tree = crate::storage::document::canonical_adapter::sql_object_to_proxima_tree(obj);
        pv2::AggregationResult {
            fields: tree_to_props(&tree),
        }
    }
}

/// Map a backing-service error onto a gRPC status, inferring the code from the
/// error message (mirrors the graph service's `graph_status`).
fn doc_status(operation: &str, err: impl std::fmt::Display) -> Status {
    let message = err.to_string();
    let lower = message.to_lowercase();
    if lower.contains("not found") {
        Status::not_found(message)
    } else if lower.contains("already exists") {
        Status::already_exists(message)
    } else if lower.contains("invalid") || lower.contains("required") {
        Status::invalid_argument(message)
    } else {
        Status::internal(format!("{operation}: {message}"))
    }
}

#[tonic::async_trait]
impl ProximaDocumentService for ProximaDocumentServiceImpl {
    async fn create_document(
        &self,
        request: Request<pv2::CreateDocumentRequest>,
    ) -> Result<Response<pv2::DocumentResponse>, Status> {
        let (clean, collection) =
            Self::scoped_collection(&request, &request.get_ref().collection_id)?;
        let req = request.into_inner();
        debug!("v2 gRPC CreateDocument collection={collection}");

        let props = conv::props_to_tree(&req.props)?;
        let id = if req.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.id
        };
        // `record.collection_id` = the SCOPED key (same as the storage-key param): together they
        // key the canonical OID, so recovery/read agree.
        let record = DocumentRecord::from_tree(
            id,
            props,
            collection.clone(),
            req.schema_id,
            req.document_type,
        );

        match self
            .documents
            .insert_document_record(&collection, record)
            .await
        {
            Ok(stored) => Ok(Response::new(pv2::DocumentResponse {
                document: Some(conv::record_to_v2_clean(&stored, &clean)),
            })),
            Err(e) => {
                error!("v2 gRPC CreateDocument failed: {e}");
                Err(doc_status("create document", e))
            }
        }
    }

    async fn get_document(
        &self,
        request: Request<pv2::GetDocumentRequest>,
    ) -> Result<Response<pv2::DocumentResponse>, Status> {
        let (clean, collection) =
            Self::scoped_collection(&request, &request.get_ref().collection_id)?;
        let req = request.into_inner();
        debug!("v2 gRPC GetDocument collection={collection} id={}", req.id);

        match self
            .documents
            .get_document(&collection, &req.id, None)
            .await
        {
            Ok(Some(record)) => Ok(Response::new(pv2::DocumentResponse {
                document: Some(conv::record_to_v2_clean(&record, &clean)),
            })),
            Ok(None) => Err(Status::not_found(format!(
                "document '{}' not found in collection '{collection}'",
                req.id
            ))),
            Err(e) => {
                error!("v2 gRPC GetDocument failed: {e}");
                Err(doc_status("get document", e))
            }
        }
    }

    async fn delete_document(
        &self,
        request: Request<pv2::DeleteDocumentRequest>,
    ) -> Result<Response<pv2::DeleteDocumentResponse>, Status> {
        let (_clean, collection) =
            Self::scoped_collection(&request, &request.get_ref().collection_id)?;
        let req = request.into_inner();
        debug!(
            "v2 gRPC DeleteDocument collection={collection} id={}",
            req.id
        );

        match self.documents.delete_document(&collection, &req.id).await {
            Ok(deleted) => Ok(Response::new(pv2::DeleteDocumentResponse { deleted })),
            Err(e) => {
                error!("v2 gRPC DeleteDocument failed: {e}");
                Err(doc_status("delete document", e))
            }
        }
    }

    async fn update_document(
        &self,
        request: Request<pv2::UpdateDocumentRequest>,
    ) -> Result<Response<pv2::DocumentResponse>, Status> {
        let (clean, collection) =
            Self::scoped_collection(&request, &request.get_ref().collection_id)?;
        let req = request.into_inner();
        debug!(
            "v2 gRPC UpdateDocument collection={collection} id={} updates={}",
            req.id,
            req.updates.len()
        );

        let updates = conv::field_updates_to_v1(&req.updates)?;
        match self
            .documents
            .update_document(&collection, &req.id, updates, req.expected_version)
            .await
        {
            Ok(updated) => Ok(Response::new(pv2::DocumentResponse {
                document: Some(conv::record_to_v2_clean(&updated, &clean)),
            })),
            Err(e) => {
                error!("v2 gRPC UpdateDocument failed: {e}");
                Err(doc_status("update document", e))
            }
        }
    }

    async fn query_documents(
        &self,
        request: Request<pv2::QueryDocumentsRequest>,
    ) -> Result<Response<pv2::QueryDocumentsResponse>, Status> {
        let (clean, collection) =
            Self::scoped_collection(&request, &request.get_ref().collection_id)?;
        let req = request.into_inner();
        debug!(
            "v2 gRPC QueryDocuments collection={collection} limit={} offset={}",
            req.limit, req.offset
        );

        // First slice: a value-free scan (projection + sort + pagination). A
        // predicate filter is deferred until it can be ProximaValue-native, so
        // none is set here.
        let params = DocumentQueryParams {
            filter: None,
            projection: req.projection,
            sort: conv::sort_to_v1(&req.sort),
            limit: req.limit,
            offset: req.offset,
            include_count: req.include_count,
        };

        match self.documents.query_documents(&collection, params).await {
            Ok(result) => Ok(Response::new(pv2::QueryDocumentsResponse {
                documents: result
                    .documents
                    .iter()
                    .map(|r| conv::record_to_v2_clean(r, &clean))
                    .collect(),
                total_count: result.total_count,
                query_time_ms: result.query_time_ms,
            })),
            Err(e) => {
                error!("v2 gRPC QueryDocuments failed: {e}");
                Err(doc_status("query documents", e))
            }
        }
    }

    async fn aggregate_documents(
        &self,
        request: Request<pv2::AggregateDocumentsRequest>,
    ) -> Result<Response<pv2::AggregateDocumentsResponse>, Status> {
        let (_clean, collection) =
            Self::scoped_collection(&request, &request.get_ref().collection_id)?;
        let req = request.into_inner();
        debug!(
            "v2 gRPC AggregateDocuments collection={} pipeline_len={}",
            collection,
            req.pipeline.len()
        );

        // Convert v2 pipeline to v1 (discriminants line up 1:1)
        let v1_pipeline: Vec<crate::proto::proximadb_v1::AggregationStage> = req
            .pipeline
            .iter()
            .map(conv::aggregation_stage_to_v1)
            .collect();

        match self
            .documents
            .aggregate_documents(&collection, None, v1_pipeline)
            .await
        {
            Ok(result) => {
                let results = result
                    .results
                    .iter()
                    .map(conv::sql_object_to_aggregation_result)
                    .collect();
                Ok(Response::new(pv2::AggregateDocumentsResponse {
                    results,
                    query_time_ms: result.query_time_ms,
                }))
            }
            Err(e) => {
                error!("v2 gRPC AggregateDocuments failed: {e}");
                Err(doc_status("aggregate documents", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{conv, pv2};
    use proximadb_records::ProximaTreeNode;
    use std::collections::HashMap;

    fn tv(value: pv2::typed_value::Value) -> pv2::TypedValue {
        pv2::TypedValue {
            declared_type: 0,
            value: Some(value),
        }
    }

    /// The v2 document wire carries the rich `ProximaValue` type system — notably
    /// a native UUID, which the legacy v1 `SqlValue` envelope cannot represent.
    /// Prove the `TypedValue` <-> neutral `ProximaTree` round-trip preserves the
    /// values losslessly (the whole reason v2 must not route through `SqlValue`).
    #[test]
    fn typed_value_props_round_trip_is_lossless() {
        use pv2::typed_value::Value;

        let mut props = HashMap::new();
        props.insert(
            "name".to_string(),
            tv(Value::TextValue("doc-1".to_string())),
        );
        props.insert("count".to_string(), tv(Value::IntegerValue(42)));
        props.insert("uid".to_string(), tv(Value::UuidValue(vec![7u8; 16])));

        // v2 wire props -> neutral ProximaTree -> v2 wire props -> neutral tree.
        let tree1 = conv::props_to_tree(&props).expect("props -> tree");
        assert_eq!(tree1.len(), 3, "all three fields decoded");
        let props2 = conv::tree_to_props(&tree1);
        let tree2 = conv::props_to_tree(&props2).expect("re-encode round-trips");

        // The neutral representation is stable across the wire round-trip — the
        // UUID survives, which is impossible through the v1 SqlValue path.
        assert_eq!(
            tree1, tree2,
            "ProximaValue types preserved across the v2 round-trip"
        );
        assert!(
            matches!(tree1.get("uid"), Some(ProximaTreeNode::Value(_))),
            "native UUID round-trips as a value node"
        );
    }
}
