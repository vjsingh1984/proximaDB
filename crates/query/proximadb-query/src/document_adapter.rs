//! Pure document-query adaptation helpers shared across query surfaces.

use proximadb_data_model::DataModel;
use proximadb_document_query::PathFilter;
use proximadb_proto::proximadb_v1::{
    DocFilterCondition, DocFilterOperator, DocumentFilter, SqlObject, SqlValue,
    sql_value::Value as SqlValueVariant,
};
use proximadb_query_filter::{FilterOperator, FilterValue};

use crate::UnifiedRecord;

/// Convert a protobuf `SqlObject` into JSON.
pub fn sql_object_to_json(obj: &proximadb_proto::proximadb_v1::SqlObject) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in &obj.fields {
        map.insert(key.clone(), sql_value_to_json(value));
    }
    serde_json::Value::Object(map)
}

/// Convert a protobuf `SqlValue` into JSON.
pub fn sql_value_to_json(value: &SqlValue) -> serde_json::Value {
    match &value.value {
        Some(SqlValueVariant::NullValue(_)) => serde_json::Value::Null,
        Some(SqlValueVariant::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(SqlValueVariant::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(SqlValueVariant::NumberValue(f)) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Some(SqlValueVariant::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(SqlValueVariant::BytesValue(b)) => {
            let encoded: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
            serde_json::Value::String(encoded)
        }
        Some(SqlValueVariant::ArrayValue(arr)) => {
            let items: Vec<serde_json::Value> = arr.values.iter().map(sql_value_to_json).collect();
            serde_json::Value::Array(items)
        }
        Some(SqlValueVariant::ObjectValue(obj)) => sql_object_to_json(obj),
        None => serde_json::Value::Null,
    }
}

/// Convert query-IR path filters into the protobuf `DocumentFilter` contract.
pub fn convert_path_filters_to_document_filter(filters: &[PathFilter]) -> Option<DocumentFilter> {
    if filters.is_empty() {
        return None;
    }

    let conditions: Vec<DocFilterCondition> = filters
        .iter()
        .map(|pf| {
            let operator = match pf.operator {
                FilterOperator::Eq => DocFilterOperator::Eq,
                FilterOperator::Ne => DocFilterOperator::Ne,
                FilterOperator::Gt => DocFilterOperator::Gt,
                FilterOperator::Gte => DocFilterOperator::Gte,
                FilterOperator::Lt => DocFilterOperator::Lt,
                FilterOperator::Lte => DocFilterOperator::Lte,
                FilterOperator::In => DocFilterOperator::In,
                FilterOperator::NotIn => DocFilterOperator::NotIn,
                FilterOperator::Contains => DocFilterOperator::Contains,
                FilterOperator::StartsWith => DocFilterOperator::Regex,
                FilterOperator::EndsWith => DocFilterOperator::Regex,
                FilterOperator::Exists => DocFilterOperator::Exists,
                FilterOperator::Type => DocFilterOperator::Type,
            };

            DocFilterCondition {
                path: pf.path.clone(),
                operator: operator.into(),
                value: Some(convert_filter_value_to_sql(&pf.value)),
                values: vec![],
            }
        })
        .collect();

    Some(DocumentFilter {
        conditions,
        or_filters: vec![],
        and_filters: vec![],
    })
}

/// Convert unified `FilterValue` into protobuf `SqlValue`.
pub fn convert_filter_value_to_sql(value: &FilterValue) -> SqlValue {
    match value {
        FilterValue::String(s) => SqlValue {
            value: Some(SqlValueVariant::StringValue(s.clone())),
        },
        FilterValue::Number(n) => SqlValue {
            value: Some(SqlValueVariant::NumberValue(*n)),
        },
        FilterValue::Bool(b) => SqlValue {
            value: Some(SqlValueVariant::BoolValue(*b)),
        },
        FilterValue::Null => SqlValue {
            value: Some(SqlValueVariant::NullValue(0)),
        },
        FilterValue::Array(arr) => {
            let sql_arr = proximadb_proto::proximadb_v1::SqlArray {
                values: arr.iter().map(convert_filter_value_to_sql).collect(),
            };
            SqlValue {
                value: Some(SqlValueVariant::ArrayValue(sql_arr)),
            }
        }
    }
}

/// Build a unified record from a document query result.
pub fn build_document_query_record(id: &str, document: &SqlObject) -> UnifiedRecord {
    UnifiedRecord {
        id: id.to_string(),
        source_model: DataModel::Document,
        data: sql_object_to_json(document),
        score: None,
        metadata: std::collections::HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_proto::proximadb_v1::{SqlObject, sql_value::Value};

    #[test]
    fn sql_value_to_json_primitives() {
        let null_val = SqlValue {
            value: Some(Value::NullValue(0)),
        };
        assert_eq!(sql_value_to_json(&null_val), serde_json::Value::Null);

        let bool_val = SqlValue {
            value: Some(Value::BoolValue(true)),
        };
        assert_eq!(sql_value_to_json(&bool_val), serde_json::Value::Bool(true));

        let int_val = SqlValue {
            value: Some(Value::Int64Value(42)),
        };
        assert_eq!(sql_value_to_json(&int_val), serde_json::json!(42));

        let str_val = SqlValue {
            value: Some(Value::StringValue("hello".to_string())),
        };
        assert_eq!(sql_value_to_json(&str_val), serde_json::json!("hello"));
    }

    #[test]
    fn sql_object_to_json_recurses_nested_values() {
        let obj = SqlObject {
            fields: std::collections::HashMap::from([
                (
                    "name".to_string(),
                    SqlValue {
                        value: Some(Value::StringValue("alice".to_string())),
                    },
                ),
                (
                    "age".to_string(),
                    SqlValue {
                        value: Some(Value::Int64Value(42)),
                    },
                ),
            ]),
        };

        assert_eq!(
            sql_object_to_json(&obj),
            serde_json::json!({"name": "alice", "age": 42})
        );
    }

    #[test]
    fn path_filters_convert_to_document_filter_contract() {
        let filters = vec![PathFilter {
            path: "$.category".to_string(),
            operator: FilterOperator::Eq,
            value: FilterValue::String("electronics".to_string()),
        }];

        let filter = convert_path_filters_to_document_filter(&filters).expect("filter");
        assert_eq!(filter.conditions.len(), 1);
        assert_eq!(filter.conditions[0].path, "$.category");
    }

    #[test]
    fn filter_value_array_converts_to_sql_array() {
        let value = FilterValue::Array(vec![
            FilterValue::String("a".to_string()),
            FilterValue::Number(2.0),
        ]);
        let sql = convert_filter_value_to_sql(&value);
        match sql.value {
            Some(SqlValueVariant::ArrayValue(arr)) => assert_eq!(arr.values.len(), 2),
            other => panic!("expected array value, got {:?}", other),
        }
    }

    #[test]
    fn build_document_query_record_preserves_document_shape() {
        let document = SqlObject {
            fields: std::collections::HashMap::from([(
                "title".to_string(),
                SqlValue {
                    value: Some(Value::StringValue("Doc".to_string())),
                },
            )]),
        };

        let record = build_document_query_record("doc_1", &document);
        assert_eq!(record.id, "doc_1");
        assert_eq!(record.source_model, DataModel::Document);
        assert_eq!(record.data["title"], "Doc");
        assert!(record.score.is_none());
    }
}
