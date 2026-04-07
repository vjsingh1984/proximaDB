//! REST API Wrapper Types
//!
//! This module provides REST-compatible wrapper types for protobuf messages.
//! These wrappers implement Serialize/Deserialize for JSON conversion while
//! keeping protobuf types pure (without serde derives).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Not;
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::{Node, PropertyValue, Edge};

/// REST-compatible wrapper for VectorOperationResponse
#[derive(Debug, Serialize, Deserialize)]
pub struct RestVectorOperationResponse {
    pub success: bool,
    pub message: String,
    pub request_id: Option<String>,
    pub processed_count: u32,
    pub search_results: Option<Vec<RestSearchResult>>,
    pub collection_info: Option<RestCollectionInfo>,
    pub execution_time_ms: u64,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// REST-compatible wrapper for SearchResult
#[derive(Debug, Serialize, Deserialize)]
pub struct RestSearchResult {
    pub id: String,
    pub score: f32,
    pub vector: Option<Vec<f32>>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub collection_id: Option<String>,
}

/// REST-compatible wrapper for CollectionInfo
#[derive(Debug, Serialize, Deserialize)]
pub struct RestCollectionInfo {
    pub id: String,
    pub name: String,
    pub dimension: u32,
    pub vector_count: u64,
    pub index_type: String,
    pub distance_metric: String,
    pub created_at: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// REST-compatible wrapper for ExecuteSqlResponse
#[derive(Debug, Serialize, Deserialize)]
pub struct RestExecuteSqlResponse {
    pub success: bool,
    pub message: String,
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    pub columns: Vec<RestColumnInfo>,
    pub rows_affected: u64,
    pub execution_time_ms: u64,
    pub query_plan: Option<serde_json::Value>,
}

/// REST-compatible wrapper for column information
#[derive(Debug, Serialize, Deserialize)]
pub struct RestColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

impl From<proximadb_v1::VectorOperationResponse> for RestVectorOperationResponse {
    fn from(proto: proximadb_v1::VectorOperationResponse) -> Self {
        Self {
            success: proto.success,
            message: proto.message,
            request_id: proto.request_id.is_empty().not().then(|| proto.request_id),
            processed_count: proto.processed_count,
            search_results: proto.search_results.into_iter().map(RestSearchResult::from).collect::<Vec<_>>().into(),
            collection_info: proto.collection_info.map(RestCollectionInfo::from),
            execution_time_ms: proto.execution_time_ms,
            metadata: convert_proto_metadata_to_json(proto.metadata),
        }
    }
}

impl From<proximadb_v1::SearchResult> for RestSearchResult {
    fn from(proto: proximadb_v1::SearchResult) -> Self {
        Self {
            id: proto.id,
            score: proto.score,
            vector: proto.vector.is_empty().not().then(|| proto.vector),
            metadata: convert_proto_metadata_to_json(proto.metadata),
            collection_id: proto.collection_id.is_empty().not().then(|| proto.collection_id),
        }
    }
}

impl From<proximadb_v1::CollectionInfo> for RestCollectionInfo {
    fn from(proto: proximadb_v1::CollectionInfo) -> Self {
        Self {
            id: proto.id,
            name: proto.name,
            dimension: proto.dimension,
            vector_count: proto.vector_count,
            index_type: proto.index_type,
            distance_metric: proto.distance_metric,
            created_at: proto.created_at.is_empty().not().then(|| proto.created_at),
            metadata: convert_proto_metadata_to_json(proto.metadata),
        }
    }
}

impl From<proximadb_v1::ExecuteSqlResponse> for RestExecuteSqlResponse {
    fn from(proto: proximadb_v1::ExecuteSqlResponse) -> Self {
        Self {
            success: proto.success,
            message: proto.message,
            rows: proto.rows.into_iter().map(|row| {
                let mut map = HashMap::new();
                for (key, value) in row.fields {
                    map.insert(key, sql_value_to_json(&value));
                }
                map
            }).collect(),
            columns: proto.columns.into_iter().map(|col| RestColumnInfo {
                name: col.name,
                data_type: col.data_type,
                nullable: col.nullable,
            }).collect(),
            rows_affected: proto.rows_affected,
            execution_time_ms: proto.execution_time_ms,
            query_plan: proto.query_plan.map(|plan| serde_json::json!(plan)),
        }
    }
}

/// Convert proto metadata to JSON HashMap
fn convert_proto_metadata_to_json(metadata: HashMap<String, proximadb_v1::SqlValue>) -> HashMap<String, serde_json::Value> {
    metadata.into_iter()
        .map(|(key, value)| (key, sql_value_to_json(&value)))
        .collect()
}

/// Convert SqlValue to serde_json::Value
pub fn sql_value_to_json(value: &proximadb_v1::SqlValue) -> serde_json::Value {
    use proximadb_v1::sql_value::Value;
    match value.value.as_ref() {
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::NumberValue(n)) => serde_json::Value::Number(
            serde_json::Number::from_f64(*n).unwrap_or(serde_json::Number::from(0))
        ),
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(Value::BytesValue(b)) => {
            serde_json::Value::Array(
                b.iter().map(|x| serde_json::Value::Number((*x as u64).into())).collect()
            )
        },
        Some(Value::NullValue(_)) => serde_json::Value::Null,
        Some(Value::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(sql_value_to_json).collect())
        },
        Some(Value::ObjectValue(obj)) => {
            let mut map = serde_json::Map::new();
            for (k, sv) in &obj.fields {
                map.insert(k.clone(), sql_value_to_json(sv));
            }
            serde_json::Value::Object(map)
        },
        None => serde_json::Value::Null,
    }
}

/// Convert Node proto to JSON for serialization
pub fn node_to_json(node: &Node) -> serde_json::Value {
    let mut properties_map = serde_json::Map::new();
    for (key, prop_value) in &node.properties {
        properties_map.insert(key.clone(), property_value_to_json(prop_value));
    }

    serde_json::json!({
        "id": node.id,
        "labels": node.labels,
        "properties": properties_map,
        "embedding": node.embedding.is_empty().not().then(|| &node.embedding)
    })
}

/// Convert PropertyValue to JSON
fn property_value_to_json(prop: &PropertyValue) -> serde_json::Value {
    // PropertyValue is now a struct, not enum - use direct field access;
    match prop.value.as_ref() {
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::IntValue(i)) => serde_json::Value::Number((*i).into()),
        Some(Value::FloatValue(f)) => serde_json::Value::Number(
            serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0))
        ),
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::BytesValue(b)) => serde_json::Value::Array(
            b.iter().map(|x| serde_json::Value::Number((*x as u64).into())).collect()
        ),
        Some(Value::ListValue(list)) => serde_json::Value::Array(
            list.values.iter().map(property_value_to_json).collect()
        ),
        None => serde_json::Value::Null,
    }
}

/// Convert Edge proto to JSON for serialization
pub fn edge_to_json(edge: &Edge) -> serde_json::Value {
    let mut properties_map = serde_json::Map::new();
    for (key, prop_value) in &edge.properties {
        properties_map.insert(key.clone(), property_value_to_json(prop_value));
    }

    serde_json::json!({
        "id": edge.id,
        "from_node_id": edge.from_node_id,
        "to_node_id": edge.to_node_id,
        "edge_type": edge.edge_type,
        "properties": properties_map,
        "weight": edge.weight,
        "created_at": edge.created_at.is_empty().not().then(|| &edge.created_at),
        "updated_at": edge.updated_at.is_empty().not().then(|| &edge.updated_at)
    })
}