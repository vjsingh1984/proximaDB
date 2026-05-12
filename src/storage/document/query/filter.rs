// Document filter evaluation
//
// Evaluates filter conditions against documents at query time.
// Used for conditions that cannot be accelerated by indexes.

use anyhow::{Result, anyhow};
use jsonpath_rust::JsonPathQuery;
use proximadb_data_model::ProximaValue;
use regex::Regex;
use serde_json::Value as JsonValue;

use crate::proto::proximadb_v1::{
    DocFilterCondition, DocFilterOperator, DocumentFilter, SqlArray, SqlObject, SqlValue,
    sql_value::Value as SqlValueVariant,
};

use super::super::DocumentRecord;
use super::super::indexes::IndexValue;

/// Filter evaluator for document queries
pub struct FilterEvaluator {
    // Future: cache compiled regex patterns
}

impl FilterEvaluator {
    /// Create a new filter evaluator
    pub fn new() -> Self {
        Self {}
    }

    /// Evaluate a filter against a document
    pub fn evaluate(&self, filter: &DocumentFilter, document: &DocumentRecord) -> bool {
        // Evaluate all conditions (AND logic)
        for condition in &filter.conditions {
            if !self.evaluate_condition(condition, &document.document) {
                return false;
            }
        }

        // Evaluate OR groups
        if !filter.or_filters.is_empty() {
            let any_or_match = filter
                .or_filters
                .iter()
                .any(|or_filter| self.evaluate(or_filter, document));
            if !any_or_match {
                return false;
            }
        }

        // Evaluate nested AND groups
        for and_filter in &filter.and_filters {
            if !self.evaluate(and_filter, document) {
                return false;
            }
        }

        true
    }

    /// Evaluate a single condition
    fn evaluate_condition(&self, condition: &DocFilterCondition, document: &SqlObject) -> bool {
        let path = &condition.path;
        let operator = DocFilterOperator::try_from(condition.operator)
            .unwrap_or(DocFilterOperator::Unspecified);

        // Extract value from document
        let doc_value = self.extract_path_value(document, path);

        match operator {
            DocFilterOperator::Eq => self.eval_eq(&doc_value, &condition.value),
            DocFilterOperator::Ne => !self.eval_eq(&doc_value, &condition.value),
            DocFilterOperator::Gt => {
                self.eval_comparison(&doc_value, &condition.value, |a, b| a > b)
            }
            DocFilterOperator::Gte => {
                self.eval_comparison(&doc_value, &condition.value, |a, b| a >= b)
            }
            DocFilterOperator::Lt => {
                self.eval_comparison(&doc_value, &condition.value, |a, b| a < b)
            }
            DocFilterOperator::Lte => {
                self.eval_comparison(&doc_value, &condition.value, |a, b| a <= b)
            }
            DocFilterOperator::In => self.eval_in(&doc_value, &condition.values),
            DocFilterOperator::NotIn => !self.eval_in(&doc_value, &condition.values),
            DocFilterOperator::Contains => self.eval_contains(&doc_value, &condition.value),
            DocFilterOperator::Regex => self.eval_regex(&doc_value, &condition.value),
            DocFilterOperator::Exists => self.eval_exists(&doc_value, &condition.value),
            DocFilterOperator::Type => self.eval_type(&doc_value, &condition.value),
            DocFilterOperator::Fulltext => {
                // Full-text search is handled by the index, not here
                true
            }
            DocFilterOperator::Unspecified => true,
        }
    }

    /// Extract value at a JSON path
    fn extract_path_value(&self, document: &SqlObject, path: &str) -> Option<SqlValue> {
        // Convert SqlObject to serde_json::Value for jsonpath processing
        let json_value = self.sql_object_to_json(document);

        // Normalize path: $.field or field -> $.field
        let normalized_path = if path.starts_with("$.") || path.starts_with('$') {
            path.to_string()
        } else {
            format!("$.{}", path)
        };

        // Execute JSON path query
        match json_value.path(&normalized_path) {
            Ok(results) => {
                // Handle various result types from jsonpath
                match &results {
                    // Empty array = no matches
                    JsonValue::Array(arr) if arr.is_empty() => None,
                    // Single element array = return the element directly
                    JsonValue::Array(arr) if arr.len() == 1 => {
                        // Skip null values as "not found"
                        if arr[0].is_null() {
                            None
                        } else {
                            self.json_to_sql_value(&arr[0])
                        }
                    }
                    // Multiple element array = return as array
                    JsonValue::Array(arr) => {
                        // Filter out nulls
                        let sql_values: Vec<SqlValue> = arr
                            .iter()
                            .filter(|v| !v.is_null())
                            .filter_map(|v| self.json_to_sql_value(v))
                            .collect();
                        if sql_values.is_empty() {
                            None
                        } else {
                            Some(SqlValue {
                                value: Some(SqlValueVariant::ArrayValue(SqlArray {
                                    values: sql_values,
                                })),
                            })
                        }
                    }
                    // Null result = not found
                    JsonValue::Null => None,
                    // Direct value result
                    _ => self.json_to_sql_value(&results),
                }
            }
            Err(_) => None,
        }
    }

    /// Convert SqlObject to serde_json::Value
    fn sql_object_to_json(&self, obj: &SqlObject) -> JsonValue {
        let mut map = serde_json::Map::new();
        for (key, value) in &obj.fields {
            if let Some(json_val) = self.sql_value_to_json(value) {
                map.insert(key.clone(), json_val);
            }
        }
        JsonValue::Object(map)
    }

    /// Convert SqlValue to serde_json::Value
    fn sql_value_to_json(&self, value: &SqlValue) -> Option<JsonValue> {
        match &value.value {
            Some(SqlValueVariant::NullValue(_)) => Some(JsonValue::Null),
            Some(SqlValueVariant::BoolValue(b)) => Some(JsonValue::Bool(*b)),
            Some(SqlValueVariant::Int64Value(i)) => Some(JsonValue::Number((*i).into())),
            Some(SqlValueVariant::NumberValue(f)) => {
                serde_json::Number::from_f64(*f).map(JsonValue::Number)
            }
            Some(SqlValueVariant::StringValue(s)) => Some(JsonValue::String(s.clone())),
            Some(SqlValueVariant::BytesValue(b)) => {
                // Encode bytes as hex string
                let hex_str: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                Some(JsonValue::String(format!("0x{}", hex_str)))
            }
            Some(SqlValueVariant::JsonbValue(b)) => {
                // Deserialize binary JSONB (MessagePack)
                ProximaValue::from_jsonb_slice(b).ok()
            }
            Some(SqlValueVariant::ArrayValue(arr)) => {
                let json_arr: Vec<JsonValue> = arr
                    .values
                    .iter()
                    .filter_map(|v| self.sql_value_to_json(v))
                    .collect();
                Some(JsonValue::Array(json_arr))
            }
            Some(SqlValueVariant::ObjectValue(obj)) => Some(self.sql_object_to_json(obj)),
            None => None,
        }
    }

    /// Convert serde_json::Value to SqlValue
    fn json_to_sql_value(&self, value: &JsonValue) -> Option<SqlValue> {
        match value {
            JsonValue::Null => Some(SqlValue {
                value: Some(SqlValueVariant::NullValue(0)),
            }),
            JsonValue::Bool(b) => Some(SqlValue {
                value: Some(SqlValueVariant::BoolValue(*b)),
            }),
            JsonValue::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(SqlValue {
                        value: Some(SqlValueVariant::Int64Value(i)),
                    })
                } else {
                    n.as_f64().map(|f| SqlValue {
                        value: Some(SqlValueVariant::NumberValue(f)),
                    })
                }
            }
            JsonValue::String(s) => Some(SqlValue {
                value: Some(SqlValueVariant::StringValue(s.clone())),
            }),
            JsonValue::Array(arr) => {
                let sql_values: Vec<SqlValue> = arr
                    .iter()
                    .filter_map(|v| self.json_to_sql_value(v))
                    .collect();
                Some(SqlValue {
                    value: Some(SqlValueVariant::ArrayValue(SqlArray { values: sql_values })),
                })
            }
            JsonValue::Object(obj) => {
                let mut fields = std::collections::HashMap::new();
                for (k, v) in obj {
                    if let Some(sql_val) = self.json_to_sql_value(v) {
                        fields.insert(k.clone(), sql_val);
                    }
                }
                Some(SqlValue {
                    value: Some(SqlValueVariant::ObjectValue(SqlObject { fields })),
                })
            }
        }
    }

    /// Evaluate equality
    fn eval_eq(&self, doc_value: &Option<SqlValue>, filter_value: &Option<SqlValue>) -> bool {
        match (doc_value, filter_value) {
            (Some(a), Some(b)) => self.sql_values_equal(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// Compare two SqlValue instances for equality
    fn sql_values_equal(&self, a: &SqlValue, b: &SqlValue) -> bool {
        match (&a.value, &b.value) {
            (Some(SqlValueVariant::NullValue(_)), Some(SqlValueVariant::NullValue(_))) => true,
            (Some(SqlValueVariant::BoolValue(va)), Some(SqlValueVariant::BoolValue(vb))) => {
                va == vb
            }
            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::Int64Value(vb))) => {
                va == vb
            }
            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::NumberValue(vb))) => {
                (va - vb).abs() < f64::EPSILON
            }
            (Some(SqlValueVariant::StringValue(va)), Some(SqlValueVariant::StringValue(vb))) => {
                va == vb
            }
            (Some(SqlValueVariant::BytesValue(va)), Some(SqlValueVariant::BytesValue(vb))) => {
                va == vb
            }
            (Some(SqlValueVariant::JsonbValue(va)), Some(SqlValueVariant::JsonbValue(vb))) => {
                va == vb
            }
            // Cross-type numeric comparison
            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::NumberValue(vb))) => {
                (*va as f64 - vb).abs() < f64::EPSILON
            }
            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::Int64Value(vb))) => {
                (va - *vb as f64).abs() < f64::EPSILON
            }
            _ => false,
        }
    }

    /// Evaluate numeric comparison
    fn eval_comparison<F>(
        &self,
        doc_value: &Option<SqlValue>,
        filter_value: &Option<SqlValue>,
        cmp: F,
    ) -> bool
    where
        F: Fn(f64, f64) -> bool,
    {
        match (doc_value, filter_value) {
            (Some(a), Some(b)) => {
                let a_num = self.sql_value_to_f64(a);
                let b_num = self.sql_value_to_f64(b);
                match (a_num, b_num) {
                    (Some(a), Some(b)) => cmp(a, b),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Convert SqlValue to f64 for comparison
    fn sql_value_to_f64(&self, value: &SqlValue) -> Option<f64> {
        match &value.value {
            Some(SqlValueVariant::Int64Value(v)) => Some(*v as f64),
            Some(SqlValueVariant::NumberValue(v)) => Some(*v),
            _ => None,
        }
    }

    /// Evaluate IN operator
    fn eval_in(&self, doc_value: &Option<SqlValue>, values: &[SqlValue]) -> bool {
        if let Some(doc) = doc_value {
            values.iter().any(|v| self.sql_values_equal(doc, v))
        } else {
            false
        }
    }

    /// Evaluate CONTAINS operator (array containment)
    fn eval_contains(&self, doc_value: &Option<SqlValue>, filter_value: &Option<SqlValue>) -> bool {
        match (doc_value, filter_value) {
            (Some(doc), Some(filter)) => {
                if let Some(SqlValueVariant::ArrayValue(arr)) = &doc.value {
                    arr.values.iter().any(|v| self.sql_values_equal(v, filter))
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Evaluate REGEX operator
    fn eval_regex(&self, doc_value: &Option<SqlValue>, filter_value: &Option<SqlValue>) -> bool {
        match (doc_value, filter_value) {
            (Some(doc), Some(filter)) => {
                if let (
                    Some(SqlValueVariant::StringValue(doc_str)),
                    Some(SqlValueVariant::StringValue(pattern)),
                ) = (&doc.value, &filter.value)
                {
                    if let Ok(regex) = Regex::new(pattern) {
                        regex.is_match(doc_str)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Evaluate EXISTS operator
    fn eval_exists(&self, doc_value: &Option<SqlValue>, filter_value: &Option<SqlValue>) -> bool {
        let should_exist = filter_value
            .as_ref()
            .and_then(|v| match &v.value {
                Some(SqlValueVariant::BoolValue(b)) => Some(*b),
                _ => None,
            })
            .unwrap_or(true);

        doc_value.is_some() == should_exist
    }

    /// Evaluate TYPE operator
    fn eval_type(&self, doc_value: &Option<SqlValue>, filter_value: &Option<SqlValue>) -> bool {
        let expected_type = filter_value.as_ref().and_then(|v| match &v.value {
            Some(SqlValueVariant::StringValue(s)) => Some(s.as_str()),
            _ => None,
        });

        match (doc_value, expected_type) {
            (Some(doc), Some(expected)) => {
                let actual_type = match &doc.value {
                    Some(SqlValueVariant::NullValue(_)) => "null",
                    Some(SqlValueVariant::BoolValue(_)) => "boolean",
                    Some(SqlValueVariant::Int64Value(_)) => "integer",
                    Some(SqlValueVariant::NumberValue(_)) => "number",
                    Some(SqlValueVariant::StringValue(_)) => "string",
                    Some(SqlValueVariant::BytesValue(_)) => "bytes",
                    Some(SqlValueVariant::JsonbValue(_)) => "jsonb",
                    Some(SqlValueVariant::ArrayValue(_)) => "array",
                    Some(SqlValueVariant::ObjectValue(_)) => "object",
                    None => "undefined",
                };
                actual_type == expected
            }
            _ => false,
        }
    }

    /// Convert SqlValue to IndexValue for index queries
    pub fn sql_value_to_index_value(&self, value: &SqlValue) -> Result<IndexValue> {
        match &value.value {
            Some(SqlValueVariant::NullValue(_)) => Ok(IndexValue::Null),
            Some(SqlValueVariant::BoolValue(b)) => Ok(IndexValue::Bool(*b)),
            Some(SqlValueVariant::Int64Value(i)) => Ok(IndexValue::Int(*i)),
            Some(SqlValueVariant::NumberValue(f)) => Ok(IndexValue::Float(*f)),
            Some(SqlValueVariant::StringValue(s)) => Ok(IndexValue::String(s.clone())),
            Some(SqlValueVariant::BytesValue(b)) => Ok(IndexValue::Bytes(b.clone())),
            _ => Err(anyhow!("Cannot convert complex value to index value")),
        }
    }
}

impl Default for FilterEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_filter_evaluator_new() {
        let _evaluator = FilterEvaluator::new();
        // Basic instantiation test
        assert!(true);
    }

    #[test]
    fn test_sql_values_equal() {
        let evaluator = FilterEvaluator::new();

        let a = SqlValue {
            value: Some(SqlValueVariant::Int64Value(42)),
        };
        let b = SqlValue {
            value: Some(SqlValueVariant::Int64Value(42)),
        };

        assert!(evaluator.sql_values_equal(&a, &b));
    }

    #[test]
    fn test_sql_value_to_index_value() {
        let evaluator = FilterEvaluator::new();

        let value = SqlValue {
            value: Some(SqlValueVariant::StringValue("test".to_string())),
        };

        let index_value = evaluator.sql_value_to_index_value(&value).unwrap();
        assert!(matches!(index_value, IndexValue::String(s) if s == "test"));
    }

    #[test]
    fn test_extract_path_value_simple_field() {
        let evaluator = FilterEvaluator::new();

        // Create a simple document: {"name": "Alice", "age": 30}
        let mut fields = HashMap::new();
        fields.insert(
            "name".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("Alice".to_string())),
            },
        );
        fields.insert(
            "age".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::Int64Value(30)),
            },
        );
        let doc = SqlObject { fields };

        // Test extraction with $.name
        let result = evaluator.extract_path_value(&doc, "$.name");
        assert!(result.is_some());
        if let Some(SqlValue {
            value: Some(SqlValueVariant::StringValue(s)),
        }) = result
        {
            assert_eq!(s, "Alice");
        } else {
            panic!("Expected string value");
        }

        // Test extraction without $ prefix (should normalize)
        let result = evaluator.extract_path_value(&doc, "age");
        assert!(result.is_some());
        if let Some(SqlValue {
            value: Some(SqlValueVariant::Int64Value(i)),
        }) = result
        {
            assert_eq!(i, 30);
        } else {
            panic!("Expected int64 value");
        }
    }

    #[test]
    fn test_extract_path_value_nested_field() {
        let evaluator = FilterEvaluator::new();

        // Create a nested document: {"profile": {"name": "Bob", "scores": [90, 85, 92]}}
        let mut profile_fields = HashMap::new();
        profile_fields.insert(
            "name".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("Bob".to_string())),
            },
        );
        profile_fields.insert(
            "scores".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::ArrayValue(SqlArray {
                    values: vec![
                        SqlValue {
                            value: Some(SqlValueVariant::Int64Value(90)),
                        },
                        SqlValue {
                            value: Some(SqlValueVariant::Int64Value(85)),
                        },
                        SqlValue {
                            value: Some(SqlValueVariant::Int64Value(92)),
                        },
                    ],
                })),
            },
        );

        let mut fields = HashMap::new();
        fields.insert(
            "profile".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::ObjectValue(SqlObject {
                    fields: profile_fields,
                })),
            },
        );
        let doc = SqlObject { fields };

        // Test nested path
        let result = evaluator.extract_path_value(&doc, "$.profile.name");
        assert!(result.is_some());
        if let Some(SqlValue {
            value: Some(SqlValueVariant::StringValue(s)),
        }) = result
        {
            assert_eq!(s, "Bob");
        } else {
            panic!("Expected string value for nested path");
        }
    }

    #[test]
    fn test_extract_path_value_missing_field() {
        let evaluator = FilterEvaluator::new();

        let fields = HashMap::new();
        let doc = SqlObject { fields };

        let result = evaluator.extract_path_value(&doc, "$.nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_json_round_trip() {
        let evaluator = FilterEvaluator::new();

        // Test that conversion is round-trip safe for various types
        let test_values = vec![
            SqlValue {
                value: Some(SqlValueVariant::StringValue("test".to_string())),
            },
            SqlValue {
                value: Some(SqlValueVariant::Int64Value(42)),
            },
            SqlValue {
                value: Some(SqlValueVariant::NumberValue(3.14)),
            },
            SqlValue {
                value: Some(SqlValueVariant::BoolValue(true)),
            },
            SqlValue {
                value: Some(SqlValueVariant::NullValue(0)),
            },
        ];

        for original in test_values {
            let json = evaluator.sql_value_to_json(&original);
            assert!(json.is_some(), "Should convert to JSON");
            let back = evaluator.json_to_sql_value(&json.unwrap());
            assert!(back.is_some(), "Should convert back to SqlValue");
            assert!(
                evaluator.sql_values_equal(&original, &back.unwrap()),
                "Round-trip should preserve value"
            );
        }
    }

    #[test]
    fn test_jsonb_filtering() {
        let evaluator = FilterEvaluator::new();
        let original = serde_json::json!({"priority": "high", "active": true});
        let bytes = ProximaValue::to_jsonb_vec(&original).unwrap();
        
        let val = SqlValue {
            value: Some(SqlValueVariant::JsonbValue(bytes)),
        };

        // Extract path value from a document containing this jsonb
        let mut fields = HashMap::new();
        fields.insert("metadata".to_string(), val);
        let doc = SqlObject { fields };

        let result = evaluator.extract_path_value(&doc, "$.metadata.priority");
        assert!(result.is_some());
        if let Some(SqlValue { value: Some(SqlValueVariant::StringValue(s)) }) = result {
            assert_eq!(s, "high");
        } else {
            panic!("Expected string 'high' from jsonb path");
        }
    }
}
