// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Columnar Arrow codec for the batched graph path over Arrow Flight.
//!
//! Bulk graph ingest/export is a *columnar* workload, so it belongs on Arrow
//! Flight (zero-copy) rather than per-row unary gRPC. This module is the
//! neutral-graph-model <-> Arrow `RecordBatch` boundary: it builds a stable
//! node/edge batch schema from [`crate::graph::model::{Node, Edge}`] and reads
//! it back. The Flight handlers route decoded batches to
//! `GraphOperationsService` batch APIs (so the rows land in the live graph
//! engine, not the generic record store) and stream query results back.
//!
//! ## Schema
//!
//! Node batch (`embedding_dim > 0` adds the two embedding columns):
//! - `id: Utf8`, `labels: Utf8` (JSON array), `properties: Utf8` (JSON object),
//!   `created_at_ms: Int64`, `updated_at_ms: Int64`
//! - `embedding_vector: FixedSizeList<Float32, dim>` (nullable),
//!   `embedding_meta: Utf8` (JSON; model id/version/dimension/params/modality)
//!
//! Edge batch:
//! - `id, from_node_id, to_node_id, edge_type: Utf8`, `weight: Float64`
//!   (nullable), `properties: Utf8` (JSON), `created_at_ms: Int64`,
//!   `updated_at_ms: Int64`
//!
//! Heterogeneous node/edge properties (`HashMap<String, PropertyValue>`) are
//! carried as a single JSON `Utf8` column via the neutral model's serde — the
//! same convention the federated executor and `multimodel_codec` already use.
//! The embedding *vector* is the one payload kept native-columnar (a
//! `FixedSizeList<Float32>` child buffer) for throughput; its metadata rides
//! alongside as JSON.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Float64Array, Int64Array, RecordBatch,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};

use crate::graph::model::{Edge, EmbeddingVersion, Node};

/// Sidecar for the embedding columns: everything in [`EmbeddingVersion`] except
/// the vector itself, which lives in the native `embedding_vector` column.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct EmbeddingMeta {
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    model_version: String,
    #[serde(default)]
    dimension: u32,
    #[serde(default)]
    created_at_ms: i64,
    #[serde(default)]
    model_params: std::collections::HashMap<String, String>,
    #[serde(default)]
    modality: i32,
}

impl EmbeddingMeta {
    fn from_embedding(e: &EmbeddingVersion) -> Self {
        Self {
            model_id: e.model_id.clone(),
            model_version: e.model_version.clone(),
            dimension: e.dimension,
            created_at_ms: e.created_at_ms,
            model_params: e.model_params.clone(),
            modality: e.modality,
        }
    }
}

// ── Schemas ─────────────────────────────────────────────────────────────────

/// Stable Arrow schema for a graph-node batch. When `embedding_dim == 0` the
/// two embedding columns are omitted (nodes without vectors).
pub fn graph_node_schema(embedding_dim: usize) -> Arc<Schema> {
    let mut fields = vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("labels", DataType::Utf8, false),
        Field::new("properties", DataType::Utf8, false),
        Field::new("created_at_ms", DataType::Int64, false),
        Field::new("updated_at_ms", DataType::Int64, false),
    ];
    if embedding_dim > 0 {
        fields.push(Field::new(
            "embedding_vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                embedding_dim as i32,
            ),
            true,
        ));
        fields.push(Field::new("embedding_meta", DataType::Utf8, true));
    }
    Arc::new(Schema::new(fields))
}

/// Stable Arrow schema for a graph-edge batch.
pub fn graph_edge_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("from_node_id", DataType::Utf8, false),
        Field::new("to_node_id", DataType::Utf8, false),
        Field::new("edge_type", DataType::Utf8, false),
        Field::new("weight", DataType::Float64, true),
        Field::new("properties", DataType::Utf8, false),
        Field::new("created_at_ms", DataType::Int64, false),
        Field::new("updated_at_ms", DataType::Int64, false),
    ]))
}

/// Determine the embedding dimension for a node batch: the dimension of the
/// first node that carries an embedding. Returns 0 when no node has one.
/// Errors if two nodes carry embeddings of differing length (a single
/// `FixedSizeList` batch is one stride wide — split mixed dims per graph).
///
/// Exposed so a streaming export can fix the schema dimension from the first
/// page and reuse it for every subsequent page (one consistent Arrow schema
/// across the whole DoGet stream).
pub fn embedding_dim_of(nodes: &[Node]) -> Result<usize> {
    batch_embedding_dim(nodes)
}

fn batch_embedding_dim(nodes: &[Node]) -> Result<usize> {
    let mut dim: Option<usize> = None;
    for n in nodes {
        if let Some(e) = &n.embedding {
            let len = e.vector.len();
            if len == 0 {
                continue;
            }
            match dim {
                None => dim = Some(len),
                Some(d) if d != len => bail!(
                    "graph node batch carries mixed embedding dimensions ({d} vs {len}); \
                     a columnar batch is single-stride"
                ),
                _ => {}
            }
        }
    }
    Ok(dim.unwrap_or(0))
}

// ── Node encode / decode ────────────────────────────────────────────────────

/// Encode neutral graph nodes into a columnar Arrow [`RecordBatch`], deriving
/// the embedding dimension from the batch.
pub fn nodes_to_batch(nodes: &[Node]) -> Result<RecordBatch> {
    let embedding_dim = batch_embedding_dim(nodes)?;
    nodes_to_batch_with_dim(nodes, embedding_dim)
}

/// Encode nodes with an explicit, caller-fixed embedding dimension. Used by the
/// streaming export so every page shares one schema; a node whose embedding
/// length differs from `embedding_dim` is rejected (mixed strides can't share a
/// `FixedSizeList` column), and a node without an embedding emits a null list.
pub fn nodes_to_batch_with_dim(nodes: &[Node], embedding_dim: usize) -> Result<RecordBatch> {
    let schema = graph_node_schema(embedding_dim);

    let ids = StringArray::from_iter_values(nodes.iter().map(|n| n.id.as_str()));
    let labels = StringArray::from(
        nodes
            .iter()
            .map(|n| serde_json::to_string(&n.labels).ok())
            .collect::<Vec<_>>(),
    );
    let properties = StringArray::from(
        nodes
            .iter()
            .map(|n| serde_json::to_string(&n.properties).ok())
            .collect::<Vec<_>>(),
    );
    let created = Int64Array::from_iter_values(nodes.iter().map(|n| n.created_at_ms));
    let updated = Int64Array::from_iter_values(nodes.iter().map(|n| n.updated_at_ms));

    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(ids),
        Arc::new(labels),
        Arc::new(properties),
        Arc::new(created),
        Arc::new(updated),
    ];

    if embedding_dim > 0 {
        // Native FixedSizeList<Float32> child buffer must hold exactly
        // len*dim floats; rows without an embedding emit a NULL list but still
        // reserve `dim` backing slots (mirrors the vector-search codec).
        let mut flat = Vec::with_capacity(nodes.len() * embedding_dim);
        let mut present = Vec::with_capacity(nodes.len());
        let mut metas: Vec<Option<String>> = Vec::with_capacity(nodes.len());
        for n in nodes {
            match &n.embedding {
                Some(e) if e.vector.len() == embedding_dim => {
                    flat.extend_from_slice(&e.vector);
                    present.push(true);
                    metas.push(serde_json::to_string(&EmbeddingMeta::from_embedding(e)).ok());
                }
                Some(e) if !e.vector.is_empty() => bail!(
                    "node `{}` embedding dimension {} does not match the batch dimension {}",
                    n.id,
                    e.vector.len(),
                    embedding_dim
                ),
                _ => {
                    flat.extend(std::iter::repeat_n(0.0f32, embedding_dim));
                    present.push(false);
                    metas.push(None);
                }
            }
        }
        let child = Arc::new(Float32Array::from(flat)) as ArrayRef;
        let item_field = Arc::new(Field::new("item", DataType::Float32, false));
        let nulls = present
            .iter()
            .any(|p| !p)
            .then(|| arrow_buffer::NullBuffer::from(present));
        let vector_array = FixedSizeListArray::new(item_field, embedding_dim as i32, child, nulls);
        columns.push(Arc::new(vector_array));
        columns.push(Arc::new(StringArray::from(metas)));
    }

    RecordBatch::try_new(schema, columns).context("build graph node RecordBatch")
}

/// Decode a graph-node [`RecordBatch`] back into neutral nodes.
pub fn batch_to_nodes(batch: &RecordBatch) -> Result<Vec<Node>> {
    let n = batch.num_rows();
    let ids = str_col(batch, "id")?;
    let labels = str_col(batch, "labels")?;
    let properties = str_col(batch, "properties")?;
    let created = i64_col(batch, "created_at_ms")?;
    let updated = i64_col(batch, "updated_at_ms")?;

    // Embedding columns are optional (absent when no node carried a vector).
    let embedding = batch
        .schema()
        .column_with_name("embedding_vector")
        .map(|(idx, _)| {
            batch
                .column(idx)
                .as_any()
                .downcast_ref::<FixedSizeListArray>()
                .ok_or_else(|| anyhow!("embedding_vector column is not a FixedSizeList"))
                .cloned()
        })
        .transpose()?;
    let embedding_meta = batch
        .schema()
        .column_with_name("embedding_meta")
        .map(|_| str_col(batch, "embedding_meta"))
        .transpose()?;

    let mut out = Vec::with_capacity(n);
    for row in 0..n {
        let labels_json = labels.value(row);
        let props_json = properties.value(row);
        let node_embedding = match &embedding {
            Some(col) if !col.is_null(row) => {
                let values = col.value(row);
                let f32s = values
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow!("embedding_vector child is not Float32"))?;
                let vector: Vec<f32> = f32s.values().to_vec();
                let meta: EmbeddingMeta = match &embedding_meta {
                    Some(m) if !m.is_null(row) => {
                        serde_json::from_str(m.value(row)).context("parse embedding_meta JSON")?
                    }
                    _ => EmbeddingMeta {
                        dimension: vector.len() as u32,
                        ..Default::default()
                    },
                };
                Some(EmbeddingVersion {
                    model_id: meta.model_id,
                    model_version: meta.model_version,
                    vector,
                    dimension: meta.dimension,
                    created_at_ms: meta.created_at_ms,
                    model_params: meta.model_params,
                    modality: meta.modality,
                })
            }
            _ => None,
        };
        out.push(Node {
            id: ids.value(row).to_string(),
            labels: serde_json::from_str(labels_json).context("parse node labels JSON")?,
            properties: serde_json::from_str(props_json).context("parse node properties JSON")?,
            embedding: node_embedding,
            created_at_ms: created.value(row),
            updated_at_ms: updated.value(row),
        });
    }
    Ok(out)
}

// ── Edge encode / decode ────────────────────────────────────────────────────

/// Encode neutral graph edges into a columnar Arrow [`RecordBatch`].
pub fn edges_to_batch(edges: &[Edge]) -> Result<RecordBatch> {
    let schema = graph_edge_schema();
    let ids = StringArray::from_iter_values(edges.iter().map(|e| e.id.as_str()));
    let from = StringArray::from_iter_values(edges.iter().map(|e| e.from_node_id.as_str()));
    let to = StringArray::from_iter_values(edges.iter().map(|e| e.to_node_id.as_str()));
    let etype = StringArray::from_iter_values(edges.iter().map(|e| e.edge_type.as_str()));
    let weight = Float64Array::from(edges.iter().map(|e| e.weight).collect::<Vec<_>>());
    let properties = StringArray::from(
        edges
            .iter()
            .map(|e| serde_json::to_string(&e.properties).ok())
            .collect::<Vec<_>>(),
    );
    let created = Int64Array::from_iter_values(edges.iter().map(|e| e.created_at_ms));
    let updated = Int64Array::from_iter_values(edges.iter().map(|e| e.updated_at_ms));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ids),
            Arc::new(from),
            Arc::new(to),
            Arc::new(etype),
            Arc::new(weight),
            Arc::new(properties),
            Arc::new(created),
            Arc::new(updated),
        ],
    )
    .context("build graph edge RecordBatch")
}

/// Decode a graph-edge [`RecordBatch`] back into neutral edges.
pub fn batch_to_edges(batch: &RecordBatch) -> Result<Vec<Edge>> {
    let n = batch.num_rows();
    let ids = str_col(batch, "id")?;
    let from = str_col(batch, "from_node_id")?;
    let to = str_col(batch, "to_node_id")?;
    let etype = str_col(batch, "edge_type")?;
    let properties = str_col(batch, "properties")?;
    let created = i64_col(batch, "created_at_ms")?;
    let updated = i64_col(batch, "updated_at_ms")?;
    let weight = batch
        .column(
            batch
                .schema()
                .column_with_name("weight")
                .ok_or_else(|| anyhow!("missing edge weight column"))?
                .0,
        )
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| anyhow!("edge weight column is not Float64"))?
        .clone();

    let mut out = Vec::with_capacity(n);
    for row in 0..n {
        out.push(Edge {
            id: ids.value(row).to_string(),
            from_node_id: from.value(row).to_string(),
            to_node_id: to.value(row).to_string(),
            edge_type: etype.value(row).to_string(),
            properties: serde_json::from_str(properties.value(row))
                .context("parse edge properties JSON")?,
            weight: (!weight.is_null(row)).then(|| weight.value(row)),
            created_at_ms: created.value(row),
            updated_at_ms: updated.value(row),
        });
    }
    Ok(out)
}

// ── Column accessors ────────────────────────────────────────────────────────

fn str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    let (idx, _) = batch
        .schema()
        .column_with_name(name)
        .ok_or_else(|| anyhow!("graph batch missing required column `{name}`"))?;
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow!("graph batch column `{name}` is not Utf8"))
}

fn i64_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    let (idx, _) = batch
        .schema()
        .column_with_name(name)
        .ok_or_else(|| anyhow!("graph batch missing required column `{name}`"))?;
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| anyhow!("graph batch column `{name}` is not Int64"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{PropertyValue, property_value::Value};

    fn sample_node(id: &str, dim: usize) -> Node {
        let mut properties = std::collections::HashMap::new();
        properties.insert(
            "name".to_string(),
            PropertyValue {
                value: Some(Value::StringValue(format!("name-{id}"))),
            },
        );
        properties.insert(
            "score".to_string(),
            PropertyValue {
                value: Some(Value::DoubleValue(1.5)),
            },
        );
        Node {
            id: id.to_string(),
            labels: vec!["Person".to_string(), "Author".to_string()],
            properties,
            embedding: (dim > 0).then(|| EmbeddingVersion {
                model_id: "bge".to_string(),
                model_version: "v1".to_string(),
                vector: (0..dim).map(|i| i as f32 * 0.1).collect(),
                dimension: dim as u32,
                created_at_ms: 7,
                model_params: Default::default(),
                modality: 0,
            }),
            created_at_ms: 11,
            updated_at_ms: 22,
        }
    }

    #[test]
    fn node_round_trip_with_embeddings() {
        let nodes = vec![sample_node("n1", 4), sample_node("n2", 4)];
        let batch = nodes_to_batch(&nodes).expect("encode");
        assert_eq!(batch.num_rows(), 2);
        let back = batch_to_nodes(&batch).expect("decode");
        assert_eq!(back, nodes);
    }

    #[test]
    fn node_round_trip_mixed_presence() {
        // One node with an embedding, one without — null list row.
        let nodes = vec![sample_node("n1", 3), sample_node("n2", 0)];
        let batch = nodes_to_batch(&nodes).expect("encode");
        let back = batch_to_nodes(&batch).expect("decode");
        assert_eq!(back, nodes);
        assert!(back[1].embedding.is_none());
    }

    #[test]
    fn node_round_trip_no_embeddings_omits_columns() {
        let nodes = vec![sample_node("n1", 0)];
        let batch = nodes_to_batch(&nodes).expect("encode");
        assert!(
            batch
                .schema()
                .column_with_name("embedding_vector")
                .is_none()
        );
        let back = batch_to_nodes(&batch).expect("decode");
        assert_eq!(back, nodes);
    }

    #[test]
    fn mixed_embedding_dims_rejected() {
        let nodes = vec![sample_node("n1", 3), sample_node("n2", 4)];
        assert!(nodes_to_batch(&nodes).is_err());
    }

    #[test]
    fn edge_round_trip() {
        let mut props = std::collections::HashMap::new();
        props.insert(
            "since".to_string(),
            PropertyValue {
                value: Some(Value::IntValue(2020)),
            },
        );
        let edges = vec![
            Edge {
                id: "e1".to_string(),
                from_node_id: "n1".to_string(),
                to_node_id: "n2".to_string(),
                edge_type: "KNOWS".to_string(),
                properties: props,
                weight: Some(0.9),
                created_at_ms: 1,
                updated_at_ms: 2,
            },
            Edge {
                id: "e2".to_string(),
                from_node_id: "n2".to_string(),
                to_node_id: "n1".to_string(),
                edge_type: "CITES".to_string(),
                properties: std::collections::HashMap::new(),
                weight: None,
                created_at_ms: 3,
                updated_at_ms: 4,
            },
        ];
        let batch = edges_to_batch(&edges).expect("encode");
        let back = batch_to_edges(&batch).expect("decode");
        assert_eq!(back, edges);
    }
}
