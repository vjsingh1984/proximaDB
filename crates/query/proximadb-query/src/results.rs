//! Shared cross-model query result contracts for the extracted query runtime.

use std::collections::HashMap;

use proximadb_data_model::DataModel;

/// Result of a unified query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Result records.
    pub records: Vec<UnifiedRecord>,
    /// Total count (if available).
    pub total_count: Option<u64>,
    /// Execution metrics.
    pub metrics: QueryMetrics,
}

/// A unified record from any data model.
#[derive(Debug, Clone)]
pub struct UnifiedRecord {
    /// Record ID.
    pub id: String,
    /// Source model.
    pub source_model: DataModel,
    /// Record data as JSON.
    pub data: serde_json::Value,
    /// Relevance score (if applicable).
    pub score: Option<f64>,
    /// Additional metadata.
    pub metadata: HashMap<String, String>,
}

impl UnifiedRecord {
    /// Returns the canonical envelope shape shared by all modalities and all
    /// transport surfaces (REST, gRPC, Arrow Flight, pgwire).
    ///
    /// Envelope fields (`_id`, `_model`, `_score`) are stable across all
    /// protocol surfaces. The modality-specific payload is placed under a
    /// deterministic key (`_data` for documents, `_node`/`_edge` for graph,
    /// `_vector` for vector, `_row` for relational, `_record` for
    /// everything else) so that cross-model join logic is never
    /// ambiguous about which field contains the authoritative payload.
    pub fn to_canonical_envelope(&self) -> serde_json::Value {
        let mut envelope = serde_json::Map::new();

        envelope.insert("_id".into(), serde_json::Value::String(self.id.clone()));
        envelope.insert(
            "_model".into(),
            serde_json::Value::String(format!("{:?}", self.source_model).to_lowercase()),
        );
        if let Some(s) = self.score {
            envelope.insert("_score".into(), serde_json::json!(s));
        } else {
            envelope.insert("_score".into(), serde_json::Value::Null);
        }

        let payload_key = payload_key_for(&self.source_model);
        envelope.insert(payload_key.into(), self.data.clone());

        if !self.metadata.is_empty() {
            let meta_obj: serde_json::Map<_, _> = self
                .metadata
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            envelope.insert("_meta".into(), serde_json::Value::Object(meta_obj));
        }

        serde_json::Value::Object(envelope)
    }
}

/// Returns the stable payload key for a given data model.
fn payload_key_for(model: &DataModel) -> &'static str {
    match model {
        DataModel::Document => "_data",
        DataModel::Graph => "_node",
        DataModel::Vector => "_vector",
        DataModel::Relational => "_row",
        _ => "_record",
    }
}

/// Normalise a document `UnifiedRecord` so that `_data` always contains the
/// full document payload and the standard envelope fields are present.  Any
/// existing `_id`/`_model`/`_score` keys inside `data` are hoisted into the
/// envelope and stripped from the payload to avoid duplication.
pub fn normalize_document_result_shape(record: &UnifiedRecord) -> serde_json::Value {
    let mut base = record.to_canonical_envelope();

    // Hoist legacy `_id`/`id` from the inner payload when present so that the
    // envelope `_id` is authoritative and the payload is clean.
    if let serde_json::Value::Object(ref mut env) = base {
        if let Some(serde_json::Value::Object(doc)) = env.get_mut("_data") {
            for key in &["_id", "id", "_model", "_score"] {
                doc.remove(*key);
            }
        }
    }

    base
}

/// Normalise a graph `UnifiedRecord`.  Graph results often carry a sparse
/// `{id, labels, properties, start_node}` blob.  This function ensures the
/// canonical `_node` key holds a well-typed object with `id`, `labels` (array),
/// and `properties` (object) sub-keys so that cross-model join logic can rely
/// on stable field paths.
pub fn normalize_graph_result_shape(record: &UnifiedRecord) -> serde_json::Value {
    let mut base = record.to_canonical_envelope();

    if let serde_json::Value::Object(ref mut env) = base {
        let normalised_node = match env.remove("_node") {
            Some(serde_json::Value::Object(mut node)) => {
                // Ensure `labels` is an array (graph adapter sometimes writes a string).
                let labels = match node.remove("labels") {
                    Some(serde_json::Value::Array(arr)) => serde_json::Value::Array(arr),
                    Some(serde_json::Value::String(s)) if !s.is_empty() => {
                        serde_json::Value::Array(vec![serde_json::Value::String(s)])
                    }
                    _ => serde_json::Value::Array(vec![]),
                };

                // `properties` may arrive as a debug string from the graph adapter;
                // promote it to a proper object when we can, or to an empty object
                // when we cannot (preserving the contract for join consumers).
                let properties = match node.remove("properties") {
                    Some(obj @ serde_json::Value::Object(_)) => obj,
                    Some(serde_json::Value::String(_)) | None => {
                        serde_json::Value::Object(serde_json::Map::new())
                    }
                    Some(other) => other,
                };

                // Re-use whatever `id` the inner node carries, falling back to
                // the envelope `_id` so consumers always have a stable field.
                let node_id = node
                    .remove("id")
                    .unwrap_or_else(|| serde_json::Value::String(record.id.clone()));

                let mut normalised = serde_json::Map::new();
                normalised.insert("id".into(), node_id);
                normalised.insert("labels".into(), labels);
                normalised.insert("properties".into(), properties);
                // Carry any remaining keys (e.g. `start_node`, `end_node`, hop counts).
                normalised.extend(node);
                serde_json::Value::Object(normalised)
            }
            Some(other) => other,
            None => serde_json::Value::Object(serde_json::Map::new()),
        };
        env.insert("_node".into(), normalised_node);
    }

    base
}

/// Query execution metrics.
#[derive(Debug, Clone, Default)]
pub struct QueryMetrics {
    /// Total execution time in microseconds.
    pub total_time_us: u64,
    /// Time per sub-query.
    pub sub_query_times: Vec<(DataModel, u64)>,
    /// Number of records scanned.
    pub records_scanned: u64,
    /// Number of records returned.
    pub records_returned: u64,
    /// Cache hit rate.
    pub cache_hit_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_record(id: &str, data: serde_json::Value) -> UnifiedRecord {
        UnifiedRecord {
            id: id.into(),
            source_model: DataModel::Document,
            data,
            score: None,
            metadata: HashMap::new(),
        }
    }

    fn graph_record(id: &str, data: serde_json::Value, score: Option<f64>) -> UnifiedRecord {
        UnifiedRecord {
            id: id.into(),
            source_model: DataModel::Graph,
            data,
            score,
            metadata: HashMap::new(),
        }
    }

    // ── canonical envelope ────────────────────────────────────────────────

    #[test]
    fn canonical_envelope_has_standard_fields() {
        let r = doc_record("doc-1", serde_json::json!({"title": "hello"}));
        let env = r.to_canonical_envelope();
        assert_eq!(env["_id"], "doc-1");
        assert_eq!(env["_model"], "document");
        assert_eq!(env["_score"], serde_json::Value::Null);
        assert!(env.get("_data").is_some(), "_data key missing");
    }

    #[test]
    fn canonical_envelope_score_present_when_set() {
        let mut r = doc_record("doc-2", serde_json::json!({}));
        r.score = Some(0.92);
        let env = r.to_canonical_envelope();
        let score = env["_score"].as_f64().expect("_score should be f64");
        assert!((score - 0.92).abs() < 1e-9);
    }

    #[test]
    fn canonical_envelope_metadata_included() {
        let mut r = doc_record("doc-3", serde_json::json!({}));
        r.metadata.insert("tenant".into(), "acme".into());
        let env = r.to_canonical_envelope();
        assert_eq!(env["_meta"]["tenant"], "acme");
    }

    #[test]
    fn graph_envelope_uses_node_key() {
        let r = graph_record(
            "node-1",
            serde_json::json!({"id": "node-1", "labels": ["Person"], "properties": {}}),
            Some(1.0),
        );
        let env = r.to_canonical_envelope();
        assert!(env.get("_node").is_some(), "_node key missing");
        assert!(
            env.get("_data").is_none(),
            "_data should not appear for graph"
        );
    }

    // ── document normalisation ────────────────────────────────────────────

    #[test]
    fn normalize_document_strips_duplicate_id_from_payload() {
        let r = doc_record(
            "doc-4",
            serde_json::json!({"_id": "doc-4", "id": "doc-4", "body": "text"}),
        );
        let norm = normalize_document_result_shape(&r);
        let payload = &norm["_data"];
        assert!(
            payload.get("_id").is_none(),
            "_id should be stripped from payload"
        );
        assert!(
            payload.get("id").is_none(),
            "id should be stripped from payload"
        );
        assert_eq!(payload["body"], "text");
        // Envelope _id still present.
        assert_eq!(norm["_id"], "doc-4");
    }

    #[test]
    fn normalize_document_preserves_non_id_fields() {
        let r = doc_record("doc-5", serde_json::json!({"title": "hello", "count": 42}));
        let norm = normalize_document_result_shape(&r);
        assert_eq!(norm["_data"]["title"], "hello");
        assert_eq!(norm["_data"]["count"], 42);
    }

    // ── graph normalisation ───────────────────────────────────────────────

    #[test]
    fn normalize_graph_labels_string_becomes_array() {
        let r = graph_record(
            "n1",
            serde_json::json!({"id": "n1", "labels": "Person", "properties": {}}),
            None,
        );
        let norm = normalize_graph_result_shape(&r);
        let labels = &norm["_node"]["labels"];
        assert!(labels.is_array(), "labels should be array");
        assert_eq!(labels[0], "Person");
    }

    #[test]
    fn normalize_graph_properties_debug_string_becomes_empty_object() {
        let r = graph_record(
            "n2",
            serde_json::json!({"id": "n2", "labels": ["User"], "properties": "Properties { name: \"Alice\" }"}),
            None,
        );
        let norm = normalize_graph_result_shape(&r);
        let props = &norm["_node"]["properties"];
        assert!(props.is_object(), "properties should be object");
        // Debug string cannot be parsed; we get an empty object rather than an error.
        assert_eq!(props.as_object().unwrap().len(), 0);
    }

    #[test]
    fn normalize_graph_structured_properties_preserved() {
        let r = graph_record(
            "n3",
            serde_json::json!({"id": "n3", "labels": ["Repo"], "properties": {"stars": 1200, "lang": "Rust"}}),
            Some(0.75),
        );
        let norm = normalize_graph_result_shape(&r);
        assert_eq!(norm["_node"]["properties"]["stars"], 1200);
        assert_eq!(norm["_node"]["properties"]["lang"], "Rust");
        assert_eq!(norm["_node"]["id"], "n3");
        assert!((norm["_score"].as_f64().unwrap() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn normalize_graph_missing_node_key_yields_empty_object() {
        // A record with no usable node data (e.g. a bare id-only result).
        let r = graph_record("n4", serde_json::json!(null), None);
        let norm = normalize_graph_result_shape(&r);
        // Should not panic and should have _node present.
        assert!(norm.get("_node").is_some());
    }

    // ── cross-model shape consistency ─────────────────────────────────────

    #[test]
    fn document_and_graph_envelopes_share_same_top_level_keys() {
        let doc = doc_record("d1", serde_json::json!({"x": 1}));
        let graph = graph_record(
            "g1",
            serde_json::json!({"id": "g1", "labels": [], "properties": {}}),
            Some(0.5),
        );

        let doc_env = normalize_document_result_shape(&doc);
        let graph_env = normalize_graph_result_shape(&graph);

        // Both must have the three stable envelope fields.
        for key in &["_id", "_model", "_score"] {
            assert!(doc_env.get(key).is_some(), "doc envelope missing {key}");
            assert!(graph_env.get(key).is_some(), "graph envelope missing {key}");
        }
    }
}
