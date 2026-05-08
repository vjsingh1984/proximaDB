//! Pure vector-query adaptation helpers shared across query surfaces.

use std::collections::HashMap;

use proximadb_data_model::DataModel;
use proximadb_proto::proximadb_v1::SqlValue;

use crate::UnifiedRecord;

/// Convert SQL-typed vector metadata into the unified string metadata contract.
pub fn build_vector_metadata(metadata: &HashMap<String, SqlValue>) -> HashMap<String, String> {
    metadata
        .iter()
        .map(|(key, value)| (key.clone(), format!("{:?}", value)))
        .collect()
}

/// Build a unified record from a vector search hit.
pub fn build_vector_search_record(
    id: &str,
    score: f32,
    metadata: HashMap<String, String>,
) -> UnifiedRecord {
    UnifiedRecord {
        id: id.to_string(),
        source_model: DataModel::Vector,
        data: serde_json::json!({
            "id": id,
            "score": score,
        }),
        score: Some(score as f64),
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_proto::proximadb_v1::sql_value::Value;

    #[test]
    fn build_vector_metadata_formats_sql_values() {
        let metadata = HashMap::from([(
            "tenant".to_string(),
            SqlValue {
                value: Some(Value::StringValue("acme".to_string())),
            },
        )]);

        let converted = build_vector_metadata(&metadata);
        let tenant = converted.get("tenant").expect("tenant metadata");
        assert!(tenant.contains("StringValue"));
        assert!(tenant.contains("acme"));
    }

    #[test]
    fn build_vector_search_record_preserves_vector_shape() {
        let record = build_vector_search_record(
            "vec_1",
            0.91,
            HashMap::from([("tenant".to_string(), "acme".to_string())]),
        );

        assert_eq!(record.id, "vec_1");
        assert_eq!(record.source_model, DataModel::Vector);
        assert_eq!(record.data["id"], "vec_1");
        assert_eq!(record.score, Some(0.91_f32 as f64));
        assert_eq!(record.metadata.get("tenant"), Some(&"acme".to_string()));
    }
}
