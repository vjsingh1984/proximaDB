//! Pseudo query generation for metadata-enriched vector search.
//!
//! This module provides utilities for generating derived metadata entries
//! that enable bounded, auditable dataset lookup capabilities.

use std::collections::HashMap;

pub const PROXIMADB_PSEUDO_QUERY_FIELD: &str = "proximadb.pseudo_query";
pub const PROXIMADB_PSEUDO_QUERY_SOURCE_FIELD: &str = "proximadb.pseudo_query_source_fields";

/// Generate derived metadata entries for bounded, auditable dataset lookup.
pub trait PseudoQueryGenerator: Send + Sync {
    /// Build additional metadata entries from source metadata.
    fn generate_metadata(
        &self,
        metadata: &HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> HashMap<String, crate::proto::proximadb_v1::SqlValue>;
}

#[derive(Debug, Default)]
pub struct DefaultPseudoQueryGenerator;

impl DefaultPseudoQueryGenerator {
    fn extract_text(value: &crate::proto::proximadb_v1::SqlValue) -> Option<String> {
        use crate::proto::proximadb_v1::sql_value::Value as SqlValueVariant;

        match value.value.as_ref()? {
            SqlValueVariant::StringValue(v) => Some(v.clone()),
            SqlValueVariant::Int64Value(v) => Some(v.to_string()),
            SqlValueVariant::NumberValue(v) => Some(v.to_string()),
            SqlValueVariant::BoolValue(v) => Some(v.to_string()),
            _ => None,
        }
    }

    fn sanitize_terms(input: &str) -> String {
        input
            .split_whitespace()
            .map(|token| token.trim().to_lowercase())
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl PseudoQueryGenerator for DefaultPseudoQueryGenerator {
    fn generate_metadata(
        &self,
        metadata: &HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> HashMap<String, crate::proto::proximadb_v1::SqlValue> {
        use crate::proto::proximadb_v1::sql_value::Value as SqlValueVariant;

        let candidate_fields = [
            "title",
            "content",
            "description",
            "summary",
            "body",
            "category",
            "tags",
        ];

        let mut source_fields = Vec::new();
        let mut terms = Vec::new();

        for field in candidate_fields {
            if let Some(value) = metadata.get(field).and_then(Self::extract_text) {
                let normalized = Self::sanitize_terms(&value);
                if !normalized.is_empty() {
                    source_fields.push(field.to_string());
                    terms.push(normalized);
                }
            }
        }

        if terms.is_empty() {
            return HashMap::new();
        }

        let mut pseudo_query = terms.join(" ");
        if pseudo_query.len() > 512 {
            pseudo_query.truncate(512);
        }

        let mut generated = HashMap::new();
        generated.insert(
            PROXIMADB_PSEUDO_QUERY_FIELD.to_string(),
            crate::proto::proximadb_v1::SqlValue {
                value: Some(SqlValueVariant::StringValue(pseudo_query)),
            },
        );
        generated.insert(
            PROXIMADB_PSEUDO_QUERY_SOURCE_FIELD.to_string(),
            crate::proto::proximadb_v1::SqlValue {
                value: Some(SqlValueVariant::StringValue(source_fields.join(","))),
            },
        );

        generated
    }
}

/// Apply pseudo query metadata to a batch of vector records.
pub fn apply_pseudo_query_metadata(
    vectors: &mut [crate::proto::proximadb_v1::VectorRecord],
    pseudo_query_generator: &dyn PseudoQueryGenerator,
) {
    for vector in vectors.iter_mut() {
        let generated = pseudo_query_generator.generate_metadata(&vector.metadata);
        for (key, value) in generated {
            vector.metadata.entry(key).or_insert(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_pseudo_query_generator() {
        let generator = DefaultPseudoQueryGenerator::default();
        let mut metadata = HashMap::new();
        metadata.insert(
            "title".to_string(),
            crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    "Test Document Title".to_string(),
                )),
            },
        );
        metadata.insert(
            "content".to_string(),
            crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    "This is test content with multiple words".to_string(),
                )),
            },
        );

        let result = generator.generate_metadata(&metadata);

        assert!(result.contains_key(PROXIMADB_PSEUDO_QUERY_FIELD));
        assert!(result.contains_key(PROXIMADB_PSEUDO_QUERY_SOURCE_FIELD));

        let pseudo_query = result.get(PROXIMADB_PSEUDO_QUERY_FIELD).unwrap();
        if let Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) =
            &pseudo_query.value
        {
            assert!(s.contains("test"));
            assert!(s.contains("document"));
            assert!(s.contains("title"));
        }
    }
}
