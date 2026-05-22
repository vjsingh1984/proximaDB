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

//! Bidirectional conversions between modality-specific proto records and
//! [`ProximaRecord`] (spec §3 — Phase B).
//!
//! These `From` impls are the seam between the legacy per-modality proto
//! types and the unified envelope. They live here (not in the root crate)
//! to keep the dependency direction clean: `proximadb-records` → `proximadb-proto`.

use std::collections::HashMap;

use proximadb_data_model::ProximaValue;
use proximadb_proto::proximadb_v1::{
    Edge, EmbeddingVersion, Node, PropertyValue, SqlArray, SqlObject, SqlValue, VectorRecord,
    property_value, sql_value,
};

use crate::{
    EdgeDirection, EdgeShape, EmbeddingCell, LabelSet, ProximaRecord, ProximaTree, ProximaTreeNode,
    TypedRef,
};

// ---------------------------------------------------------------------------
// Value conversion helpers
// ---------------------------------------------------------------------------

/// Convert a proto `SqlValue` to a `ProximaValue`.
///
/// Used for metadata fields in `VectorRecord`.
pub fn sql_value_to_proxima(sql: &SqlValue) -> ProximaValue {
    match &sql.value {
        Some(sql_value::Value::StringValue(s)) => ProximaValue::String(s.clone()),
        Some(sql_value::Value::NumberValue(f)) => ProximaValue::Float64(*f),
        Some(sql_value::Value::BoolValue(b)) => ProximaValue::Boolean(*b),
        Some(sql_value::Value::Int64Value(i)) => ProximaValue::Int64(*i),
        Some(sql_value::Value::BytesValue(b)) => match ProximaValue::from_jsonb_slice(b) {
            Ok(v) => ProximaValue::Jsonb(v),
            Err(_) => ProximaValue::Binary(b.clone()),
        },
        Some(sql_value::Value::NullValue(_)) => ProximaValue::Null,
        Some(sql_value::Value::ArrayValue(arr)) => {
            let items: Vec<ProximaValue> = arr.values.iter().map(sql_value_to_proxima).collect();
            ProximaValue::Array(items)
        }
        Some(sql_value::Value::ObjectValue(obj)) => {
            let map: HashMap<String, ProximaValue> = obj
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), sql_value_to_proxima(v)))
                .collect();
            ProximaValue::Map(map)
        }
        None => ProximaValue::Null,
    }
}

/// Convert a canonical `ProximaValue` into the legacy v1 `SqlValue`.
///
/// This exists only as a storage-service adapter while the vector operations
/// service still accepts `VectorRecord`. Public and internal v2 APIs should use
/// `ProximaValue`/`ProximaRecord` directly.
pub fn proxima_to_sql_value(value: &ProximaValue) -> SqlValue {
    let inner = match value {
        ProximaValue::Boolean(v) => sql_value::Value::BoolValue(*v),
        ProximaValue::Int8(v) => sql_value::Value::Int64Value(*v as i64),
        ProximaValue::Int16(v) => sql_value::Value::Int64Value(*v as i64),
        ProximaValue::Int32(v) => sql_value::Value::Int64Value(*v as i64),
        ProximaValue::Int64(v) => sql_value::Value::Int64Value(*v),
        ProximaValue::UInt8(v) => sql_value::Value::Int64Value(*v as i64),
        ProximaValue::UInt16(v) => sql_value::Value::Int64Value(*v as i64),
        ProximaValue::UInt32(v) => sql_value::Value::Int64Value(*v as i64),
        ProximaValue::UInt64(v) => sql_value::Value::StringValue(v.to_string()),
        ProximaValue::Float16(v) | ProximaValue::Float32(v) => {
            sql_value::Value::NumberValue(*v as f64)
        }
        ProximaValue::Float64(v) => sql_value::Value::NumberValue(*v),
        ProximaValue::Decimal(v) => sql_value::Value::StringValue(v.clone()),
        ProximaValue::String(v) | ProximaValue::Symbol(v) => {
            sql_value::Value::StringValue(v.clone())
        }
        ProximaValue::Binary(v) | ProximaValue::BinaryVector(v) => {
            sql_value::Value::BytesValue(v.clone())
        }
        ProximaValue::Date(v) => sql_value::Value::Int64Value(*v as i64),
        ProximaValue::Time(v, _) => sql_value::Value::Int64Value(*v),
        ProximaValue::Timestamp(v, _) | ProximaValue::TimestampTz(v, _) => {
            sql_value::Value::Int64Value(*v)
        }
        ProximaValue::Uuid(v) | ProximaValue::ULID(v) => sql_value::Value::BytesValue(v.to_vec()),
        ProximaValue::Json(v) => sql_value::Value::StringValue(v.to_string()),
        ProximaValue::Jsonb(v) => sql_value::Value::BytesValue(
            ProximaValue::to_jsonb_vec(v).unwrap_or_else(|_| v.to_string().into_bytes()),
        ),
        ProximaValue::Array(values) => sql_value::Value::ArrayValue(SqlArray {
            values: values.iter().map(proxima_to_sql_value).collect(),
        }),
        ProximaValue::Map(values) | ProximaValue::Struct(values) => {
            sql_value::Value::ObjectValue(SqlObject {
                fields: values
                    .iter()
                    .map(|(k, v)| (k.clone(), proxima_to_sql_value(v)))
                    .collect(),
            })
        }
        ProximaValue::DenseVector(values) => sql_value::Value::ArrayValue(SqlArray {
            values: values
                .iter()
                .map(|v| SqlValue {
                    value: Some(sql_value::Value::NumberValue(*v as f64)),
                })
                .collect(),
        }),
        ProximaValue::SparseVector { indices, values } => {
            sql_value::Value::ObjectValue(SqlObject {
                fields: HashMap::from([
                    (
                        "indices".to_string(),
                        proxima_to_sql_value(&ProximaValue::Array(
                            indices.iter().map(|v| ProximaValue::UInt32(*v)).collect(),
                        )),
                    ),
                    (
                        "values".to_string(),
                        proxima_to_sql_value(&ProximaValue::DenseVector(values.clone())),
                    ),
                ]),
            })
        }
        ProximaValue::Null => sql_value::Value::NullValue(0),
    };

    SqlValue { value: Some(inner) }
}

/// Flatten a `ProximaTree` into legacy metadata for the remaining v1 storage path.
pub fn proxima_tree_to_sql_metadata(tree: &ProximaTree) -> HashMap<String, SqlValue> {
    tree.iter()
        .map(|(key, node)| {
            let value = match node {
                ProximaTreeNode::Value(value) => proxima_to_sql_value(value),
                ProximaTreeNode::Object(subtree) => {
                    proxima_to_sql_value(&ProximaValue::Map(proxima_tree_to_value_map(subtree)))
                }
            };
            (key.clone(), value)
        })
        .collect()
}

/// Convert a canonical nested property tree to nested `ProximaValue` maps.
pub fn proxima_tree_to_value_map(tree: &ProximaTree) -> HashMap<String, ProximaValue> {
    tree.iter()
        .map(|(key, node)| {
            let value = match node {
                ProximaTreeNode::Value(value) => value.clone(),
                ProximaTreeNode::Object(subtree) => {
                    ProximaValue::Map(proxima_tree_to_value_map(subtree))
                }
            };
            (key.clone(), value)
        })
        .collect()
}

/// Convert the canonical envelope to `VectorRecord` for the current vector
/// operations service. This should be deleted once storage accepts envelopes.
pub fn proxima_record_to_vector(record: &ProximaRecord) -> VectorRecord {
    let vector = record
        .embeddings
        .first()
        .map(|embedding| embedding.values.clone())
        .unwrap_or_default();

    VectorRecord {
        id: record.oid.clone(),
        vector,
        metadata: proxima_tree_to_sql_metadata(&record.props),
        timestamp: Some(record.created_at_ns / 1_000_000),
        updated_at: Some(record.updated_at_ns / 1_000_000),
        expires_at: record.valid_to_ns.map(|ns| ns / 1_000_000),
        version: Some(record.record_version as u32),
        source: record.origin.clone(),
    }
}

// ---------------------------------------------------------------------------
// ProximaRecord → VectorRecord
// ---------------------------------------------------------------------------

impl From<ProximaRecord> for VectorRecord {
    fn from(r: ProximaRecord) -> Self {
        proxima_record_to_vector(&r)
    }
}

impl From<&ProximaRecord> for VectorRecord {
    fn from(r: &ProximaRecord) -> Self {
        proxima_record_to_vector(r)
    }
}

/// Convert a proto `PropertyValue` to a `ProximaValue`.
///
/// Used for property maps in graph `Node` and `Edge`.
pub fn property_value_to_proxima(pv: &PropertyValue) -> ProximaValue {
    match &pv.value {
        Some(property_value::Value::StringValue(s)) => ProximaValue::String(s.clone()),
        Some(property_value::Value::IntValue(i)) => ProximaValue::Int64(*i),
        Some(property_value::Value::DoubleValue(f)) => ProximaValue::Float64(*f),
        Some(property_value::Value::BoolValue(b)) => ProximaValue::Boolean(*b),
        Some(property_value::Value::BytesValue(b)) => ProximaValue::Binary(b.clone()),
        Some(property_value::Value::ArrayValue(arr)) => {
            let items: Vec<ProximaValue> =
                arr.values.iter().map(property_value_to_proxima).collect();
            ProximaValue::Array(items)
        }
        Some(property_value::Value::ObjectValue(obj)) => {
            let map: HashMap<String, ProximaValue> = obj
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), property_value_to_proxima(v)))
                .collect();
            ProximaValue::Map(map)
        }
        Some(property_value::Value::VectorValue(vd)) => {
            ProximaValue::DenseVector(vd.values.clone())
        }
        None => ProximaValue::Null,
    }
}

/// Convert an `EmbeddingVersion` proto to an `EmbeddingCell`.
pub fn embedding_version_to_cell(ev: &EmbeddingVersion) -> EmbeddingCell {
    EmbeddingCell {
        model_id: ev.model_id.clone(),
        modality: format!("{}", ev.modality), // stored as i32 in proto
        values: ev.vector.clone(),
        dim: ev.dimension,
    }
}

fn ms_to_ns(ms: i64) -> i64 {
    ms.saturating_mul(1_000_000)
}

// ---------------------------------------------------------------------------
// VectorRecord → ProximaRecord
// ---------------------------------------------------------------------------

impl From<VectorRecord> for ProximaRecord {
    fn from(v: VectorRecord) -> Self {
        ProximaRecord::from(&v)
    }
}

impl From<&VectorRecord> for ProximaRecord {
    fn from(v: &VectorRecord) -> Self {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i64;

        let created_at_ns = v.timestamp.map(ms_to_ns).unwrap_or(now_ns);
        let updated_at_ns = v.updated_at.map(ms_to_ns).unwrap_or(created_at_ns);

        // Convert metadata map to ProximaTree
        let props: ProximaTree = v
            .metadata
            .iter()
            .map(|(k, sv)| (k.clone(), ProximaTreeNode::Value(sql_value_to_proxima(sv))))
            .collect();

        // The vector becomes a default embedding cell
        let embeddings = if !v.vector.is_empty() {
            vec![EmbeddingCell {
                model_id: "default".to_string(),
                modality: "dense_vector".to_string(),
                dim: v.vector.len() as u32,
                values: v.vector.clone(),
            }]
        } else {
            vec![]
        };

        ProximaRecord {
            oid: v.id.clone(),
            local_id: None,
            tid: None,
            variation_id: None,
            record_version: v.version.unwrap_or(0) as u64,
            spec_version: 1,
            tenant_id: String::new(),
            permitted_principals: Vec::new(),
            rls_policy_id: None,
            created_at_ns,
            updated_at_ns,
            valid_from_ns: None,
            valid_to_ns: v.expires_at.map(ms_to_ns),
            origin: v.source.clone(),
            actor: None,
            method: Some("vector_insert".to_string()),
            memory_type: None,
            props,
            refs: Vec::new(),
            edge: None,
            embeddings,
            sequence: None,
            labels: LabelSet::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Node → ProximaRecord
// ---------------------------------------------------------------------------

impl From<&Node> for ProximaRecord {
    fn from(n: &Node) -> Self {
        let created_at_ns = ms_to_ns(n.created_at_ms);
        let updated_at_ns = ms_to_ns(n.updated_at_ms);

        let props: ProximaTree = n
            .properties
            .iter()
            .map(|(k, pv)| {
                (
                    k.clone(),
                    ProximaTreeNode::Value(property_value_to_proxima(pv)),
                )
            })
            .collect();

        let labels = LabelSet::from(n.labels.clone());

        let embeddings = if let Some(ev) = &n.embedding {
            vec![embedding_version_to_cell(ev)]
        } else {
            vec![]
        };

        // Add a ref back to the graph node origin so cross-model queries can find it
        let refs = vec![TypedRef::GraphEdge {
            edge_id: n.id.clone(),
            direction: EdgeDirection::Outgoing,
        }];

        ProximaRecord {
            oid: n.id.clone(),
            local_id: None,
            tid: None,
            variation_id: None,
            record_version: 0,
            spec_version: 1,
            tenant_id: String::new(),
            permitted_principals: Vec::new(),
            rls_policy_id: None,
            created_at_ns,
            updated_at_ns,
            valid_from_ns: None,
            valid_to_ns: None,
            origin: None,
            actor: None,
            method: Some("graph_node".to_string()),
            memory_type: None,
            props,
            refs,
            edge: None,
            embeddings,
            sequence: None,
            labels,
        }
    }
}

// ---------------------------------------------------------------------------
// Edge → ProximaRecord
// ---------------------------------------------------------------------------

impl From<&Edge> for ProximaRecord {
    fn from(e: &Edge) -> Self {
        let created_at_ns = ms_to_ns(e.created_at_ms);
        let updated_at_ns = ms_to_ns(e.updated_at_ms);

        let props: ProximaTree = e
            .properties
            .iter()
            .map(|(k, pv)| {
                (
                    k.clone(),
                    ProximaTreeNode::Value(property_value_to_proxima(pv)),
                )
            })
            .collect();

        let edge = Some(EdgeShape {
            source_id: e.from_node_id.clone(),
            target_id: e.to_node_id.clone(),
            edge_type: e.edge_type.clone(),
            weight: e.weight,
        });

        ProximaRecord {
            oid: e.id.clone(),
            local_id: None,
            tid: None,
            variation_id: None,
            record_version: 0,
            spec_version: 1,
            tenant_id: String::new(),
            permitted_principals: Vec::new(),
            rls_policy_id: None,
            created_at_ns,
            updated_at_ns,
            valid_from_ns: None,
            valid_to_ns: None,
            origin: None,
            actor: None,
            method: Some("graph_edge".to_string()),
            memory_type: None,
            props,
            refs: Vec::new(),
            edge,
            embeddings: Vec::new(),
            sequence: None,
            labels: LabelSet::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (TDD — written before impl)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_data_model::ProximaValue;
    use proximadb_proto::proximadb_v1::{
        PropertyValue, SqlValue, VectorRecord, property_value, sql_value,
    };
    use std::collections::HashMap;

    fn make_sql_string(s: &str) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::StringValue(s.to_string())),
        }
    }

    fn make_sql_int(i: i64) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::Int64Value(i)),
        }
    }

    fn make_sql_float(f: f64) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::NumberValue(f)),
        }
    }

    fn make_prop_string(s: &str) -> PropertyValue {
        PropertyValue {
            value: Some(property_value::Value::StringValue(s.to_string())),
        }
    }

    fn make_prop_int(i: i64) -> PropertyValue {
        PropertyValue {
            value: Some(property_value::Value::IntValue(i)),
        }
    }

    // --- SqlValue → ProximaValue ---

    #[test]
    fn test_sql_string_to_proxima() {
        let sv = make_sql_string("hello");
        assert!(matches!(sql_value_to_proxima(&sv), ProximaValue::String(s) if s == "hello"));
    }

    #[test]
    fn test_sql_int_to_proxima() {
        let sv = make_sql_int(42);
        assert!(matches!(sql_value_to_proxima(&sv), ProximaValue::Int64(42)));
    }

    #[test]
    fn test_sql_float_to_proxima() {
        let expected = 3.125;
        let sv = make_sql_float(expected);
        match sql_value_to_proxima(&sv) {
            ProximaValue::Float64(f) => assert!((f - expected).abs() < 1e-9),
            other => panic!("expected Float64, got {:?}", other),
        }
    }

    #[test]
    fn test_sql_null_to_proxima() {
        let sv = SqlValue {
            value: Some(sql_value::Value::NullValue(0)),
        };
        assert!(matches!(sql_value_to_proxima(&sv), ProximaValue::Null));
    }

    #[test]
    fn test_sql_none_to_proxima_null() {
        let sv = SqlValue { value: None };
        assert!(matches!(sql_value_to_proxima(&sv), ProximaValue::Null));
    }

    #[test]
    fn test_sql_jsonb_to_proxima() {
        let original = serde_json::json!({"foo": "bar"});
        let bytes = ProximaValue::to_jsonb_vec(&original).unwrap();
        let sv = SqlValue {
            value: Some(sql_value::Value::BytesValue(bytes)),
        };
        match sql_value_to_proxima(&sv) {
            ProximaValue::Jsonb(v) => assert_eq!(v, original),
            other => panic!("expected Jsonb, got {:?}", other),
        }
    }

    #[test]
    fn sql_value_conversion_covers_binary_bool_array_and_object_shapes() {
        let binary = SqlValue {
            value: Some(sql_value::Value::BytesValue(vec![0xc1])),
        };
        assert_eq!(
            sql_value_to_proxima(&binary),
            ProximaValue::Binary(vec![0xc1])
        );
        assert_eq!(
            sql_value_to_proxima(&SqlValue {
                value: Some(sql_value::Value::BoolValue(true)),
            }),
            ProximaValue::Boolean(true)
        );
        assert_eq!(
            sql_value_to_proxima(&SqlValue {
                value: Some(sql_value::Value::ArrayValue(SqlArray {
                    values: vec![make_sql_string("x"), make_sql_int(7)],
                })),
            }),
            ProximaValue::Array(vec![
                ProximaValue::String("x".to_string()),
                ProximaValue::Int64(7),
            ])
        );
        assert_eq!(
            sql_value_to_proxima(&SqlValue {
                value: Some(sql_value::Value::ObjectValue(SqlObject {
                    fields: HashMap::from([("k".to_string(), make_sql_float(1.5))]),
                })),
            }),
            ProximaValue::Map(HashMap::from([(
                "k".to_string(),
                ProximaValue::Float64(1.5),
            )]))
        );
    }

    #[test]
    fn proxima_to_sql_value_covers_lossy_legacy_adapter_shapes() {
        let cases = vec![
            (
                ProximaValue::Boolean(false),
                sql_value::Value::BoolValue(false),
            ),
            (ProximaValue::Int8(-8), sql_value::Value::Int64Value(-8)),
            (ProximaValue::Int16(-16), sql_value::Value::Int64Value(-16)),
            (ProximaValue::Int32(32), sql_value::Value::Int64Value(32)),
            (ProximaValue::Int64(64), sql_value::Value::Int64Value(64)),
            (ProximaValue::UInt8(8), sql_value::Value::Int64Value(8)),
            (ProximaValue::UInt16(16), sql_value::Value::Int64Value(16)),
            (ProximaValue::UInt32(32), sql_value::Value::Int64Value(32)),
            (
                ProximaValue::UInt64(u64::MAX),
                sql_value::Value::StringValue(u64::MAX.to_string()),
            ),
            (
                ProximaValue::Float32(1.25),
                sql_value::Value::NumberValue(1.25),
            ),
            (
                ProximaValue::Float64(2.5),
                sql_value::Value::NumberValue(2.5),
            ),
            (
                ProximaValue::Decimal("99.01".to_string()),
                sql_value::Value::StringValue("99.01".to_string()),
            ),
            (
                ProximaValue::Symbol("hot".to_string()),
                sql_value::Value::StringValue("hot".to_string()),
            ),
            (
                ProximaValue::Binary(vec![1, 2]),
                sql_value::Value::BytesValue(vec![1, 2]),
            ),
            (
                ProximaValue::BinaryVector(vec![0b1010]),
                sql_value::Value::BytesValue(vec![0b1010]),
            ),
            (
                ProximaValue::Date(20_000),
                sql_value::Value::Int64Value(20_000),
            ),
            (
                ProximaValue::Time(123, proximadb_data_model::TimeUnit::Microsecond),
                sql_value::Value::Int64Value(123),
            ),
            (
                ProximaValue::Timestamp(456, proximadb_data_model::TimeUnit::Microsecond),
                sql_value::Value::Int64Value(456),
            ),
            (
                ProximaValue::TimestampTz(789, proximadb_data_model::TimeUnit::Microsecond),
                sql_value::Value::Int64Value(789),
            ),
            (
                ProximaValue::Uuid([3; 16]),
                sql_value::Value::BytesValue(vec![3; 16]),
            ),
            (
                ProximaValue::ULID([4; 16]),
                sql_value::Value::BytesValue(vec![4; 16]),
            ),
            (
                ProximaValue::Json(serde_json::json!({"a": 1})),
                sql_value::Value::StringValue(r#"{"a":1}"#.to_string()),
            ),
            (ProximaValue::Null, sql_value::Value::NullValue(0)),
        ];

        for (value, expected) in cases {
            assert_eq!(proxima_to_sql_value(&value).value, Some(expected));
        }

        assert!(matches!(
            proxima_to_sql_value(&ProximaValue::Array(vec![
                ProximaValue::Int64(1),
                ProximaValue::String("x".to_string())
            ]))
            .value,
            Some(sql_value::Value::ArrayValue(SqlArray { values })) if values.len() == 2
        ));
        assert!(matches!(
            proxima_to_sql_value(&ProximaValue::Map(HashMap::from([(
                "k".to_string(),
                ProximaValue::Boolean(true)
            )])))
            .value,
            Some(sql_value::Value::ObjectValue(SqlObject { fields })) if fields.contains_key("k")
        ));
        assert!(matches!(
            proxima_to_sql_value(&ProximaValue::DenseVector(vec![0.1, 0.2])).value,
            Some(sql_value::Value::ArrayValue(SqlArray { values })) if values.len() == 2
        ));
        assert!(matches!(
            proxima_to_sql_value(&ProximaValue::SparseVector {
                indices: vec![1, 3],
                values: vec![0.5, 0.9],
            })
            .value,
            Some(sql_value::Value::ObjectValue(SqlObject { fields }))
                if fields.contains_key("indices") && fields.contains_key("values")
        ));
    }

    // --- PropertyValue → ProximaValue ---

    #[test]
    fn test_property_string_to_proxima() {
        let pv = make_prop_string("alice");
        assert!(matches!(property_value_to_proxima(&pv), ProximaValue::String(s) if s == "alice"));
    }

    #[test]
    fn test_property_int_to_proxima() {
        let pv = make_prop_int(99);
        assert!(matches!(
            property_value_to_proxima(&pv),
            ProximaValue::Int64(99)
        ));
    }

    #[test]
    fn property_value_conversion_covers_all_graph_property_shapes() {
        assert_eq!(
            property_value_to_proxima(&PropertyValue {
                value: Some(property_value::Value::DoubleValue(1.5)),
            }),
            ProximaValue::Float64(1.5)
        );
        assert_eq!(
            property_value_to_proxima(&PropertyValue {
                value: Some(property_value::Value::BoolValue(true)),
            }),
            ProximaValue::Boolean(true)
        );
        assert_eq!(
            property_value_to_proxima(&PropertyValue {
                value: Some(property_value::Value::BytesValue(vec![1, 2])),
            }),
            ProximaValue::Binary(vec![1, 2])
        );
        assert_eq!(
            property_value_to_proxima(&PropertyValue {
                value: Some(property_value::Value::ArrayValue(
                    proximadb_proto::proximadb_v1::PropertyArray {
                        values: vec![make_prop_string("x"), make_prop_int(3)],
                    },
                )),
            }),
            ProximaValue::Array(vec![
                ProximaValue::String("x".to_string()),
                ProximaValue::Int64(3),
            ])
        );
        assert_eq!(
            property_value_to_proxima(&PropertyValue {
                value: Some(property_value::Value::ObjectValue(
                    proximadb_proto::proximadb_v1::PropertyObject {
                        fields: HashMap::from([(
                            "flag".to_string(),
                            PropertyValue {
                                value: Some(property_value::Value::BoolValue(false)),
                            }
                        )]),
                    },
                )),
            }),
            ProximaValue::Map(HashMap::from([(
                "flag".to_string(),
                ProximaValue::Boolean(false),
            )]))
        );
        assert_eq!(
            property_value_to_proxima(&PropertyValue {
                value: Some(property_value::Value::VectorValue(
                    proximadb_proto::proximadb_v1::VectorData {
                        values: vec![0.1, 0.2],
                    },
                )),
            }),
            ProximaValue::DenseVector(vec![0.1, 0.2])
        );
        assert_eq!(
            property_value_to_proxima(&PropertyValue { value: None }),
            ProximaValue::Null
        );
    }

    #[test]
    fn proxima_tree_and_record_to_legacy_vector_preserve_nested_props_and_temporal_fields() {
        let nested = ProximaTree::from([(
            "city".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("Chicago".to_string())),
        )]);
        let tree = ProximaTree::from([
            (
                "score".to_string(),
                ProximaTreeNode::Value(ProximaValue::Float64(0.9)),
            ),
            ("address".to_string(), ProximaTreeNode::Object(nested)),
        ]);

        let values = proxima_tree_to_value_map(&tree);
        assert!(matches!(values.get("address"), Some(ProximaValue::Map(_))));

        let metadata = proxima_tree_to_sql_metadata(&tree);
        assert!(matches!(
            metadata.get("address").and_then(|v| v.value.as_ref()),
            Some(sql_value::Value::ObjectValue(_))
        ));

        let mut record = ProximaRecord {
            oid: "rec-1".to_string(),
            props: tree,
            created_at_ns: 10_000_000,
            updated_at_ns: 20_000_000,
            valid_to_ns: Some(30_000_000),
            record_version: 7,
            origin: Some("canonical".to_string()),
            ..ProximaRecord::default()
        };
        record.embeddings.push(EmbeddingCell {
            model_id: "model".to_string(),
            modality: "text".to_string(),
            dim: 2,
            values: vec![0.1, 0.2],
        });

        let vector = proxima_record_to_vector(&record);
        assert_eq!(vector.id, "rec-1");
        assert_eq!(vector.vector, vec![0.1, 0.2]);
        assert_eq!(vector.timestamp, Some(10));
        assert_eq!(vector.updated_at, Some(20));
        assert_eq!(vector.expires_at, Some(30));
        assert_eq!(vector.version, Some(7));
        assert_eq!(vector.source.as_deref(), Some("canonical"));

        let vector_from_ref = VectorRecord::from(&record);
        let vector_from_owned = VectorRecord::from(record);
        assert_eq!(vector_from_ref.id, vector_from_owned.id);
    }

    #[test]
    fn embedding_version_and_node_embedding_lower_into_canonical_envelope() {
        let embedding = EmbeddingVersion {
            model_id: "e5".to_string(),
            model_version: "1".to_string(),
            vector: vec![0.4, 0.5],
            dimension: 2,
            created_at_ms: 100,
            model_params: HashMap::new(),
            modality: proximadb_proto::proximadb_v1::Modality::Image as i32,
        };

        let cell = embedding_version_to_cell(&embedding);
        assert_eq!(cell.model_id, "e5");
        assert_eq!(cell.modality, "1");
        assert_eq!(cell.dim, 2);
        assert_eq!(cell.values, vec![0.4, 0.5]);

        let node = Node {
            id: "node-img".to_string(),
            labels: vec!["Asset".to_string()],
            properties: HashMap::from([("kind".to_string(), make_prop_string("image"))]),
            embedding: Some(embedding),
            created_at_ms: 5,
            updated_at_ms: 6,
        };
        let record = ProximaRecord::from(&node);
        assert_eq!(record.embeddings.len(), 1);
        assert_eq!(record.embeddings[0].model_id, "e5");
        assert!(matches!(
            record.refs.as_slice(),
            [TypedRef::GraphEdge {
                edge_id,
                direction: EdgeDirection::Outgoing,
            }] if edge_id == "node-img"
        ));
        assert_eq!(record.updated_at_ns, ms_to_ns(6));
    }

    #[test]
    fn vector_record_owned_conversion_and_expiry_fields_are_preserved() {
        let record = VectorRecord {
            id: "vec-expiring".to_string(),
            vector: vec![0.7],
            metadata: HashMap::new(),
            timestamp: Some(11),
            updated_at: Some(12),
            expires_at: Some(13),
            version: None,
            source: None,
        };

        let canonical = ProximaRecord::from(record);
        assert_eq!(canonical.oid, "vec-expiring");
        assert_eq!(canonical.created_at_ns, ms_to_ns(11));
        assert_eq!(canonical.updated_at_ns, ms_to_ns(12));
        assert_eq!(canonical.valid_to_ns, Some(ms_to_ns(13)));
        assert_eq!(canonical.record_version, 0);
        assert_eq!(canonical.method.as_deref(), Some("vector_insert"));
    }

    // --- VectorRecord → ProximaRecord ---

    #[test]
    fn test_vector_record_to_proxima() {
        let mut meta = HashMap::new();
        meta.insert("category".to_string(), make_sql_string("tech"));
        meta.insert("score".to_string(), make_sql_float(0.95));

        let vr = VectorRecord {
            id: "vec_001".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata: meta,
            timestamp: Some(1_704_067_200_000), // 2024-01-01 ms
            updated_at: None,
            expires_at: None,
            version: Some(2),
            source: Some("test_source".to_string()),
        };

        let pr = ProximaRecord::from(&vr);

        assert_eq!(pr.oid, "vec_001");
        assert_eq!(pr.record_version, 2);
        assert_eq!(pr.origin.as_deref(), Some("test_source"));

        // Embedding cell from vector
        assert_eq!(pr.embeddings.len(), 1);
        assert_eq!(pr.embeddings[0].dim, 3);
        assert_eq!(pr.embeddings[0].modality, "dense_vector");

        // Props from metadata
        assert!(pr.props.contains_key("category"));
        assert!(pr.props.contains_key("score"));

        // Temporal
        assert!(pr.created_at_ns > 0);
        assert!(pr.valid_to_ns.is_none());
    }

    #[test]
    fn test_vector_record_empty_vector() {
        let vr = VectorRecord {
            id: "empty_vec".to_string(),
            vector: vec![],
            metadata: HashMap::new(),
            timestamp: None,
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        };

        let pr = ProximaRecord::from(&vr);
        assert!(pr.embeddings.is_empty(), "empty vector → no embedding cell");
    }

    // --- Node → ProximaRecord ---

    #[test]
    fn test_node_to_proxima() {
        use proximadb_proto::proximadb_v1::Node;
        let mut props = HashMap::new();
        props.insert("name".to_string(), make_prop_string("Alice"));
        props.insert("age".to_string(), make_prop_int(30));

        let node = Node {
            id: "node_alice".to_string(),
            labels: vec!["Person".to_string(), "Employee".to_string()],
            properties: props,
            embedding: None,
            created_at_ms: 1_000_000,
            updated_at_ms: 2_000_000,
        };

        let pr = ProximaRecord::from(&node);

        assert_eq!(pr.oid, "node_alice");
        assert!(pr.labels.contains("Person"));
        assert!(pr.labels.contains("Employee"));
        assert!(pr.props.contains_key("name"));
        assert!(pr.props.contains_key("age"));
        assert!(pr.edge.is_none(), "nodes have no edge topology");
        assert_eq!(pr.created_at_ns, ms_to_ns(1_000_000));
    }

    // --- Edge → ProximaRecord ---

    #[test]
    fn test_edge_to_proxima() {
        use proximadb_proto::proximadb_v1::Edge;
        let edge = Edge {
            id: "edge_001".to_string(),
            from_node_id: "node_a".to_string(),
            to_node_id: "node_b".to_string(),
            edge_type: "KNOWS".to_string(),
            properties: HashMap::new(),
            weight: Some(0.75),
            created_at_ms: 1_000_000,
            updated_at_ms: 1_000_000,
        };

        let pr = ProximaRecord::from(&edge);

        assert_eq!(pr.oid, "edge_001");

        let es = pr.edge.as_ref().expect("edge record must have EdgeShape");
        assert_eq!(es.source_id, "node_a");
        assert_eq!(es.target_id, "node_b");
        assert_eq!(es.edge_type, "KNOWS");
        assert!((es.weight.unwrap() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn test_edge_without_weight() {
        use proximadb_proto::proximadb_v1::Edge;
        let edge = Edge {
            id: "edge_002".to_string(),
            from_node_id: "a".to_string(),
            to_node_id: "b".to_string(),
            edge_type: "LINKED".to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        let pr = ProximaRecord::from(&edge);
        assert!(pr.edge.as_ref().unwrap().weight.is_none());
    }
}
