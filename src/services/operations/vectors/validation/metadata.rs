//! Pseudo query generation for metadata-enriched vector search.
//!
//! This module provides utilities for generating derived metadata entries
//! that enable bounded, auditable dataset lookup capabilities.

use std::collections::HashMap;

use proximadb_data_model::ProximaValue;

pub const PROXIMADB_PSEUDO_QUERY_FIELD: &str = "proximadb.pseudo_query";
pub const PROXIMADB_PSEUDO_QUERY_SOURCE_FIELD: &str = "proximadb.pseudo_query_source_fields";

/// Generate derived metadata entries for bounded, auditable dataset lookup.
pub trait PseudoQueryGenerator: Send + Sync {
    /// Build additional metadata entries from source metadata.
    fn generate_metadata(
        &self,
        metadata: &HashMap<String, ProximaValue>,
    ) -> HashMap<String, ProximaValue>;
}

#[derive(Debug, Default)]
pub struct DefaultPseudoQueryGenerator;

impl DefaultPseudoQueryGenerator {
    fn extract_text(value: &ProximaValue) -> Option<String> {
        match value {
            ProximaValue::String(v) | ProximaValue::Symbol(v) | ProximaValue::Decimal(v) => {
                Some(v.clone())
            }
            ProximaValue::Int8(v) => Some(v.to_string()),
            ProximaValue::Int16(v) => Some(v.to_string()),
            ProximaValue::Int32(v) => Some(v.to_string()),
            ProximaValue::Int64(v) => Some(v.to_string()),
            ProximaValue::UInt8(v) => Some(v.to_string()),
            ProximaValue::UInt16(v) => Some(v.to_string()),
            ProximaValue::UInt32(v) => Some(v.to_string()),
            ProximaValue::UInt64(v) => Some(v.to_string()),
            ProximaValue::Float32(v) => Some(v.to_string()),
            ProximaValue::Float64(v) => Some(v.to_string()),
            ProximaValue::Boolean(v) => Some(v.to_string()),
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
        metadata: &HashMap<String, ProximaValue>,
    ) -> HashMap<String, ProximaValue> {
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
            ProximaValue::String(pseudo_query),
        );
        generated.insert(
            PROXIMADB_PSEUDO_QUERY_SOURCE_FIELD.to_string(),
            ProximaValue::String(source_fields.join(",")),
        );

        generated
    }
}

/// Apply pseudo query metadata to a batch of canonical ProximaRecord envelopes.
pub fn apply_pseudo_query_metadata(
    records: &mut [proximadb_records::ProximaRecord],
    pseudo_query_generator: &dyn PseudoQueryGenerator,
) {
    for record in records.iter_mut() {
        let flat: HashMap<String, ProximaValue> = record
            .props
            .iter()
            .filter_map(|(k, node)| {
                if let proximadb_records::ProximaTreeNode::Value(v) = node {
                    Some((k.clone(), v.clone()))
                } else {
                    None
                }
            })
            .collect();
        let generated = pseudo_query_generator.generate_metadata(&flat);
        for (key, value) in generated {
            record.props.entry(key).or_insert_with(|| {
                proximadb_records::ProximaTreeNode::Value(value)
            });
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
            ProximaValue::String("Test Document Title".to_string()),
        );
        metadata.insert(
            "content".to_string(),
            ProximaValue::String("This is test content with multiple words".to_string()),
        );

        let result = generator.generate_metadata(&metadata);

        assert!(result.contains_key(PROXIMADB_PSEUDO_QUERY_FIELD));
        assert!(result.contains_key(PROXIMADB_PSEUDO_QUERY_SOURCE_FIELD));

        let pseudo_query = result.get(PROXIMADB_PSEUDO_QUERY_FIELD).unwrap();
        if let ProximaValue::String(s) = pseudo_query {
            assert!(s.contains("test"));
            assert!(s.contains("document"));
            assert!(s.contains("title"));
        } else {
            panic!("pseudo query should be a string");
        }
    }
}
