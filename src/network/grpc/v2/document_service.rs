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
use crate::storage::document::{DocumentRecord, DocumentService};

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

    /// Derive the effective backing collection namespace from the request tenant.
    ///
    /// Isolation is structural: the tenant is folded into the storage key, never
    /// applied as a per-query predicate. Embedded / unauthenticated calls (no
    /// tenant) fall back to the raw `collection_id`.
    fn effective_collection_id<T>(request: &Request<T>, collection_id: &str) -> String {
        match grpc_auth::tenant_id(request) {
            Some(tenant) if !tenant.is_empty() => format!("{tenant}::{collection_id}"),
            _ => collection_id.to_string(),
        }
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
    use crate::storage::document::DocumentRecord;

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
        let collection = Self::effective_collection_id(&request, &request.get_ref().collection_id);
        let req = request.into_inner();
        debug!("v2 gRPC CreateDocument collection={collection}");

        let props = conv::props_to_tree(&req.props)?;
        let id = if req.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.id
        };
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
                document: Some(conv::record_to_v2(&stored)),
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
        let collection = Self::effective_collection_id(&request, &request.get_ref().collection_id);
        let req = request.into_inner();
        debug!("v2 gRPC GetDocument collection={collection} id={}", req.id);

        match self
            .documents
            .get_document(&collection, &req.id, None)
            .await
        {
            Ok(Some(record)) => Ok(Response::new(pv2::DocumentResponse {
                document: Some(conv::record_to_v2(&record)),
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
        let collection = Self::effective_collection_id(&request, &request.get_ref().collection_id);
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
