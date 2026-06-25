// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC V2 native entity service implementation.
//!
//! **Orchestration layer over existing services — NOT a separate storage path.**
//!
//! The EntityService orchestrates across three existing primitives:
//! - **GraphOperationsService** — entity-as-node with relations (topology)
//! - **VectorOperationsService** — embeddings (ANN search)
//! - **DocumentService** — provenance chunks (metadata)
//!
//! This avoids redundant storage: an "entity" is just a graph node with associated
//! vectors and optional document chunks. Tenant isolation is structural — the
//! request tenant (`x-tenant-id`) is folded into the backing collection key via
//! [`grpc_auth::tenant_id`], never a per-query predicate (mirrors the v2 document
//! service).

use std::collections::HashMap;
use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::{debug, error, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::core::search::cross_modal_fusion::FusionPolicy;
use crate::graph::{
    Edge, GraphOperationsService, Node, NodeId, NodeQuery, PropertyFilter, PropertyValue,
    property_value::Value as GraphValue,
};
use crate::network::grpc::auth as grpc_auth;
use crate::proto::proximadb_v2 as pv2;
use crate::proto::proximadb_v2::proxima_entity_service_server::{
    ProximaEntityService, ProximaEntityServiceServer,
};
use crate::services::fusion_service::{FusionService, GraphFusionParams, GraphGrain};
use crate::services::operations::vectors::VectorOperationsService;
use crate::storage::document::{DocumentRecord, DocumentService};
use proximadb_records::{
    EmbeddingCell, EmbeddingScalarType, EmbeddingValues, ProximaRecord, ProximaTree,
    ProximaTreeNode, ProximaValue,
};

/// gRPC V2 native entity service — orchestration layer over graph + vector + document.
pub struct ProximaEntityServiceImpl {
    /// Graph service for entity-as-node and relations
    graph_service: Arc<GraphOperationsService>,
    /// Vector service for embeddings
    vector_service: Arc<VectorOperationsService>,
    /// Cross-modal fusion port — the single retrieval engine
    /// (`SEARCH_SURFACE_CONTRACT_2026_06_24.adoc`). SearchEntities delegates its
    /// vector (`similar`) mode here rather than reimplementing retrieval.
    fusion_service: Arc<FusionService>,
    /// Document service for provenance/evidence chunks
    document_service: Arc<DocumentService>,
}

impl ProximaEntityServiceImpl {
    /// Create a new service from the shared unified request handlers.
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        let graph = request_handlers.graph_operations_service.clone();
        let vector = request_handlers.vector_operations_service.clone();
        Self {
            graph_service: graph.clone(),
            vector_service: vector.clone(),
            // The fusion port is built once at boot from the vector + graph
            // services (mirrors the REST AppState pattern, PR #282).
            fusion_service: Arc::new(FusionService::new(vector, graph)),
            document_service: request_handlers.document_service.clone(),
        }
    }

    /// Convert to a tonic server.
    pub fn into_server(self) -> ProximaEntityServiceServer<Self> {
        ProximaEntityServiceServer::new(self)
    }

    /// Derive the effective backing collection namespace from the request tenant.
    /// Isolation is structural: the tenant is folded into the storage key, never
    /// a per-query predicate.
    fn effective_collection_id<T>(request: &Request<T>, collection_id: &str) -> String {
        match grpc_auth::tenant_id(request) {
            Some(tenant) if !tenant.is_empty() => format!("{tenant}::{collection_id}"),
            _ => collection_id.to_string(),
        }
    }

    /// Generate a unique node ID for this entity in the graph.
    fn entity_node_id(collection_id: &str, entity_id: &str) -> NodeId {
        format!("entity:{collection_id}:{entity_id}")
    }

    /// Generate a unique auxiliary ID (embedding vector / provenance document)
    /// for an entity. The entity **node id** is the recoverable prefix (split on
    /// the last `/`), so fusion results — which carry only the vector `oid` —
    /// project back to their entity node without a re-fetch. The `/` delimiter
    /// matches the codebase's canonical-oid convention (`graph/{id}/node/{id}`).
    fn auxiliary_id(collection_id: &str, entity_id: &str, model_id: &str) -> String {
        format!(
            "{}/{model_id}",
            Self::entity_node_id(collection_id, entity_id)
        )
    }

    /// Recover the entity node id from an auxiliary (vector/provenance) oid.
    /// Inverse of [`Self::auxiliary_id`].
    fn node_id_from_auxiliary_oid(oid: &str) -> &str {
        oid.rsplit_once('/')
            .map(|(node_id, _)| node_id)
            .unwrap_or(oid)
    }
}

/// Map an internal error to a gRPC status.
fn entity_status(operation: &str, err: impl std::fmt::Display) -> Status {
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
impl ProximaEntityService for ProximaEntityServiceImpl {
    async fn upsert_entity(
        &self,
        request: Request<pv2::UpsertEntityRequest>,
    ) -> Result<Response<pv2::UpsertEntityResponse>, Status> {
        let collection = Self::effective_collection_id(&request, &request.get_ref().collection_id);
        let tenant_id = grpc_auth::tenant_id(&request).unwrap_or_default();
        let req = request.into_inner();
        let entity = req
            .entity
            .ok_or_else(|| Status::invalid_argument("upsert_entity: `entity` is required"))?;
        let entity_id = if entity.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            entity.id.clone()
        };

        debug!("v2 gRPC UpsertEntity collection={collection} entity_id={entity_id}");

        // Step 1: Create/update the graph node (authoritative entity record).
        let node_id = Self::entity_node_id(&collection, &entity_id);
        let mut node_properties = HashMap::new();
        for (k, v) in &entity.flexible_metadata {
            if let Some(pv) = typed_value_to_property_value(v) {
                node_properties.insert(k.clone(), pv);
            }
        }
        node_properties.insert("_entity_type".to_string(), str_property("entity"));
        node_properties.insert("_collection_id".to_string(), str_property(&collection));

        let now_ms = chrono::Utc::now().timestamp_millis();
        let node = Node {
            id: node_id.clone(),
            labels: vec!["entity".to_string()],
            properties: node_properties,
            embedding: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };

        // Upsert semantics: try create, fall back to update if the node exists.
        match self
            .graph_service
            .create_node(&collection, node.clone())
            .await
        {
            Ok(_) => debug!("Created graph node {node_id}"),
            Err(create_err) => {
                if let Err(update_err) = self.graph_service.update_node(&collection, node).await {
                    error!("Entity node upsert failed: create={create_err}; update={update_err}");
                    return Err(entity_status("upsert entity", update_err));
                }
                debug!("Updated graph node {node_id}");
            }
        }

        // Step 2: Upsert embedding vectors (best-effort; non-fatal on failure).
        for embedding in &entity.embeddings {
            let vector_id = Self::auxiliary_id(&collection, &entity_id, &embedding.model_id);
            let mut props = ProximaTree::new();
            props.insert(
                "_entity_id".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(entity_id.clone())),
            );
            props.insert(
                "_entity_collection".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(collection.clone())),
            );
            props.insert(
                "_embedding_model_id".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(embedding.model_id.clone())),
            );

            let record = ProximaRecord {
                oid: vector_id.clone(),
                tenant_id: tenant_id.clone(),
                local_id: Some(format!("{entity_id}:{}", embedding.model_id)),
                embeddings: vec![EmbeddingCell {
                    model_id: embedding.model_id.clone(),
                    modality: modality_to_string(embedding.modality),
                    values: EmbeddingValues::Fp32(embedding.vector.clone()),
                    dim: embedding.dimension,
                    precision: EmbeddingScalarType::Fp32,
                    precision_epoch: None,
                }],
                props,
                ..Default::default()
            };

            debug!(
                "Upsert embedding model={} dim={} vector_id={vector_id}",
                embedding.model_id,
                embedding.vector.len()
            );

            match self
                .vector_service
                .insert_batch(&collection, vec![record])
                .await
            {
                Ok(result) => {
                    if !result.success {
                        warn!(
                            "Embedding insert non-success for {vector_id}: {:?}",
                            result.errors
                        );
                    }
                }
                Err(e) => warn!("Embedding insert failed (non-fatal) for {vector_id}: {e}"),
            }
        }

        // Step 3: Store provenance as a document chunk if present (best-effort).
        if let Some(provenance) = entity.provenance.as_ref() {
            let doc_id = Self::auxiliary_id(&collection, &entity_id, "provenance");
            let mut tree = ProximaTree::new();
            tree.insert(
                "source_id".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(provenance.source_id.clone())),
            );
            tree.insert(
                "chunk_id".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(provenance.chunk_id.clone())),
            );
            tree.insert(
                "chunk_position".to_string(),
                ProximaTreeNode::Value(ProximaValue::Int32(provenance.chunk_position as i32)),
            );
            if !provenance.extraction_method.is_empty() {
                tree.insert(
                    "extraction_method".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String(
                        provenance.extraction_method.clone(),
                    )),
                );
            }
            for (k, v) in &provenance.metadata {
                tree.insert(
                    k.clone(),
                    ProximaTreeNode::Value(ProximaValue::String(v.clone())),
                );
            }
            tree.insert(
                "_entity_id".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(entity_id.clone())),
            );
            tree.insert(
                "_entity_collection".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(collection.clone())),
            );

            let doc_record = DocumentRecord::from_tree(
                doc_id.clone(),
                tree,
                collection.clone(),
                None,
                Some("entity_provenance".to_string()),
            );

            debug!("Insert provenance document doc_id={doc_id}");
            match self
                .document_service
                .insert_document_record(&collection, doc_record)
                .await
            {
                Ok(_) => debug!("Provenance document inserted"),
                Err(e) => warn!("Provenance insert failed (non-fatal) for {doc_id}: {e}"),
            }
        }

        // Step 4: Create relation edges (best-effort; non-fatal on failure).
        for relation in &entity.relations {
            let source = Self::entity_node_id(&collection, &relation.source_entity_id);
            let target = Self::entity_node_id(&collection, &relation.target_entity_id);
            let edge_id = format!("{source}:{}->{target}", relation.relation_type);

            let mut edge_properties = HashMap::new();
            for (k, v) in &relation.properties {
                edge_properties.insert(k.clone(), str_property(v));
            }
            edge_properties.insert("_entity_relation".to_string(), str_property("true"));

            let edge = Edge {
                id: edge_id.clone(),
                from_node_id: source.clone(),
                to_node_id: target.clone(),
                edge_type: relation.relation_type.clone(),
                properties: edge_properties,
                weight: Some(relation.weight as f64),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };

            match self.graph_service.create_edge(&collection, edge).await {
                Ok(_) => debug!("Created edge {edge_id}"),
                Err(e) => warn!("Edge create failed (non-fatal) for {edge_id}: {e}"),
            }
        }

        Ok(Response::new(pv2::UpsertEntityResponse {
            success: true,
            entity_id,
            message: "Entity upserted successfully".to_string(),
        }))
    }

    async fn get_entity(
        &self,
        request: Request<pv2::GetEntityRequest>,
    ) -> Result<Response<pv2::GetEntityResponse>, Status> {
        let collection = Self::effective_collection_id(&request, &request.get_ref().collection_id);
        let req = request.into_inner();

        debug!(
            "v2 gRPC GetEntity collection={collection} entity_id={}",
            req.entity_id
        );

        let node_id = Self::entity_node_id(&collection, &req.entity_id);
        let node = self
            .graph_service
            .get_node(&collection, &node_id)
            .await
            .map_err(|e| entity_status("get entity", e))?
            .ok_or_else(|| Status::not_found(format!("Entity '{}' not found", req.entity_id)))?;

        Ok(Response::new(pv2::GetEntityResponse {
            entity: Some(node_to_entity(&node, &collection)),
        }))
    }

    async fn delete_entity(
        &self,
        request: Request<pv2::DeleteEntityRequest>,
    ) -> Result<Response<pv2::DeleteEntityResponse>, Status> {
        let collection = Self::effective_collection_id(&request, &request.get_ref().collection_id);
        let req = request.into_inner();

        debug!(
            "v2 gRPC DeleteEntity collection={collection} entity_id={}",
            req.entity_id
        );

        let node_id = Self::entity_node_id(&collection, &req.entity_id);
        let deleted = self
            .graph_service
            .delete_node(&collection, &node_id)
            .await
            .map_err(|e| entity_status("delete entity", e))?
            .is_some();

        // NOTE: associated embedding vectors and provenance documents are not
        // automatically cascaded. They can be removed separately by the caller
        // using the auxiliary-id convention (`entity:{collection}:{entity_id}/{model}`).
        Ok(Response::new(pv2::DeleteEntityResponse {
            success: deleted,
            message: if deleted {
                "Entity deleted successfully".to_string()
            } else {
                "Entity not found".to_string()
            },
        }))
    }

    async fn search_entities(
        &self,
        request: Request<pv2::SearchEntitiesRequest>,
    ) -> Result<Response<pv2::SearchEntitiesResponse>, Status> {
        let collection = Self::effective_collection_id(&request, &request.get_ref().collection_id);
        let req = request.into_inner();

        debug!(
            "v2 gRPC SearchEntities collection={collection} top_k={} has_vector={} has_filters={}",
            req.top_k,
            req.similar.is_some(),
            req.filters.is_some()
        );

        // Case 1: vector (`similar`) search — delegate to the fusion seam
        // (`SEARCH_SURFACE_CONTRACT_2026_06_24.adoc`, TD-146). The seam runs the
        // vector seed and (gracefully) no-ops graph expansion here — entity node
        // ids are not yet in the canonical graph-oid space, so graph-augmented
        // fusion is deferred (TD-146 scope B). Only `similar.vector` is supported;
        // text/raw_data need a server-side embedding hop (not yet wired).
        if let Some(similar) = req.similar.as_ref() {
            let query_vector = match similar.query.as_ref() {
                Some(pv2::similar_query::Query::Vector(v)) => v.values.clone(),
                _ => {
                    return Err(Status::unimplemented(
                        "Only `similar.vector` is supported; text/raw_data embedding is not yet wired.",
                    ));
                }
            };
            if query_vector.is_empty() {
                return Err(Status::invalid_argument(
                    "similar.vector.values must not be empty",
                ));
            }
            // NOTE: combining `similar` with `filters` is not yet supported (needs
            // the index metadata-predicate mask, TD-139). Vector mode takes precedence.
            let limit = if req.top_k == 0 {
                10
            } else {
                req.top_k as usize
            };
            let params = GraphFusionParams {
                graph_id: collection.clone(),
                vector_collection: collection.clone(),
                query_vector,
                max_depth: 0, // pure vector entity search; graph expand is a no-op
                edge_types: Vec::new(),
                max_seeds: limit,
                limit,
                vector_weight: 1.0,
                graph_weight: 0.0,
                grain: GraphGrain::Nodes,
                // Entity search is metadata-only today (graph expand is a no-op); within-tenant
                // `permitted_principals` RBAC is not threaded here. `None` ⇒ structural isolation.
                // Threading the caller principal is a follow-up for entity RBAC.
                principal: None,
                policy: FusionPolicy::default(),
            };

            let (items, _stats) = self
                .fusion_service
                .graph_fusion_search(params)
                .await
                .map_err(|e| entity_status("search entities (fusion)", e))?;

            // Project fused vector oids → entity nodes. The seam late-materializes
            // (oid + score only), so fetch each node for its metadata.
            let mut results = Vec::with_capacity(items.len());
            for item in items {
                let node_id = Self::node_id_from_auxiliary_oid(&item.oid).to_owned();
                if let Ok(Some(node)) = self.graph_service.get_node(&collection, &node_id).await {
                    results.push(pv2::EntityResult {
                        entity: Some(node_to_entity(&node, &collection)),
                        score: item.score,
                        debug_info: HashMap::new(),
                    });
                }
            }
            let total = results.len() as u32;
            return Ok(Response::new(pv2::SearchEntitiesResponse {
                results,
                total,
                page_info: None,
                progress: None,
            }));
        }

        // Cases 2 & 3: metadata-filtered or unfiltered node scan.
        let limit = if req.top_k == 0 { 50 } else { req.top_k };
        let mut filters = Vec::new();
        if let Some(meta_filter) = req.filters.as_ref() {
            for clause in &meta_filter.clauses {
                if let Some(value) = filter_clause_to_property_value(clause) {
                    filters.push(PropertyFilter {
                        key: clause.field.clone(),
                        operator: entity_op_to_graph_op(clause.op),
                        value: Some(value),
                    });
                }
            }
        }

        let query = NodeQuery {
            graph_id: collection.clone(),
            labels: vec!["entity".to_string()],
            filters,
            limit: Some(limit),
            offset: None,
            continuation_token: None,
        };

        let nodes = self
            .graph_service
            .query_nodes(&collection, query)
            .await
            .map_err(|e| entity_status("search entities", e))?;

        let prefix = format!("entity:{collection}:");
        let results = nodes
            .into_iter()
            .filter(|n| n.id.starts_with(&prefix))
            .map(|n| pv2::EntityResult {
                entity: Some(node_to_entity(&n, &collection)),
                score: 0.0,
                debug_info: HashMap::new(),
            })
            .collect::<Vec<_>>();
        let total = results.len() as u32;

        Ok(Response::new(pv2::SearchEntitiesResponse {
            results,
            total,
            page_info: None,
            progress: None,
        }))
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Build an entity-shaped v2 [`pv2::Entity`] from a graph node, stripping the
/// internal `_`-prefixed bookkeeping properties.
fn node_to_entity(node: &Node, collection: &str) -> pv2::Entity {
    let mut flexible_metadata = HashMap::new();
    for (k, v) in &node.properties {
        if !k.starts_with('_')
            && let Some(tv) = property_value_to_typed_value(v)
        {
            flexible_metadata.insert(k.clone(), tv);
        }
    }

    let prefix = format!("entity:{collection}:");
    let id = node
        .id
        .strip_prefix(&prefix)
        .unwrap_or(&node.id)
        .to_string();

    pv2::Entity {
        id,
        collection_id: collection.to_string(),
        flexible_metadata,
        embeddings: vec![],
        typed_metadata: None,
        provenance: None,
        relations: vec![],
        temporal: None,
    }
}

/// Create a string graph [`PropertyValue`].
fn str_property(s: impl Into<String>) -> PropertyValue {
    PropertyValue {
        value: Some(GraphValue::StringValue(s.into())),
    }
}

/// Map an [`pv2::EntityModality`] discriminant to a storage modality string.
fn modality_to_string(modality: i32) -> String {
    match modality {
        2 => "image",
        3 => "audio",
        4 => "video",
        5 => "multimodal",
        // UNSPECIFIED (0) and TEXT (1) both default to text.
        _ => "text",
    }
    .to_string()
}

/// Map an [`pv2::EntityComparisonOp`] discriminant to a graph
/// [`crate::graph::PropertyFilterOperator`] discriminant.
fn entity_op_to_graph_op(entity_op: i32) -> i32 {
    use crate::graph::PropertyFilterOperator as Op;
    let mapped = match entity_op {
        1 => Op::Equals,       // EQ
        2 => Op::NotEquals,    // NE
        3 => Op::GreaterThan,  // GT
        4 => Op::GreaterEqual, // GTE
        5 => Op::LessThan,     // LT
        6 => Op::LessEqual,    // LTE
        9 => Op::Contains,     // CONTAINS
        // IN / NOT_IN / UNSPECIFIED default to equals.
        _ => Op::Equals,
    };
    mapped as i32
}

/// Convert v2 `TypedValue` to graph `PropertyValue`.
fn typed_value_to_property_value(tv: &pv2::TypedValue) -> Option<PropertyValue> {
    use pv2::typed_value::Value;

    let value = match &tv.value {
        None => return None,
        Some(Value::TextValue(s)) => Some(GraphValue::StringValue(s.clone())),
        Some(Value::IntegerValue(i)) => Some(GraphValue::IntValue(*i)),
        Some(Value::FloatValue(f)) => Some(GraphValue::DoubleValue(*f)),
        Some(Value::BooleanValue(b)) => Some(GraphValue::BoolValue(*b)),
        Some(Value::TimestampValue(t)) => {
            // Graph has no native timestamp; store as string for round-trip.
            Some(GraphValue::StringValue(t.to_string()))
        }
        Some(Value::UuidValue(u)) | Some(Value::BinaryValue(u)) => {
            Some(GraphValue::BytesValue(u.clone()))
        }
        Some(Value::IsNull(_)) => return None,
        _ => return None, // Arrays / maps are not supported as scalar node props.
    };

    Some(PropertyValue { value })
}

/// Convert graph `PropertyValue` to v2 `TypedValue`.
fn property_value_to_typed_value(pv: &PropertyValue) -> Option<pv2::TypedValue> {
    use pv2::typed_value::Value;

    let value = match &pv.value {
        None => return None,
        Some(GraphValue::StringValue(s)) => Some(Value::TextValue(s.clone())),
        Some(GraphValue::IntValue(i)) => Some(Value::IntegerValue(*i)),
        Some(GraphValue::DoubleValue(f)) => Some(Value::FloatValue(*f)),
        Some(GraphValue::BoolValue(b)) => Some(Value::BooleanValue(*b)),
        Some(GraphValue::BytesValue(b)) => {
            // Heuristic: a 16-byte blob is a UUID, anything else binary.
            if b.len() == 16 {
                Some(Value::UuidValue(b.clone()))
            } else {
                Some(Value::BinaryValue(b.clone()))
            }
        }
        Some(GraphValue::VectorValue(v)) => {
            // Lossy: store the first float only as a scalar marker. Vector-typed
            // node properties are uncommon; full vector data lives in embeddings.
            v.first().map(|f| Value::FloatValue(*f as f64))
        }
        _ => return None, // Array / Object values not surfaced as scalar metadata.
    };

    Some(pv2::TypedValue {
        declared_type: 0,
        value,
    })
}

/// Convert a v2 search `FilterClause` value to a graph `PropertyValue`.
fn filter_clause_to_property_value(clause: &pv2::FilterClause) -> Option<PropertyValue> {
    use pv2::filter_clause::Value as ClauseValue;

    let value = match clause.value.as_ref()? {
        ClauseValue::StringValue(s) => GraphValue::StringValue(s.clone()),
        ClauseValue::IntValue(i) => GraphValue::IntValue(*i),
        ClauseValue::DoubleValue(f) => GraphValue::DoubleValue(*f),
        ClauseValue::BoolValue(b) => GraphValue::BoolValue(*b),
    };

    Some(PropertyValue { value: Some(value) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_value_property_round_trip() {
        use pv2::typed_value::Value;

        let tv = pv2::TypedValue {
            declared_type: 0,
            value: Some(Value::TextValue("test".to_string())),
        };

        let pv = typed_value_to_property_value(&tv).unwrap();
        let back = property_value_to_typed_value(&pv).unwrap();

        match back.value {
            Some(Value::TextValue(s)) => assert_eq!(s, "test"),
            other => panic!("expected TextValue, got {other:?}"),
        }
    }

    #[test]
    fn entity_op_mapping_is_lossless_for_common_ops() {
        use crate::graph::PropertyFilterOperator as Op;
        assert_eq!(entity_op_to_graph_op(1), Op::Equals as i32);
        assert_eq!(entity_op_to_graph_op(2), Op::NotEquals as i32);
        assert_eq!(entity_op_to_graph_op(4), Op::GreaterEqual as i32);
        assert_eq!(entity_op_to_graph_op(9), Op::Contains as i32);
        // Unknown / IN / NOT_IN default to Equals.
        assert_eq!(entity_op_to_graph_op(7), Op::Equals as i32);
    }
}
