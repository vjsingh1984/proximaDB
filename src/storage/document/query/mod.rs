// Query execution for document storage
//
// Provides:
// - Filter evaluation
// - Sort operations
// - Pagination
// - Query planning and optimization

pub mod filter;
pub mod path_parser;

use anyhow::Result;
use jsonpath_rust::JsonPathQuery;
use serde_json::Value as JsonValue;
use tracing::debug;

use crate::proto::proximadb_v1::{
    DocFilterCondition, DocFilterOperator, SortField, SortOrder, SqlObject, SqlValue,
    sql_value::Value as SqlValueVariant,
};

use self::filter::FilterEvaluator;
use super::indexes::IndexManager;
use super::{DocumentQueryParams, DocumentRecord};

/// Query executor for document queries
pub struct QueryExecutor {
    /// Filter evaluator
    filter_evaluator: FilterEvaluator,
}

impl QueryExecutor {
    /// Create a new query executor
    pub fn new() -> Self {
        Self {
            filter_evaluator: FilterEvaluator::new(),
        }
    }

    /// Execute a document query
    pub async fn execute(
        &self,
        collection: &str,
        documents: &[DocumentRecord],
        params: &DocumentQueryParams,
        index_manager: &IndexManager,
    ) -> Result<(Vec<DocumentRecord>, u64)> {
        debug!("Executing query on collection: {}", collection);

        // Step 1: Determine candidate documents using indexes
        let candidates = self
            .get_candidates(collection, params, index_manager)
            .await?;

        // Step 2: Load and filter documents
        let mut documents = self.load_and_filter(documents, &candidates, params).await?;

        // Step 3: Sort results
        if !params.sort.is_empty() {
            self.sort_documents(&mut documents, &params.sort)?;
        }

        // Step 4: Get total count before pagination (if requested)
        let total_count = documents.len() as u64;

        // Step 5: Apply pagination
        let offset = params.offset as usize;
        let limit = if params.limit == 0 {
            documents.len()
        } else {
            params.limit as usize
        };

        let paginated: Vec<DocumentRecord> =
            documents.into_iter().skip(offset).take(limit).collect();

        Ok((paginated, total_count))
    }

    /// Get candidate document IDs using indexes
    async fn get_candidates(
        &self,
        collection: &str,
        params: &DocumentQueryParams,
        index_manager: &IndexManager,
    ) -> Result<Vec<String>> {
        // If no filter, return all documents (full scan)
        let filter = match &params.filter {
            Some(f) => f,
            None => return Ok(vec![]), // Empty means full scan
        };

        // Try to use indexes for each condition
        let mut candidate_sets: Vec<Vec<String>> = Vec::new();

        for condition in &filter.conditions {
            if let Some(candidates) = self
                .get_candidates_for_condition(collection, condition, index_manager)
                .await?
            {
                candidate_sets.push(candidates);
            }
        }

        // Intersect all candidate sets (AND logic)
        if candidate_sets.is_empty() {
            return Ok(vec![]); // Full scan
        }

        let mut result = candidate_sets.remove(0);
        for set in candidate_sets {
            let set: std::collections::HashSet<_> = set.into_iter().collect();
            result.retain(|id| set.contains(id));
        }

        Ok(result)
    }

    /// Get candidates for a single filter condition
    async fn get_candidates_for_condition(
        &self,
        collection: &str,
        condition: &DocFilterCondition,
        index_manager: &IndexManager,
    ) -> Result<Option<Vec<String>>> {
        let path = &condition.path;
        let operator = DocFilterOperator::try_from(condition.operator)
            .unwrap_or(DocFilterOperator::Unspecified);

        // Convert to index query
        let query_condition = match operator {
            DocFilterOperator::Eq => {
                if let Some(ref value) = condition.value {
                    Some(super::indexes::PathQueryCondition::Eq(
                        self.filter_evaluator.sql_value_to_index_value(value)?,
                    ))
                } else {
                    None
                }
            }
            DocFilterOperator::Gt => {
                if let Some(ref value) = condition.value {
                    Some(super::indexes::PathQueryCondition::Gt(
                        self.filter_evaluator.sql_value_to_index_value(value)?,
                    ))
                } else {
                    None
                }
            }
            DocFilterOperator::Gte => {
                if let Some(ref value) = condition.value {
                    Some(super::indexes::PathQueryCondition::Gte(
                        self.filter_evaluator.sql_value_to_index_value(value)?,
                    ))
                } else {
                    None
                }
            }
            DocFilterOperator::Lt => {
                if let Some(ref value) = condition.value {
                    Some(super::indexes::PathQueryCondition::Lt(
                        self.filter_evaluator.sql_value_to_index_value(value)?,
                    ))
                } else {
                    None
                }
            }
            DocFilterOperator::Lte => {
                if let Some(ref value) = condition.value {
                    Some(super::indexes::PathQueryCondition::Lte(
                        self.filter_evaluator.sql_value_to_index_value(value)?,
                    ))
                } else {
                    None
                }
            }
            _ => None, // Other operators may not use indexes
        };

        if let Some(cond) = query_condition {
            let candidates = index_manager
                .query_path_index(collection, path, &cond)
                .await?;
            if !candidates.is_empty() {
                return Ok(Some(candidates));
            }
        }

        Ok(None)
    }

    /// Load documents and apply filters
    async fn load_and_filter(
        &self,
        documents: &[DocumentRecord],
        candidates: &[String],
        params: &DocumentQueryParams,
    ) -> Result<Vec<DocumentRecord>> {
        let candidate_ids: Option<std::collections::HashSet<&str>> = if candidates.is_empty() {
            None
        } else {
            Some(candidates.iter().map(|id| id.as_str()).collect())
        };

        let documents: Vec<DocumentRecord> = documents
            .iter()
            .filter(|doc| {
                candidate_ids
                    .as_ref()
                    .is_none_or(|ids| ids.contains(doc.id.as_str()))
            })
            .cloned()
            .collect();

        // Apply filter to loaded documents
        if let Some(ref filter) = params.filter {
            return Ok(documents
                .into_iter()
                .filter(|doc| self.filter_evaluator.evaluate(filter, doc))
                .collect());
        }

        Ok(documents)
    }

    /// Sort documents by the given fields
    fn sort_documents(
        &self,
        documents: &mut Vec<DocumentRecord>,
        sort_fields: &[SortField],
    ) -> Result<()> {
        documents.sort_by(|a, b| {
            for field in sort_fields {
                let order = SortOrder::try_from(field.order).unwrap_or(SortOrder::Asc);
                let cmp = self.compare_by_path(&a.document, &b.document, &field.path);
                let cmp = match order {
                    SortOrder::Desc => cmp.reverse(),
                    _ => cmp,
                };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        });

        Ok(())
    }

    /// Compare two documents by a JSON path
    fn compare_by_path(&self, a: &SqlObject, b: &SqlObject, path: &str) -> std::cmp::Ordering {
        let val_a = self.extract_value(a, path);
        let val_b = self.extract_value(b, path);

        match (val_a, val_b) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => self.compare_sql_values(&a, &b),
        }
    }

    fn compare_sql_values(&self, a: &SqlValue, b: &SqlValue) -> std::cmp::Ordering {
        match (&a.value, &b.value) {
            (Some(SqlValueVariant::NullValue(_)), Some(SqlValueVariant::NullValue(_))) => {
                std::cmp::Ordering::Equal
            }
            (Some(SqlValueVariant::NullValue(_)), _) => std::cmp::Ordering::Less,
            (_, Some(SqlValueVariant::NullValue(_))) => std::cmp::Ordering::Greater,
            (Some(SqlValueVariant::BoolValue(va)), Some(SqlValueVariant::BoolValue(vb))) => {
                va.cmp(vb)
            }
            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::Int64Value(vb))) => {
                va.cmp(vb)
            }
            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::NumberValue(vb))) => {
                va.total_cmp(vb)
            }
            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::NumberValue(vb))) => {
                (*va as f64).total_cmp(vb)
            }
            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::Int64Value(vb))) => {
                va.total_cmp(&(*vb as f64))
            }
            (Some(SqlValueVariant::StringValue(va)), Some(SqlValueVariant::StringValue(vb))) => {
                va.cmp(vb)
            }
            _ => std::cmp::Ordering::Equal,
        }
    }

    fn extract_value(&self, doc: &SqlObject, path: &str) -> Option<SqlValue> {
        let json_doc = self.sql_object_to_json(doc);
        let normalized_path = self.normalize_path(path);

        match json_doc.path(&normalized_path) {
            Ok(result) => match &result {
                JsonValue::Array(arr) if arr.len() == 1 => {
                    if arr[0].is_null() {
                        None
                    } else {
                        self.json_to_sql_value(&arr[0])
                    }
                }
                JsonValue::Array(arr) if arr.is_empty() => None,
                JsonValue::Null => None,
                _ => self.json_to_sql_value(&result),
            },
            Err(_) => None,
        }
    }

    fn normalize_path(&self, path: &str) -> String {
        if path.starts_with("$.") || path.starts_with('$') {
            path.to_string()
        } else {
            format!("$.{}", path)
        }
    }

    fn sql_object_to_json(&self, obj: &SqlObject) -> JsonValue {
        let mut map = serde_json::Map::new();
        for (key, value) in &obj.fields {
            if let Some(json_val) = self.sql_value_to_json(value) {
                map.insert(key.clone(), json_val);
            }
        }
        JsonValue::Object(map)
    }

    fn sql_value_to_json(&self, value: &SqlValue) -> Option<JsonValue> {
        match &value.value {
            Some(SqlValueVariant::NullValue(_)) => Some(JsonValue::Null),
            Some(SqlValueVariant::BoolValue(b)) => Some(JsonValue::Bool(*b)),
            Some(SqlValueVariant::Int64Value(i)) => Some(JsonValue::Number((*i).into())),
            Some(SqlValueVariant::NumberValue(f)) => {
                serde_json::Number::from_f64(*f).map(JsonValue::Number)
            }
            Some(SqlValueVariant::StringValue(s)) => Some(JsonValue::String(s.clone())),
            Some(SqlValueVariant::BytesValue(bytes)) => {
                let hex: String = bytes.iter().map(|byte| format!("{:02x}", byte)).collect();
                Some(JsonValue::String(format!("0x{}", hex)))
            }
            Some(SqlValueVariant::ArrayValue(arr)) => Some(JsonValue::Array(
                arr.values
                    .iter()
                    .filter_map(|value| self.sql_value_to_json(value))
                    .collect(),
            )),
            Some(SqlValueVariant::ObjectValue(obj)) => Some(self.sql_object_to_json(obj)),
            None => None,
        }
    }

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
                } else { n.as_f64().map(|f| SqlValue {
                        value: Some(SqlValueVariant::NumberValue(f)),
                    }) }
            }
            JsonValue::String(s) => Some(SqlValue {
                value: Some(SqlValueVariant::StringValue(s.clone())),
            }),
            JsonValue::Array(arr) => Some(SqlValue {
                value: Some(SqlValueVariant::ArrayValue(
                    crate::proto::proximadb_v1::SqlArray {
                        values: arr
                            .iter()
                            .filter_map(|item| self.json_to_sql_value(item))
                            .collect(),
                    },
                )),
            }),
            JsonValue::Object(obj) => Some(SqlValue {
                value: Some(SqlValueVariant::ObjectValue(SqlObject {
                    fields: obj
                        .iter()
                        .filter_map(|(key, value)| {
                            self.json_to_sql_value(value)
                                .map(|value| (key.clone(), value))
                        })
                        .collect(),
                })),
            }),
        }
    }
}

impl Default for QueryExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::proto::proximadb_v1::{DocumentFilter, SqlObject, SqlValue, sql_value::Value};
    use crate::storage::document::{DocumentQueryParams, DocumentRecord};

    #[test]
    fn test_query_executor_new() {
        let _executor = QueryExecutor::new();
        // Basic instantiation test
        assert!(true);
    }

    #[tokio::test]
    async fn test_execute_filters_and_sorts_in_memory_documents() {
        let executor = QueryExecutor::new();
        let index_manager = IndexManager::new();

        let documents = vec![
            DocumentRecord {
                id: "doc1".to_string(),
                document: SqlObject {
                    fields: HashMap::from([
                        (
                            "status".to_string(),
                            SqlValue {
                                value: Some(Value::StringValue("inactive".to_string())),
                            },
                        ),
                        (
                            "age".to_string(),
                            SqlValue {
                                value: Some(Value::Int64Value(40)),
                            },
                        ),
                    ]),
                },
                version: 1,
                collection_id: "users".to_string(),
                updated_at_ns: 0,
                schema_id: None,
                document_type: None,
            },
            DocumentRecord {
                id: "doc2".to_string(),
                document: SqlObject {
                    fields: HashMap::from([
                        (
                            "status".to_string(),
                            SqlValue {
                                value: Some(Value::StringValue("active".to_string())),
                            },
                        ),
                        (
                            "age".to_string(),
                            SqlValue {
                                value: Some(Value::Int64Value(20)),
                            },
                        ),
                    ]),
                },
                version: 1,
                collection_id: "users".to_string(),
                updated_at_ns: 0,
                schema_id: None,
                document_type: None,
            },
        ];

        let params = DocumentQueryParams {
            filter: Some(DocumentFilter {
                conditions: vec![DocFilterCondition {
                    path: "status".to_string(),
                    operator: DocFilterOperator::Eq as i32,
                    value: Some(SqlValue {
                        value: Some(Value::StringValue("active".to_string())),
                    }),
                    values: Vec::new(),
                }],
                or_filters: Vec::new(),
                and_filters: Vec::new(),
            }),
            projection: Vec::new(),
            sort: vec![SortField {
                path: "age".to_string(),
                order: SortOrder::Desc as i32,
            }],
            limit: 10,
            offset: 0,
            include_count: true,
        };

        let (result, total_count) = executor
            .execute("users", &documents, &params, &index_manager)
            .await
            .expect("document query should execute");

        assert_eq!(total_count, 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "doc2");
    }
}
