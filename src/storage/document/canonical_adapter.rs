//! Compatibility adapter between the legacy document storage surface and the
//! canonical document record contract.
//!
//! The architectural direction is documented in
//! `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`:
//! document APIs are facades over durable `ProximaRecord` storage, while v1
//! `SqlObject` remains only at compatibility edges until the public service
//! contracts finish moving to v2 `ProximaValue`/`ProximaRecord`.

use proximadb_document::{
    CanonicalDocument, DocumentRecordMetadata, canonical_document_from_record,
};
use proximadb_records::conversions::{proxima_to_sql_value, sql_value_to_proxima};
use proximadb_records::{ProximaRecord, ProximaTree, ProximaTreeNode};

use crate::proto::proximadb_v1::{SqlObject, SqlValue, sql_value};
use crate::storage::document::DocumentRecord;

/// Convert the legacy document record shape into the canonical document facade
/// representation used by the document modality crate.
pub fn legacy_document_to_canonical(record: &DocumentRecord) -> CanonicalDocument {
    let mut canonical = CanonicalDocument::new(
        record.collection_id.clone(),
        record.id.clone(),
        sql_object_to_proxima_tree(&record.document),
    );

    canonical.metadata = DocumentRecordMetadata {
        schema_id: record.schema_id.clone(),
        document_type: record.document_type.clone(),
        version: record.version,
        updated_at_ns: Some(record.updated_at_ns),
        ..DocumentRecordMetadata::default()
    };

    canonical
}

/// Convert a legacy document record directly into the durable canonical record
/// envelope.
pub fn legacy_document_to_proxima_record(record: &DocumentRecord) -> ProximaRecord {
    legacy_document_to_canonical(record).into_proxima_record()
}

/// Rebuild the legacy document record surface from a canonical document facade
/// record.
pub fn canonical_document_to_legacy(document: CanonicalDocument) -> DocumentRecord {
    DocumentRecord {
        id: document.key.document_id,
        document: proxima_tree_to_sql_object(&document.document),
        props: document.document,
        version: document.metadata.version,
        collection_id: document.key.collection_id,
        updated_at_ns: document.metadata.updated_at_ns.unwrap_or(0),
        schema_id: document.metadata.schema_id,
        document_type: document.metadata.document_type,
    }
}

/// Rebuild the legacy document record surface from a durable canonical record.
///
/// Returns `None` when the supplied record is not labelled as a document facade
/// record.
pub fn proxima_record_to_legacy_document(record: &ProximaRecord) -> Option<DocumentRecord> {
    canonical_document_from_record(record).map(canonical_document_to_legacy)
}

/// Convert legacy v1 `SqlObject` document fields into an NF2 property tree.
pub fn sql_object_to_proxima_tree(object: &SqlObject) -> ProximaTree {
    object
        .fields
        .iter()
        .map(|(key, value)| (key.clone(), sql_value_to_tree_node(value)))
        .collect()
}

/// Convert an NF2 property tree back to the legacy v1 `SqlObject` edge shape.
pub fn proxima_tree_to_sql_object(tree: &ProximaTree) -> SqlObject {
    SqlObject {
        fields: tree
            .iter()
            .map(|(key, node)| (key.clone(), tree_node_to_sql_value(node)))
            .collect(),
    }
}

/// Lift a legacy v1 `SqlValue` into an NF² tree node (object values become
/// nested sub-trees; everything else a canonical leaf). Used by the document
/// update-mutation path when applying a proto `$set` value onto `props`.
pub fn sql_value_to_tree_node(value: &SqlValue) -> ProximaTreeNode {
    match &value.value {
        Some(sql_value::Value::ObjectValue(object)) => {
            ProximaTreeNode::Object(sql_object_to_proxima_tree(object))
        }
        _ => ProximaTreeNode::Value(sql_value_to_proxima(value)),
    }
}

fn tree_node_to_sql_value(node: &ProximaTreeNode) -> SqlValue {
    match node {
        ProximaTreeNode::Value(value) => proxima_to_sql_value(value),
        ProximaTreeNode::Object(tree) => SqlValue {
            value: Some(sql_value::Value::ObjectValue(proxima_tree_to_sql_object(
                tree,
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn string_value(value: &str) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::StringValue(value.to_string())),
        }
    }

    fn int_value(value: i64) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::Int64Value(value)),
        }
    }

    fn object(fields: Vec<(&str, SqlValue)>) -> SqlObject {
        SqlObject {
            fields: HashMap::from_iter(
                fields
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), value)),
            ),
        }
    }

    fn legacy_record() -> DocumentRecord {
        let document = object(vec![
            ("title", string_value("Architecture")),
            (
                "author",
                SqlValue {
                    value: Some(sql_value::Value::ObjectValue(object(vec![(
                        "name",
                        string_value("Ada"),
                    )]))),
                },
            ),
            ("revision", int_value(3)),
        ]);
        let props = sql_object_to_proxima_tree(&document);
        DocumentRecord {
            id: "doc-1".to_string(),
            document,
            props,
            version: 7,
            collection_id: "papers".to_string(),
            updated_at_ns: 42,
            schema_id: Some("paper-v1".to_string()),
            document_type: Some("research-paper".to_string()),
        }
    }

    #[test]
    fn maps_legacy_document_to_canonical_record() {
        let canonical = legacy_document_to_canonical(&legacy_record());

        assert_eq!(canonical.key.collection_id, "papers");
        assert_eq!(canonical.key.document_id, "doc-1");
        assert_eq!(canonical.metadata.schema_id.as_deref(), Some("paper-v1"));
        assert_eq!(
            canonical.metadata.document_type.as_deref(),
            Some("research-paper")
        );
        assert_eq!(canonical.metadata.version, 7);
        assert_eq!(canonical.metadata.updated_at_ns, Some(42));

        assert!(matches!(
            canonical.document.get("author"),
            Some(ProximaTreeNode::Object(_))
        ));
    }

    #[test]
    fn round_trips_legacy_document_through_proxima_record() {
        let legacy = legacy_record();
        let record = legacy_document_to_proxima_record(&legacy);
        let rebuilt = proxima_record_to_legacy_document(&record).expect("document record");

        assert_eq!(rebuilt.id, legacy.id);
        assert_eq!(rebuilt.collection_id, legacy.collection_id);
        assert_eq!(rebuilt.version, legacy.version);
        assert_eq!(rebuilt.updated_at_ns, legacy.updated_at_ns);
        assert_eq!(rebuilt.schema_id, legacy.schema_id);
        assert_eq!(rebuilt.document_type, legacy.document_type);
        assert_eq!(
            rebuilt.document.fields.get("title"),
            legacy.document.fields.get("title")
        );

        let author = rebuilt.document.fields.get("author").expect("author field");
        assert!(matches!(
            &author.value,
            Some(sql_value::Value::ObjectValue(object)) if object.fields.contains_key("name")
        ));
    }

    #[test]
    fn ignores_non_document_proxima_records() {
        assert!(proxima_record_to_legacy_document(&ProximaRecord::default()).is_none());
    }
}
