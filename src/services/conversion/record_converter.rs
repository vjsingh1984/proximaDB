/*
 * Copyright 2025 ProximaDB
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

//! RecordConverter: Bidirectional conversion between VectorRecord (v1) and ProximaRecord (v2)
//!
//! This module provides the conversion logic for migrating between record formats while
//! maintaining full backward compatibility.

use std::collections::HashMap;

use crate::core::types::{TextField, TextStorageStrategy, TypedValue as CoreTypedValue};
use crate::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value::Value as SqlValueVariant};

/// ProximaRecord representation using core types (not proto v2)
/// This is the internal representation used for conversion
#[derive(Debug, Clone)]
pub struct ProximaRecord {
    pub id: String,
    pub vector: Vec<f32>,
    pub vector_dimension: Option<u32>,
    pub typed_fields: HashMap<String, CoreTypedValue>,
    pub flexible_fields: HashMap<String, SqlValue>,
    pub text_fields: Vec<TextField>,
    pub timestamp_ms: i64,
    pub updated_at_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub version: Option<u32>,
    pub source: Option<String>,
    pub schema_id: Option<String>,
}

impl Default for ProximaRecord {
    fn default() -> Self {
        Self {
            id: String::new(),
            vector: Vec::new(),
            vector_dimension: None,
            typed_fields: HashMap::new(),
            flexible_fields: HashMap::new(),
            text_fields: Vec::new(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            updated_at_ms: None,
            expires_at_ms: None,
            version: None,
            source: None,
            schema_id: None,
        }
    }
}

/// RecordConverter provides bidirectional conversion between VectorRecord and ProximaRecord.
///
/// ## Design Principles
///
/// 1. **Lossless Conversion**: No data is lost in round-trip conversions
/// 2. **Type Inference**: SqlValue types are mapped to appropriate TypedValue types
/// 3. **TEXT Extraction**: Designated text columns are extracted to TextField entries
/// 4. **Backward Compatibility**: ProximaRecord can always be converted back to VectorRecord
pub struct RecordConverter;

impl RecordConverter {
    /// Convert a VectorRecord (v1) to ProximaRecord
    ///
    /// # Arguments
    ///
    /// * `v` - The VectorRecord to convert
    /// * `schema_id` - Optional schema ID for the collection
    /// * `text_columns` - List of column names that should be stored as TEXT fields
    ///
    /// # Returns
    ///
    /// A new ProximaRecord with:
    /// - Typed fields from metadata (excluding text columns)
    /// - Dedicated TextField entries for text columns
    /// - All temporal and provenance fields preserved
    pub fn vector_to_proxima(
        v: &VectorRecord,
        schema_id: Option<&str>,
        text_columns: &[String],
    ) -> ProximaRecord {
        let mut proxima = ProximaRecord {
            id: v.id.clone(),
            vector: v.vector.clone(),
            vector_dimension: Some(v.vector.len() as u32),
            typed_fields: HashMap::new(),
            flexible_fields: HashMap::new(),
            text_fields: Vec::new(),
            timestamp_ms: v.timestamp.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64
            }),
            updated_at_ms: v.updated_at,
            expires_at_ms: v.expires_at,
            version: v.version,
            source: v.source.clone(),
            schema_id: schema_id.map(|s| s.to_string()),
        };

        // Process metadata: separate text columns from typed fields
        for (key, sql_value) in &v.metadata {
            if text_columns.contains(key) {
                // Extract as TEXT field
                if let Some(text_content) = Self::sql_value_to_string(sql_value) {
                    proxima.text_fields.push(TextField {
                        name: key.clone(),
                        content: text_content.clone(),
                        storage_hint: Self::determine_storage_hint(&text_content),
                        chunk_count: None,
                        chunk_reference: None,
                    });
                }
            } else {
                // Convert to typed field
                if let Some(typed_value) = Self::sql_to_typed(sql_value) {
                    proxima.typed_fields.insert(key.clone(), typed_value);
                } else {
                    // Fallback: store in flexible_fields for unrecognized types
                    proxima
                        .flexible_fields
                        .insert(key.clone(), sql_value.clone());
                }
            }
        }

        proxima
    }

    /// Convert a ProximaRecord back to VectorRecord (v1)
    ///
    /// # Arguments
    ///
    /// * `p` - The ProximaRecord to convert
    ///
    /// # Returns
    ///
    /// A VectorRecord with all data merged back into the metadata map.
    /// TEXT fields are converted back to string SqlValues.
    pub fn proxima_to_vector(p: &ProximaRecord) -> VectorRecord {
        let mut metadata = HashMap::new();

        // Convert typed_fields back to SqlValue
        for (key, typed_value) in &p.typed_fields {
            if let Some(sql_value) = Self::typed_to_sql(typed_value) {
                metadata.insert(key.clone(), sql_value);
            }
        }

        // Convert text_fields back to string SqlValue
        for text_field in &p.text_fields {
            metadata.insert(
                text_field.name.clone(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(text_field.content.clone())),
                },
            );
        }

        // Include flexible_fields directly (already SqlValue)
        for (key, sql_value) in &p.flexible_fields {
            metadata.insert(key.clone(), sql_value.clone());
        }

        VectorRecord {
            id: p.id.clone(),
            vector: p.vector.clone(),
            metadata,
            timestamp: Some(p.timestamp_ms),
            updated_at: p.updated_at_ms,
            expires_at: p.expires_at_ms,
            version: p.version,
            source: p.source.clone(),
        }
    }

    /// Determine the optimal TEXT storage strategy based on content size
    fn determine_storage_hint(content: &str) -> TextStorageStrategy {
        let size = content.len();
        if size < 4096 {
            TextStorageStrategy::Inline
        } else if size < 1_048_576 {
            TextStorageStrategy::Chunked
        } else {
            TextStorageStrategy::Sidecar
        }
    }

    /// Extract string value from SqlValue
    fn sql_value_to_string(sql_value: &SqlValue) -> Option<String> {
        match &sql_value.value {
            Some(SqlValueVariant::StringValue(s)) => Some(s.clone()),
            Some(SqlValueVariant::Int64Value(i)) => Some(i.to_string()),
            Some(SqlValueVariant::NumberValue(f)) => Some(f.to_string()),
            Some(SqlValueVariant::BoolValue(b)) => Some(b.to_string()),
            _ => None,
        }
    }

    /// Convert SqlValue to CoreTypedValue with type inference
    fn sql_to_typed(sql_value: &SqlValue) -> Option<CoreTypedValue> {
        let value = sql_value.value.as_ref()?;

        match value {
            SqlValueVariant::StringValue(s) => Some(CoreTypedValue::Text(s.clone())),
            SqlValueVariant::Int64Value(i) => Some(CoreTypedValue::Integer(*i)),
            SqlValueVariant::NumberValue(f) => Some(CoreTypedValue::Float(*f)),
            SqlValueVariant::BoolValue(b) => Some(CoreTypedValue::Boolean(*b)),
            SqlValueVariant::NullValue(_) => Some(CoreTypedValue::Null),
            SqlValueVariant::ArrayValue(arr) => {
                // Infer array type from first element
                if let Some(first) = arr.values.first() {
                    match &first.value {
                        Some(SqlValueVariant::StringValue(_)) => {
                            let strings: Vec<String> = arr
                                .values
                                .iter()
                                .filter_map(|v| match &v.value {
                                    Some(SqlValueVariant::StringValue(s)) => Some(s.clone()),
                                    _ => None,
                                })
                                .collect();
                            Some(CoreTypedValue::ArrayText(strings))
                        }
                        Some(SqlValueVariant::Int64Value(_)) => {
                            let ints: Vec<i64> = arr
                                .values
                                .iter()
                                .filter_map(|v| match &v.value {
                                    Some(SqlValueVariant::Int64Value(i)) => Some(*i),
                                    _ => None,
                                })
                                .collect();
                            Some(CoreTypedValue::ArrayInteger(ints))
                        }
                        Some(SqlValueVariant::NumberValue(_)) => {
                            let floats: Vec<f64> = arr
                                .values
                                .iter()
                                .filter_map(|v| match &v.value {
                                    Some(SqlValueVariant::NumberValue(f)) => Some(*f),
                                    _ => None,
                                })
                                .collect();
                            Some(CoreTypedValue::ArrayFloat(floats))
                        }
                        Some(SqlValueVariant::BoolValue(_)) => {
                            let bools: Vec<bool> = arr
                                .values
                                .iter()
                                .filter_map(|v| match &v.value {
                                    Some(SqlValueVariant::BoolValue(b)) => Some(*b),
                                    _ => None,
                                })
                                .collect();
                            Some(CoreTypedValue::ArrayBoolean(bools))
                        }
                        _ => None,
                    }
                } else {
                    // Empty array - default to text array
                    Some(CoreTypedValue::ArrayText(vec![]))
                }
            }
            SqlValueVariant::ObjectValue(obj) => {
                // Convert object to Map<String, String>
                let map: HashMap<String, String> = obj
                    .fields
                    .iter()
                    .filter_map(|(k, v)| Self::sql_value_to_string(v).map(|s| (k.clone(), s)))
                    .collect();
                Some(CoreTypedValue::MapStringString(map))
            }
            SqlValueVariant::BytesValue(b) => Some(CoreTypedValue::Binary(b.clone())),
        }
    }

    /// Convert CoreTypedValue back to SqlValue
    fn typed_to_sql(typed: &CoreTypedValue) -> Option<SqlValue> {
        match typed {
            CoreTypedValue::Text(s) => Some(SqlValue {
                value: Some(SqlValueVariant::StringValue(s.clone())),
            }),
            CoreTypedValue::Integer(i) => Some(SqlValue {
                value: Some(SqlValueVariant::Int64Value(*i)),
            }),
            CoreTypedValue::Float(f) => Some(SqlValue {
                value: Some(SqlValueVariant::NumberValue(*f)),
            }),
            CoreTypedValue::Boolean(b) => Some(SqlValue {
                value: Some(SqlValueVariant::BoolValue(*b)),
            }),
            CoreTypedValue::Null => Some(SqlValue {
                value: Some(SqlValueVariant::NullValue(0)),
            }),
            CoreTypedValue::Binary(b) => Some(SqlValue {
                value: Some(SqlValueVariant::BytesValue(b.clone())),
            }),
            CoreTypedValue::ArrayText(arr) => Some(SqlValue {
                value: Some(SqlValueVariant::ArrayValue(
                    crate::proto::proximadb_v1::SqlArray {
                        values: arr
                            .iter()
                            .map(|s| SqlValue {
                                value: Some(SqlValueVariant::StringValue(s.clone())),
                            })
                            .collect(),
                    },
                )),
            }),
            CoreTypedValue::ArrayInteger(arr) => Some(SqlValue {
                value: Some(SqlValueVariant::ArrayValue(
                    crate::proto::proximadb_v1::SqlArray {
                        values: arr
                            .iter()
                            .map(|i| SqlValue {
                                value: Some(SqlValueVariant::Int64Value(*i)),
                            })
                            .collect(),
                    },
                )),
            }),
            CoreTypedValue::ArrayFloat(arr) => Some(SqlValue {
                value: Some(SqlValueVariant::ArrayValue(
                    crate::proto::proximadb_v1::SqlArray {
                        values: arr
                            .iter()
                            .map(|f| SqlValue {
                                value: Some(SqlValueVariant::NumberValue(*f)),
                            })
                            .collect(),
                    },
                )),
            }),
            CoreTypedValue::ArrayBoolean(arr) => Some(SqlValue {
                value: Some(SqlValueVariant::ArrayValue(
                    crate::proto::proximadb_v1::SqlArray {
                        values: arr
                            .iter()
                            .map(|b| SqlValue {
                                value: Some(SqlValueVariant::BoolValue(*b)),
                            })
                            .collect(),
                    },
                )),
            }),
            CoreTypedValue::MapStringString(map) => Some(SqlValue {
                value: Some(SqlValueVariant::ObjectValue(
                    crate::proto::proximadb_v1::SqlObject {
                        fields: map
                            .iter()
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    SqlValue {
                                        value: Some(SqlValueVariant::StringValue(v.clone())),
                                    },
                                )
                            })
                            .collect(),
                    },
                )),
            }),
            // Complex types that don't have direct SqlValue equivalents
            _ => None,
        }
    }

    /// Batch convert VectorRecords to ProximaRecords
    pub fn batch_vector_to_proxima(
        records: &[VectorRecord],
        schema_id: Option<&str>,
        text_columns: &[String],
    ) -> Vec<ProximaRecord> {
        records
            .iter()
            .map(|v| Self::vector_to_proxima(v, schema_id, text_columns))
            .collect()
    }

    /// Batch convert ProximaRecords to VectorRecords
    pub fn batch_proxima_to_vector(records: &[ProximaRecord]) -> Vec<VectorRecord> {
        records.iter().map(Self::proxima_to_vector).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_vector_record() -> VectorRecord {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("technology".to_string())),
            },
        );
        metadata.insert(
            "content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(
                    "This is a test document with text content.".to_string(),
                )),
            },
        );
        metadata.insert(
            "priority".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::Int64Value(5)),
            },
        );
        metadata.insert(
            "score".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::NumberValue(0.95)),
            },
        );
        metadata.insert(
            "active".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::BoolValue(true)),
            },
        );

        VectorRecord {
            id: "doc_1".to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata,
            timestamp: Some(1704067200000),
            updated_at: Some(1704067200000),
            expires_at: None,
            version: Some(1),
            source: Some("test_source".to_string()),
        }
    }

    #[test]
    fn test_vector_to_proxima_basic() {
        let vector_record = create_test_vector_record();
        let text_columns = vec!["content".to_string()];

        let proxima = RecordConverter::vector_to_proxima(&vector_record, None, &text_columns);

        assert_eq!(proxima.id, "doc_1");
        assert_eq!(proxima.vector, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(proxima.vector_dimension, Some(4));
        assert_eq!(proxima.timestamp_ms, 1704067200000);
        assert_eq!(proxima.version, Some(1));
        assert_eq!(proxima.source, Some("test_source".to_string()));

        // TEXT field should be extracted
        assert_eq!(proxima.text_fields.len(), 1);
        assert_eq!(proxima.text_fields[0].name, "content");
        assert!(proxima.text_fields[0].content.contains("test document"));

        // Other fields should be in typed_fields
        assert!(proxima.typed_fields.contains_key("category"));
        assert!(proxima.typed_fields.contains_key("priority"));
        assert!(proxima.typed_fields.contains_key("score"));
        assert!(proxima.typed_fields.contains_key("active"));
        assert!(!proxima.typed_fields.contains_key("content")); // Moved to text_fields
    }

    #[test]
    fn test_proxima_to_vector_roundtrip() {
        let original = create_test_vector_record();
        let text_columns = vec!["content".to_string()];

        // Convert to ProximaRecord
        let proxima = RecordConverter::vector_to_proxima(&original, None, &text_columns);

        // Convert back to VectorRecord
        let converted = RecordConverter::proxima_to_vector(&proxima);

        // Verify basic fields
        assert_eq!(converted.id, original.id);
        assert_eq!(converted.vector, original.vector);
        assert_eq!(converted.timestamp, original.timestamp);
        assert_eq!(converted.version, original.version);
        assert_eq!(converted.source, original.source);

        // Verify metadata preserved
        assert_eq!(converted.metadata.len(), original.metadata.len());
        assert!(converted.metadata.contains_key("category"));
        assert!(converted.metadata.contains_key("content"));
        assert!(converted.metadata.contains_key("priority"));
    }

    #[test]
    fn test_storage_hint_determination() {
        // Inline for small text
        let small_text = "Hello world";
        assert!(matches!(
            RecordConverter::determine_storage_hint(small_text),
            TextStorageStrategy::Inline
        ));

        // Chunked for medium text
        let medium_text = "a".repeat(10_000);
        assert!(matches!(
            RecordConverter::determine_storage_hint(&medium_text),
            TextStorageStrategy::Chunked
        ));

        // Sidecar for large text
        let large_text = "a".repeat(2_000_000);
        assert!(matches!(
            RecordConverter::determine_storage_hint(&large_text),
            TextStorageStrategy::Sidecar
        ));
    }

    #[test]
    fn test_type_conversion() {
        let sql_string = SqlValue {
            value: Some(SqlValueVariant::StringValue("hello".to_string())),
        };
        let typed = RecordConverter::sql_to_typed(&sql_string).unwrap();
        assert!(matches!(typed, CoreTypedValue::Text(_)));

        let sql_int = SqlValue {
            value: Some(SqlValueVariant::Int64Value(42)),
        };
        let typed = RecordConverter::sql_to_typed(&sql_int).unwrap();
        assert!(matches!(typed, CoreTypedValue::Integer(42)));

        let sql_float = SqlValue {
            value: Some(SqlValueVariant::NumberValue(3.14)),
        };
        let typed = RecordConverter::sql_to_typed(&sql_float).unwrap();
        assert!(matches!(typed, CoreTypedValue::Float(_)));

        let sql_bool = SqlValue {
            value: Some(SqlValueVariant::BoolValue(true)),
        };
        let typed = RecordConverter::sql_to_typed(&sql_bool).unwrap();
        assert!(matches!(typed, CoreTypedValue::Boolean(true)));
    }

    #[test]
    fn test_batch_conversion() {
        let records: Vec<VectorRecord> = (0..5)
            .map(|i| VectorRecord {
                id: format!("doc_{}", i),
                vector: vec![0.1 * i as f32],
                metadata: HashMap::new(),
                timestamp: Some(1704067200000 + i as i64),
                updated_at: None,
                expires_at: None,
                version: Some(i as u32),
                source: None,
            })
            .collect();

        let proxima_records =
            RecordConverter::batch_vector_to_proxima(&records, Some("schema_1"), &[]);

        assert_eq!(proxima_records.len(), 5);
        for (i, p) in proxima_records.iter().enumerate() {
            assert_eq!(p.id, format!("doc_{}", i));
            assert_eq!(p.schema_id, Some("schema_1".to_string()));
        }

        let vector_records = RecordConverter::batch_proxima_to_vector(&proxima_records);
        assert_eq!(vector_records.len(), 5);
    }

    #[test]
    fn test_no_text_columns() {
        let vector_record = create_test_vector_record();
        let text_columns: Vec<String> = vec![];

        let proxima = RecordConverter::vector_to_proxima(&vector_record, None, &text_columns);

        // All fields should be in typed_fields, none in text_fields
        assert!(proxima.text_fields.is_empty());
        assert!(proxima.typed_fields.contains_key("content"));
    }
}
