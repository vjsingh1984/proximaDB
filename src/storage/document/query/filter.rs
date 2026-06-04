// Document filter evaluation
//
// Evaluates filter conditions against documents at query time.
// Used for conditions that cannot be accelerated by indexes.

use jsonpath_rust::JsonPathQuery;
use proximadb_data_model::ProximaValue;
use proximadb_records::ProximaTree;
use proximadb_records::conversions::{json_to_proxima, sql_value_to_proxima};
use regex::Regex;
use serde_json::Value as JsonValue;

use crate::core::search::sql_value_filter::proxima_tree_to_json_map;
use crate::proto::proximadb_v1::{DocFilterCondition, DocFilterOperator, DocumentFilter};

use super::super::DocumentRecord;

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
        // Evaluate all conditions (AND logic). TD-106 Slice 7a: read the canonical
        // `props` tree (the vestigial `document: SqlObject` field is removed in S7e).
        for condition in &filter.conditions {
            if !self.evaluate_condition(condition, &document.props) {
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
    fn evaluate_condition(&self, condition: &DocFilterCondition, document: &ProximaTree) -> bool {
        let path = &condition.path;
        let operator = DocFilterOperator::try_from(condition.operator)
            .unwrap_or(DocFilterOperator::Unspecified);

        // `extract_path_value` returns a canonical `ProximaValue` (Slice 2). The filter
        // operands `condition.value`/`condition.values` are proto wire operands off
        // `DocFilterCondition` — a legitimate edge; lift them once via `sql_value_to_proxima`
        // so the comparison kernel below operates entirely on canonical values
        // (TD-106 seam S3).
        let doc_value = self.extract_path_value(document, path);
        let cond_value = condition.value.as_ref().map(sql_value_to_proxima);
        let cond_values: Vec<ProximaValue> =
            condition.values.iter().map(sql_value_to_proxima).collect();

        match operator {
            DocFilterOperator::Eq => self.eval_eq(&doc_value, &cond_value),
            DocFilterOperator::Ne => !self.eval_eq(&doc_value, &cond_value),
            DocFilterOperator::Gt => self.eval_comparison(&doc_value, &cond_value, |a, b| a > b),
            DocFilterOperator::Gte => self.eval_comparison(&doc_value, &cond_value, |a, b| a >= b),
            DocFilterOperator::Lt => self.eval_comparison(&doc_value, &cond_value, |a, b| a < b),
            DocFilterOperator::Lte => self.eval_comparison(&doc_value, &cond_value, |a, b| a <= b),
            DocFilterOperator::In => self.eval_in(&doc_value, &cond_values),
            DocFilterOperator::NotIn => !self.eval_in(&doc_value, &cond_values),
            DocFilterOperator::Contains => self.eval_contains(&doc_value, &cond_value),
            DocFilterOperator::Regex => self.eval_regex(&doc_value, &cond_value),
            DocFilterOperator::Exists => self.eval_exists(&doc_value, &cond_value),
            DocFilterOperator::Type => self.eval_type(&doc_value, &cond_value),
            DocFilterOperator::Fulltext => {
                // Full-text search is handled by the index, not here
                true
            }
            DocFilterOperator::Unspecified => true,
        }
    }

    /// Extract value at a JSON path, returning a canonical `ProximaValue`.
    fn extract_path_value(&self, document: &ProximaTree, path: &str) -> Option<ProximaValue> {
        // The jsonpath engine operates on serde_json::Value, so the canonical `props`
        // tree is rendered to JSON via the shared `proxima_tree_to_json_map` bridge; the
        // result is lifted back to `ProximaValue` via `json_to_proxima` (TD-106 Slice 7a)
        // — no proto `SqlObject`/`SqlValue` is constructed here.
        let json_value =
            JsonValue::Object(proxima_tree_to_json_map(document).into_iter().collect());

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
                            Some(json_to_proxima(&arr[0]))
                        }
                    }
                    // Multiple element array = return as array (nulls filtered)
                    JsonValue::Array(arr) => {
                        let values: Vec<ProximaValue> = arr
                            .iter()
                            .filter(|v| !v.is_null())
                            .map(json_to_proxima)
                            .collect();
                        if values.is_empty() {
                            None
                        } else {
                            Some(ProximaValue::Array(values))
                        }
                    }
                    // Null result = not found
                    JsonValue::Null => None,
                    // Direct value result
                    _ => Some(json_to_proxima(&results)),
                }
            }
            Err(_) => None,
        }
    }

    /// Evaluate equality
    fn eval_eq(
        &self,
        doc_value: &Option<ProximaValue>,
        filter_value: &Option<ProximaValue>,
    ) -> bool {
        match (doc_value, filter_value) {
            (Some(a), Some(b)) => self.proxima_values_equal(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// Compare two `ProximaValue` instances for equality (document-filter semantics).
    ///
    /// Mirrors the prior proto-`SqlValue` comparison: scalar equality per type, f64
    /// epsilon for floats, and cross-type `Int64`/`Float64` numeric equality. Byte
    /// payloads land as `Binary` or `Jsonb` via `sql_value_to_proxima`, compared within
    /// their variant (identical bytes map to the same variant, so this preserves the old
    /// raw-bytes equality).
    fn proxima_values_equal(&self, a: &ProximaValue, b: &ProximaValue) -> bool {
        match (a, b) {
            (ProximaValue::Null, ProximaValue::Null) => true,
            (ProximaValue::Boolean(va), ProximaValue::Boolean(vb)) => va == vb,
            (ProximaValue::Int64(va), ProximaValue::Int64(vb)) => va == vb,
            (ProximaValue::Float64(va), ProximaValue::Float64(vb)) => {
                (va - vb).abs() < f64::EPSILON
            }
            (ProximaValue::String(va), ProximaValue::String(vb)) => va == vb,
            (ProximaValue::Binary(va), ProximaValue::Binary(vb)) => va == vb,
            (ProximaValue::Jsonb(va), ProximaValue::Jsonb(vb)) => va == vb,
            // Cross-type numeric comparison
            (ProximaValue::Int64(va), ProximaValue::Float64(vb)) => {
                (*va as f64 - vb).abs() < f64::EPSILON
            }
            (ProximaValue::Float64(va), ProximaValue::Int64(vb)) => {
                (va - *vb as f64).abs() < f64::EPSILON
            }
            _ => false,
        }
    }

    /// Evaluate numeric comparison
    fn eval_comparison<F>(
        &self,
        doc_value: &Option<ProximaValue>,
        filter_value: &Option<ProximaValue>,
        cmp: F,
    ) -> bool
    where
        F: Fn(f64, f64) -> bool,
    {
        match (doc_value, filter_value) {
            (Some(a), Some(b)) => {
                match (self.proxima_value_to_f64(a), self.proxima_value_to_f64(b)) {
                    (Some(a), Some(b)) => cmp(a, b),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Convert a `ProximaValue` to f64 for comparison
    fn proxima_value_to_f64(&self, value: &ProximaValue) -> Option<f64> {
        match value {
            ProximaValue::Int64(v) => Some(*v as f64),
            ProximaValue::Float64(v) => Some(*v),
            _ => None,
        }
    }

    /// Evaluate IN operator
    fn eval_in(&self, doc_value: &Option<ProximaValue>, values: &[ProximaValue]) -> bool {
        if let Some(doc) = doc_value {
            values.iter().any(|v| self.proxima_values_equal(doc, v))
        } else {
            false
        }
    }

    /// Evaluate CONTAINS operator (array containment)
    fn eval_contains(
        &self,
        doc_value: &Option<ProximaValue>,
        filter_value: &Option<ProximaValue>,
    ) -> bool {
        match (doc_value, filter_value) {
            (Some(ProximaValue::Array(arr)), Some(filter)) => {
                arr.iter().any(|v| self.proxima_values_equal(v, filter))
            }
            _ => false,
        }
    }

    /// Evaluate REGEX operator
    fn eval_regex(
        &self,
        doc_value: &Option<ProximaValue>,
        filter_value: &Option<ProximaValue>,
    ) -> bool {
        match (doc_value, filter_value) {
            (Some(ProximaValue::String(doc_str)), Some(ProximaValue::String(pattern))) => {
                match Regex::new(pattern) {
                    Ok(regex) => regex.is_match(doc_str),
                    Err(_) => false,
                }
            }
            _ => false,
        }
    }

    /// Evaluate EXISTS operator
    fn eval_exists(
        &self,
        doc_value: &Option<ProximaValue>,
        filter_value: &Option<ProximaValue>,
    ) -> bool {
        let should_exist = match filter_value {
            Some(ProximaValue::Boolean(b)) => *b,
            _ => true,
        };

        doc_value.is_some() == should_exist
    }

    /// Evaluate TYPE operator
    fn eval_type(
        &self,
        doc_value: &Option<ProximaValue>,
        filter_value: &Option<ProximaValue>,
    ) -> bool {
        let expected_type = match filter_value {
            Some(ProximaValue::String(s)) => Some(s.as_str()),
            _ => None,
        };

        match (doc_value, expected_type) {
            (Some(doc), Some(expected)) => {
                let actual_type = match doc {
                    ProximaValue::Null => "null",
                    ProximaValue::Boolean(_) => "boolean",
                    ProximaValue::Int64(_) => "integer",
                    ProximaValue::Float64(_) => "number",
                    ProximaValue::String(_) => "string",
                    ProximaValue::Jsonb(_) => "jsonb",
                    ProximaValue::Binary(_) => "bytes",
                    ProximaValue::Array(_) => "array",
                    ProximaValue::Map(_) => "object",
                    _ => "undefined",
                };
                actual_type == expected
            }
            _ => false,
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
    use proximadb_records::ProximaTreeNode;
    use std::collections::HashMap;

    /// Build a flat canonical `props` tree from leaf values.
    fn tree(fields: Vec<(&str, ProximaValue)>) -> ProximaTree {
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), ProximaTreeNode::Value(v)))
            .collect()
    }

    #[test]
    fn test_filter_evaluator_new() {
        let _evaluator = FilterEvaluator::new();
        // Basic instantiation test
        assert!(true);
    }

    #[test]
    fn test_proxima_values_equal() {
        let evaluator = FilterEvaluator::new();

        // Same-type equality
        assert!(evaluator.proxima_values_equal(&ProximaValue::Int64(42), &ProximaValue::Int64(42)));
        assert!(!evaluator.proxima_values_equal(&ProximaValue::Int64(42), &ProximaValue::Int64(7)));
        // Cross-type numeric equality (Int64 vs Float64), preserved from the proto kernel
        assert!(
            evaluator.proxima_values_equal(&ProximaValue::Int64(42), &ProximaValue::Float64(42.0))
        );
        // Null equality
        assert!(evaluator.proxima_values_equal(&ProximaValue::Null, &ProximaValue::Null));
        // Distinct variants are not equal
        assert!(
            !evaluator
                .proxima_values_equal(&ProximaValue::String("42".into()), &ProximaValue::Int64(42))
        );
    }

    #[test]
    fn test_extract_path_value_simple_field() {
        let evaluator = FilterEvaluator::new();

        // Create a simple document: {"name": "Alice", "age": 30}
        let doc = tree(vec![
            ("name", ProximaValue::String("Alice".to_string())),
            ("age", ProximaValue::Int64(30)),
        ]);

        // Test extraction with $.name
        let result = evaluator.extract_path_value(&doc, "$.name");
        assert!(result.is_some());
        if let Some(ProximaValue::String(s)) = result {
            assert_eq!(s, "Alice");
        } else {
            panic!("Expected string value");
        }

        // Test extraction without $ prefix (should normalize)
        let result = evaluator.extract_path_value(&doc, "age");
        assert!(result.is_some());
        if let Some(ProximaValue::Int64(i)) = result {
            assert_eq!(i, 30);
        } else {
            panic!("Expected int64 value");
        }
    }

    #[test]
    fn test_extract_path_value_nested_field() {
        let evaluator = FilterEvaluator::new();

        // Create a nested document: {"profile": {"name": "Bob", "scores": [90, 85, 92]}}
        let profile: ProximaTree = HashMap::from([
            (
                "name".to_string(),
                ProximaTreeNode::Value(ProximaValue::String("Bob".to_string())),
            ),
            (
                "scores".to_string(),
                ProximaTreeNode::Value(ProximaValue::Array(vec![
                    ProximaValue::Int64(90),
                    ProximaValue::Int64(85),
                    ProximaValue::Int64(92),
                ])),
            ),
        ]);
        let doc: ProximaTree =
            HashMap::from([("profile".to_string(), ProximaTreeNode::Object(profile))]);

        // Test nested path
        let result = evaluator.extract_path_value(&doc, "$.profile.name");
        assert!(result.is_some());
        if let Some(ProximaValue::String(s)) = result {
            assert_eq!(s, "Bob");
        } else {
            panic!("Expected string value for nested path");
        }
    }

    #[test]
    fn test_extract_path_value_missing_field() {
        let evaluator = FilterEvaluator::new();

        let doc = ProximaTree::new();

        let result = evaluator.extract_path_value(&doc, "$.nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_jsonb_filtering() {
        let evaluator = FilterEvaluator::new();
        let original = serde_json::json!({"priority": "high", "active": true});

        // Document carries the jsonb object as a canonical leaf; the
        // `proxima_tree_to_json_map` bridge renders it back to JSON for jsonpath.
        let doc: ProximaTree = HashMap::from([(
            "metadata".to_string(),
            ProximaTreeNode::Value(ProximaValue::Jsonb(original)),
        )]);

        let result = evaluator.extract_path_value(&doc, "$.metadata.priority");
        assert!(result.is_some());
        if let Some(ProximaValue::String(s)) = result {
            assert_eq!(s, "high");
        } else {
            panic!("Expected string 'high' from jsonb path");
        }
    }
}
