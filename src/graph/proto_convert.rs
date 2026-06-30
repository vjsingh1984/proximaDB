// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Boundary conversions between the `proximadb.v1` protobuf (wire) graph types
//! and the neutral [`crate::graph::model`] domain types (TD-123 Step 1).
//!
//! The graph engine/services speak only the neutral model; the v1 gRPC and REST
//! adapters use these `From` impls to translate at their boundary. (The v2 gRPC
//! surface converts its own `proximadb.v2` messages to the neutral model
//! directly in `src/network/grpc/v2/graph_service.rs`.)
//!
//! Most conversions are field-for-field. Two are lossy by design:
//! - property `VectorValue`: proto `VectorData` <-> neutral `Vec<f32>`.
//! - [`GraphPath`]: proto `{entities, relations}` (entity.proto) <-> neutral
//!   `{node_ids, edge_ids}`. proto->neutral keeps entity ids; neutral->proto
//!   rebuilds bare `Entity{id}` and drops relation detail (v1 is deprecated).

use crate::graph::model as m;
use crate::proto::proximadb_v1 as pb;

// ── PropertyValue ───────────────────────────────────────────────────────────

impl From<pb::PropertyValue> for m::PropertyValue {
    fn from(p: pb::PropertyValue) -> Self {
        m::PropertyValue {
            value: p.value.map(Into::into),
        }
    }
}
impl From<m::PropertyValue> for pb::PropertyValue {
    fn from(p: m::PropertyValue) -> Self {
        pb::PropertyValue {
            value: p.value.map(Into::into),
        }
    }
}

impl From<pb::property_value::Value> for m::property_value::Value {
    fn from(v: pb::property_value::Value) -> Self {
        use m::property_value::Value as N;
        use pb::property_value::Value as P;
        match v {
            P::StringValue(s) => N::StringValue(s),
            P::IntValue(i) => N::IntValue(i),
            P::DoubleValue(d) => N::DoubleValue(d),
            P::BoolValue(b) => N::BoolValue(b),
            P::BytesValue(b) => N::BytesValue(b),
            P::ArrayValue(a) => N::ArrayValue(a.into()),
            P::ObjectValue(o) => N::ObjectValue(o.into()),
            P::VectorValue(vd) => N::VectorValue(vd.values),
        }
    }
}
impl From<m::property_value::Value> for pb::property_value::Value {
    fn from(v: m::property_value::Value) -> Self {
        use m::property_value::Value as N;
        use pb::property_value::Value as P;
        match v {
            N::StringValue(s) => P::StringValue(s),
            N::IntValue(i) => P::IntValue(i),
            N::DoubleValue(d) => P::DoubleValue(d),
            N::BoolValue(b) => P::BoolValue(b),
            N::BytesValue(b) => P::BytesValue(b),
            N::ArrayValue(a) => P::ArrayValue(a.into()),
            N::ObjectValue(o) => P::ObjectValue(o.into()),
            N::VectorValue(v) => P::VectorValue(pb::VectorData { values: v }),
        }
    }
}

impl From<pb::PropertyArray> for m::PropertyArray {
    fn from(a: pb::PropertyArray) -> Self {
        m::PropertyArray {
            values: a.values.into_iter().map(Into::into).collect(),
        }
    }
}
impl From<m::PropertyArray> for pb::PropertyArray {
    fn from(a: m::PropertyArray) -> Self {
        pb::PropertyArray {
            values: a.values.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<pb::PropertyObject> for m::PropertyObject {
    fn from(o: pb::PropertyObject) -> Self {
        m::PropertyObject {
            fields: o.fields.into_iter().map(|(k, v)| (k, v.into())).collect(),
        }
    }
}
impl From<m::PropertyObject> for pb::PropertyObject {
    fn from(o: m::PropertyObject) -> Self {
        pb::PropertyObject {
            fields: o.fields.into_iter().map(|(k, v)| (k, v.into())).collect(),
        }
    }
}

// ── EmbeddingVersion ────────────────────────────────────────────────────────

impl From<pb::EmbeddingVersion> for m::EmbeddingVersion {
    fn from(e: pb::EmbeddingVersion) -> Self {
        m::EmbeddingVersion {
            model_id: e.model_id,
            model_version: e.model_version,
            vector: e.vector,
            dimension: e.dimension,
            created_at_ms: e.created_at_ms,
            model_params: e.model_params,
            modality: e.modality,
        }
    }
}
impl From<m::EmbeddingVersion> for pb::EmbeddingVersion {
    fn from(e: m::EmbeddingVersion) -> Self {
        pb::EmbeddingVersion {
            model_id: e.model_id,
            model_version: e.model_version,
            vector: e.vector,
            dimension: e.dimension,
            created_at_ms: e.created_at_ms,
            model_params: e.model_params,
            modality: e.modality,
        }
    }
}

// ── Node / Edge ─────────────────────────────────────────────────────────────

impl From<pb::Node> for m::Node {
    fn from(n: pb::Node) -> Self {
        m::Node {
            id: n.id,
            labels: n.labels,
            properties: n
                .properties
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            embedding: n.embedding.map(Into::into),
            created_at_ms: n.created_at_ms,
            updated_at_ms: n.updated_at_ms,
        }
    }
}
impl From<m::Node> for pb::Node {
    fn from(n: m::Node) -> Self {
        pb::Node {
            id: n.id,
            labels: n.labels,
            properties: n
                .properties
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            embedding: n.embedding.map(Into::into),
            created_at_ms: n.created_at_ms,
            updated_at_ms: n.updated_at_ms,
        }
    }
}

impl From<pb::Edge> for m::Edge {
    fn from(e: pb::Edge) -> Self {
        m::Edge {
            id: e.id,
            from_node_id: e.from_node_id,
            to_node_id: e.to_node_id,
            edge_type: e.edge_type,
            properties: e
                .properties
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            weight: e.weight,
            created_at_ms: e.created_at_ms,
            updated_at_ms: e.updated_at_ms,
        }
    }
}
impl From<m::Edge> for pb::Edge {
    fn from(e: m::Edge) -> Self {
        pb::Edge {
            id: e.id,
            from_node_id: e.from_node_id,
            to_node_id: e.to_node_id,
            edge_type: e.edge_type,
            properties: e
                .properties
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            weight: e.weight,
            created_at_ms: e.created_at_ms,
            updated_at_ms: e.updated_at_ms,
        }
    }
}

// ── PropertyFilter / queries ────────────────────────────────────────────────

impl From<pb::PropertyFilter> for m::PropertyFilter {
    fn from(f: pb::PropertyFilter) -> Self {
        m::PropertyFilter {
            key: f.key,
            operator: f.operator,
            value: f.value.map(Into::into),
        }
    }
}
impl From<m::PropertyFilter> for pb::PropertyFilter {
    fn from(f: m::PropertyFilter) -> Self {
        pb::PropertyFilter {
            key: f.key,
            operator: f.operator,
            value: f.value.map(Into::into),
        }
    }
}

impl From<pb::NodeQuery> for m::NodeQuery {
    fn from(q: pb::NodeQuery) -> Self {
        m::NodeQuery {
            graph_id: q.graph_id,
            labels: q.labels,
            filters: q.filters.into_iter().map(Into::into).collect(),
            limit: q.limit,
            offset: q.offset,
            continuation_token: q.continuation_token,
        }
    }
}
impl From<m::NodeQuery> for pb::NodeQuery {
    fn from(q: m::NodeQuery) -> Self {
        pb::NodeQuery {
            graph_id: q.graph_id,
            labels: q.labels,
            filters: q.filters.into_iter().map(Into::into).collect(),
            limit: q.limit,
            offset: q.offset,
            continuation_token: q.continuation_token,
        }
    }
}

impl From<pb::EdgeQuery> for m::EdgeQuery {
    fn from(q: pb::EdgeQuery) -> Self {
        m::EdgeQuery {
            graph_id: q.graph_id,
            from_node_id: q.from_node_id,
            to_node_id: q.to_node_id,
            edge_types: q.edge_types,
            filters: q.filters.into_iter().map(Into::into).collect(),
            limit: q.limit,
            offset: q.offset,
            continuation_token: q.continuation_token,
        }
    }
}
impl From<m::EdgeQuery> for pb::EdgeQuery {
    fn from(q: m::EdgeQuery) -> Self {
        pb::EdgeQuery {
            graph_id: q.graph_id,
            from_node_id: q.from_node_id,
            to_node_id: q.to_node_id,
            edge_types: q.edge_types,
            filters: q.filters.into_iter().map(Into::into).collect(),
            limit: q.limit,
            offset: q.offset,
            continuation_token: q.continuation_token,
        }
    }
}

// ── Traversal ───────────────────────────────────────────────────────────────

impl From<pb::TraversalRequest> for m::TraversalRequest {
    fn from(r: pb::TraversalRequest) -> Self {
        m::TraversalRequest {
            graph_id: r.graph_id,
            start_node_id: r.start_node_id,
            max_depth: r.max_depth,
            edge_types: r.edge_types,
            node_labels: r.node_labels,
            filters: r.filters.into_iter().map(Into::into).collect(),
            algorithm: r.algorithm,
            limit: r.limit,
            timeout_ms: r.timeout_ms,
            max_frontier: r.max_frontier,
        }
    }
}
impl From<m::TraversalRequest> for pb::TraversalRequest {
    fn from(r: m::TraversalRequest) -> Self {
        pb::TraversalRequest {
            graph_id: r.graph_id,
            start_node_id: r.start_node_id,
            max_depth: r.max_depth,
            edge_types: r.edge_types,
            node_labels: r.node_labels,
            filters: r.filters.into_iter().map(Into::into).collect(),
            algorithm: r.algorithm,
            limit: r.limit,
            timeout_ms: r.timeout_ms,
            max_frontier: r.max_frontier,
        }
    }
}

impl From<pb::TraversalStats> for m::TraversalStats {
    fn from(s: pb::TraversalStats) -> Self {
        m::TraversalStats {
            nodes_visited: s.nodes_visited,
            edges_traversed: s.edges_traversed,
            max_depth_reached: s.max_depth_reached,
            execution_time_microseconds: s.execution_time_microseconds,
        }
    }
}
impl From<m::TraversalStats> for pb::TraversalStats {
    fn from(s: m::TraversalStats) -> Self {
        pb::TraversalStats {
            nodes_visited: s.nodes_visited,
            edges_traversed: s.edges_traversed,
            max_depth_reached: s.max_depth_reached,
            execution_time_microseconds: s.execution_time_microseconds,
        }
    }
}

impl From<m::GraphPath> for pb::GraphPath {
    fn from(p: m::GraphPath) -> Self {
        pb::GraphPath {
            entities: p
                .node_ids
                .into_iter()
                .map(|id| pb::Entity {
                    id,
                    ..Default::default()
                })
                .collect(),
            relations: Vec::new(),
        }
    }
}
impl From<pb::GraphPath> for m::GraphPath {
    fn from(p: pb::GraphPath) -> Self {
        m::GraphPath {
            node_ids: p.entities.into_iter().map(|e| e.id).collect(),
            edge_ids: Vec::new(),
        }
    }
}

impl From<m::TraversalResponse> for pb::TraversalResponse {
    fn from(r: m::TraversalResponse) -> Self {
        pb::TraversalResponse {
            nodes: r.nodes.into_iter().map(Into::into).collect(),
            edges: r.edges.into_iter().map(Into::into).collect(),
            paths: r.paths.into_iter().map(Into::into).collect(),
            stats: r.stats.map(Into::into),
        }
    }
}
impl From<pb::TraversalResponse> for m::TraversalResponse {
    fn from(r: pb::TraversalResponse) -> Self {
        m::TraversalResponse {
            nodes: r.nodes.into_iter().map(Into::into).collect(),
            edges: r.edges.into_iter().map(Into::into).collect(),
            paths: r.paths.into_iter().map(Into::into).collect(),
            stats: r.stats.map(Into::into),
        }
    }
}

// ── Stats ───────────────────────────────────────────────────────────────────

impl From<m::LabelStats> for pb::LabelStats {
    fn from(s: m::LabelStats) -> Self {
        pb::LabelStats {
            label: s.label,
            count: s.count,
        }
    }
}
impl From<pb::LabelStats> for m::LabelStats {
    fn from(s: pb::LabelStats) -> Self {
        m::LabelStats {
            label: s.label,
            count: s.count,
        }
    }
}
impl From<m::EdgeTypeStats> for pb::EdgeTypeStats {
    fn from(s: m::EdgeTypeStats) -> Self {
        pb::EdgeTypeStats {
            edge_type: s.edge_type,
            count: s.count,
        }
    }
}
impl From<pb::EdgeTypeStats> for m::EdgeTypeStats {
    fn from(s: pb::EdgeTypeStats) -> Self {
        m::EdgeTypeStats {
            edge_type: s.edge_type,
            count: s.count,
        }
    }
}

impl From<m::GraphStats> for pb::GraphStats {
    fn from(s: m::GraphStats) -> Self {
        pb::GraphStats {
            total_nodes: s.total_nodes,
            total_edges: s.total_edges,
            label_stats: s.label_stats.into_iter().map(Into::into).collect(),
            edge_type_stats: s.edge_type_stats.into_iter().map(Into::into).collect(),
            total_properties: s.total_properties,
            memory_usage_bytes: s.memory_usage_bytes,
            average_degree: s.average_degree,
            max_degree: s.max_degree,
            connected_components: s.connected_components,
        }
    }
}
impl From<pb::GraphStats> for m::GraphStats {
    fn from(s: pb::GraphStats) -> Self {
        m::GraphStats {
            total_nodes: s.total_nodes,
            total_edges: s.total_edges,
            label_stats: s.label_stats.into_iter().map(Into::into).collect(),
            edge_type_stats: s.edge_type_stats.into_iter().map(Into::into).collect(),
            total_properties: s.total_properties,
            memory_usage_bytes: s.memory_usage_bytes,
            average_degree: s.average_degree,
            max_degree: s.max_degree,
            connected_components: s.connected_components,
        }
    }
}
