// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC V2 entity service — a thin proto adapter over the shared
//! [`EntityOrchestrator`] (`src/services/entity_orchestrator.rs`).
//!
//! The orchestration (graph node + embeddings + provenance + edges, and
//! fusion-delegating search) lives in the orchestrator, shared with the REST
//! facade. This file only converts proto ↔ the orchestrator's neutral types.
//! Per `SEARCH_SURFACE_CONTRACT_2026_06_24.adoc`: retrieval delegates to the
//! fusion seam; this facade owns no ranking. Tenant isolation is structural
//! (the tenant is folded into the backing collection key via `x-tenant-id`).

use std::collections::HashMap;
use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::debug;

use crate::api_handlers::UnifiedHandlers;
use crate::graph::{Node, PropertyFilter, PropertyValue, property_value::Value as GraphValue};
use crate::network::grpc::auth as grpc_auth;
use crate::proto::proximadb_v2 as pv2;
use crate::proto::proximadb_v2::proxima_entity_service_server::{
    ProximaEntityService, ProximaEntityServiceServer,
};
use crate::services::entity_orchestrator::{
    EntityEmbedding, EntityOrchestrator, EntityProvenance, EntityRelation, EntityUpsert,
};
use crate::services::fusion_service::FusionService;

/// gRPC V2 entity service — thin proto adapter over [`EntityOrchestrator`].
pub struct ProximaEntityServiceImpl {
    orchestrator: Arc<EntityOrchestrator>,
}

impl ProximaEntityServiceImpl {
    /// Build from the shared unified request handlers.
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        let graph = request_handlers.graph_operations_service.clone();
        let vector = request_handlers.vector_operations_service.clone();
        let document = request_handlers.document_service.clone();
        let fusion = Arc::new(FusionService::new(vector.clone(), graph.clone()));
        Self {
            orchestrator: Arc::new(EntityOrchestrator::new(graph, vector, fusion, document)),
        }
    }

    /// Convert to a tonic server.
    pub fn into_server(self) -> ProximaEntityServiceServer<Self> {
        ProximaEntityServiceServer::new(self)
    }

    /// Fold the request tenant into the backing collection key (structural isolation).
    fn effective_collection_id<T>(request: &Request<T>, collection_id: &str) -> String {
        match grpc_auth::tenant_id(request) {
            Some(tenant) if !tenant.is_empty() => format!("{tenant}::{collection_id}"),
            _ => collection_id.to_string(),
        }
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

        // Proto → neutral orchestrator input.
        let mut metadata = HashMap::new();
        for (k, v) in &entity.flexible_metadata {
            if let Some(pv) = typed_value_to_property_value(v) {
                metadata.insert(k.clone(), pv);
            }
        }
        let embeddings = entity
            .embeddings
            .iter()
            .map(|e| EntityEmbedding {
                model_id: e.model_id.clone(),
                modality: modality_to_string(e.modality),
                vector: e.vector.clone(),
                dimension: e.dimension,
            })
            .collect::<Vec<_>>();
        let provenance = entity.provenance.as_ref().map(|p| EntityProvenance {
            source_id: p.source_id.clone(),
            chunk_id: p.chunk_id.clone(),
            chunk_position: p.chunk_position,
            extraction_method: p.extraction_method.clone(),
            metadata: p.metadata.clone().into_iter().collect(),
        });
        let relations = entity
            .relations
            .iter()
            .map(|r| EntityRelation {
                source_entity_id: r.source_entity_id.clone(),
                target_entity_id: r.target_entity_id.clone(),
                relation_type: r.relation_type.clone(),
                weight: r.weight,
                properties: r.properties.clone().into_iter().collect(),
            })
            .collect::<Vec<_>>();

        let input = EntityUpsert {
            entity_id: entity.id.clone(),
            metadata,
            embeddings,
            provenance,
            relations,
        };

        debug!("v2 gRPC UpsertEntity collection={collection}");
        let entity_id = self
            .orchestrator
            .upsert(&collection, &tenant_id, input)
            .await
            .map_err(|e| entity_status("upsert entity", e))?;

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

        let node = self
            .orchestrator
            .get(&collection, &req.entity_id)
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

        let deleted = self
            .orchestrator
            .delete(&collection, &req.entity_id)
            .await
            .map_err(|e| entity_status("delete entity", e))?;

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

        // Vector mode: only `similar.vector` is supported today.
        let query_vector = match req.similar.as_ref() {
            Some(s) => match s.query.as_ref() {
                Some(pv2::similar_query::Query::Vector(v)) => {
                    if v.values.is_empty() {
                        return Err(Status::invalid_argument(
                            "similar.vector.values must not be empty",
                        ));
                    }
                    Some(v.values.clone())
                }
                _ => {
                    return Err(Status::unimplemented(
                        "Only `similar.vector` is supported; text/raw_data embedding is not yet wired.",
                    ));
                }
            },
            None => None,
        };

        // Metadata filters → graph PropertyFilters.
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

        let hits = self
            .orchestrator
            .search(&collection, query_vector, filters, req.top_k as usize)
            .await
            .map_err(|e| entity_status("search entities", e))?;

        let results = hits
            .into_iter()
            .map(|hit| pv2::EntityResult {
                entity: Some(node_to_entity(&hit.node, &collection)),
                score: hit.score,
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
// Proto ↔ neutral conversion helpers (proto-specific; stay in this facade)
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

/// Map an [`pv2::EntityModality`] discriminant to a storage modality string.
fn modality_to_string(modality: i32) -> String {
    match modality {
        2 => "image",
        3 => "audio",
        4 => "video",
        5 => "multimodal",
        _ => "text", // UNSPECIFIED (0) and TEXT (1)
    }
    .to_string()
}

/// Map an [`pv2::EntityComparisonOp`] discriminant to a graph
/// [`crate::graph::PropertyFilterOperator`] discriminant.
fn entity_op_to_graph_op(entity_op: i32) -> i32 {
    use crate::graph::PropertyFilterOperator as Op;
    let mapped = match entity_op {
        1 => Op::Equals,
        2 => Op::NotEquals,
        3 => Op::GreaterThan,
        4 => Op::GreaterEqual,
        5 => Op::LessThan,
        6 => Op::LessEqual,
        9 => Op::Contains,
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
        Some(Value::TimestampValue(t)) => Some(GraphValue::StringValue(t.to_string())),
        Some(Value::UuidValue(u)) | Some(Value::BinaryValue(u)) => {
            Some(GraphValue::BytesValue(u.clone()))
        }
        Some(Value::IsNull(_)) => return None,
        _ => return None,
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
            if b.len() == 16 {
                Some(Value::UuidValue(b.clone()))
            } else {
                Some(Value::BinaryValue(b.clone()))
            }
        }
        Some(GraphValue::VectorValue(v)) => v.first().map(|f| Value::FloatValue(*f as f64)),
        _ => return None,
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
        assert_eq!(entity_op_to_graph_op(7), Op::Equals as i32);
    }
}
