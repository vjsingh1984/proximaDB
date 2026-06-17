//! Rebuildable document projection contracts.
//!
//! JSON path indexes, array indexes, full-text indexes, and columnar variation
//! tables are Layer 2 projections over canonical `ProximaRecord` data. This
//! module defines that boundary without giving any projection its own durable
//! source-of-truth semantics.

use async_trait::async_trait;
use proximadb_data_model::ProximaValue;
use proximadb_kernel::error::ProximaDBError;
use proximadb_records::{ProximaRecord, ProximaTree, ProximaTreeNode, tree_get};

use crate::record::{DOCUMENT_COLLECTION_PROP, DOCUMENT_RECORD_LABEL};

/// Document projection family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentProjectionKind {
    /// JSON path / generated-column style lookup.
    JsonPath,
    /// Array-membership or unnested array lookup.
    Array,
    /// Full-text or inverted text index.
    FullText,
    /// Stable-shape columnar projection for a schema variation.
    ColumnarVariation,
}

/// Normalized document property path.
///
/// Paths are stored without leading `$` or `.` so they can be applied directly
/// to `ProximaRecord.props`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentPath(String);

impl DocumentPath {
    pub fn new(path: impl Into<String>) -> Self {
        let raw = path.into();
        let normalized = raw
            .trim()
            .trim_start_matches('$')
            .trim_start_matches('.')
            .to_string();
        Self(normalized)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Catalog-facing descriptor for a document projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentProjectionDescriptor {
    /// Projection/index name.
    pub name: String,
    /// User-facing collection this projection indexes.
    pub collection_id: String,
    /// Projection family.
    pub kind: DocumentProjectionKind,
    /// Source document paths consumed by this projection.
    pub source_paths: Vec<DocumentPath>,
    /// Optional schema variation id for stable-shape projections.
    pub variation_id: Option<String>,
    /// Whether this projection can be dropped and rebuilt from canonical records.
    pub rebuildable: bool,
}

impl DocumentProjectionDescriptor {
    pub fn new(
        name: impl Into<String>,
        collection_id: impl Into<String>,
        kind: DocumentProjectionKind,
        source_paths: Vec<DocumentPath>,
    ) -> Self {
        Self {
            name: name.into(),
            collection_id: collection_id.into(),
            kind,
            source_paths,
            variation_id: None,
            rebuildable: true,
        }
    }
}

/// Result of applying a canonical record to a projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionApplyResult {
    /// Canonical record id that was consumed.
    pub record_oid: String,
    /// Number of projection entries updated.
    pub entries_written: usize,
}

/// Rebuildable document projection contract.
#[async_trait]
pub trait DocumentProjection: Send + Sync {
    fn descriptor(&self) -> &DocumentProjectionDescriptor;

    /// Apply one canonical record to the projection.
    async fn apply_record(
        &self,
        record: &ProximaRecord,
    ) -> Result<ProjectionApplyResult, ProximaDBError>;

    /// Remove one canonical record from the projection.
    async fn remove_record(&self, record: &ProximaRecord) -> Result<bool, ProximaDBError>;

    /// Drop and rebuild the projection from canonical records.
    async fn rebuild_from_records(
        &self,
        records: &[ProximaRecord],
    ) -> Result<usize, ProximaDBError>;
}

/// Return true if a canonical record belongs to a document collection.
pub fn record_belongs_to_document_collection(record: &ProximaRecord, collection_id: &str) -> bool {
    if !record.labels.contains(DOCUMENT_RECORD_LABEL) {
        return false;
    }

    matches!(
        record.props.get(DOCUMENT_COLLECTION_PROP),
        Some(ProximaTreeNode::Value(ProximaValue::String(value))) if value == collection_id
    )
}

/// Resolve a document path against a canonical record.
pub fn document_value_at_path<'a>(
    record: &'a ProximaRecord,
    path: &DocumentPath,
) -> Option<&'a ProximaValue> {
    tree_get(&record.props, path.as_str())
}

/// Extract values required by a projection descriptor from a canonical record.
pub fn projection_source_values<'a>(
    record: &'a ProximaRecord,
    descriptor: &'a DocumentProjectionDescriptor,
) -> Vec<(&'a str, &'a ProximaValue)> {
    descriptor
        .source_paths
        .iter()
        .filter_map(|path| document_value_at_path(record, path).map(|value| (path.as_str(), value)))
        .collect()
}

/// Helper for test/projection builders that need nested canonical document props.
pub fn object_node(fields: impl IntoIterator<Item = (String, ProximaTreeNode)>) -> ProximaTreeNode {
    ProximaTreeNode::Object(ProximaTree::from_iter(fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{CanonicalDocument, DocumentRecordMetadata};
    use std::collections::HashMap;

    fn value_string(value: &str) -> ProximaTreeNode {
        ProximaTreeNode::Value(ProximaValue::String(value.to_string()))
    }

    fn value_i64(value: i64) -> ProximaTreeNode {
        ProximaTreeNode::Value(ProximaValue::Int64(value))
    }

    fn document_record() -> ProximaRecord {
        let document = HashMap::from([
            ("title".to_string(), value_string("Design notes")),
            (
                "author".to_string(),
                object_node([("name".to_string(), value_string("Ada"))]),
            ),
            ("revision".to_string(), value_i64(3)),
        ]);
        let mut canonical = CanonicalDocument::new("docs", "doc-1", document);
        canonical.metadata = DocumentRecordMetadata {
            version: 3,
            ..DocumentRecordMetadata::default()
        };
        canonical.into_proxima_record()
    }

    #[test]
    fn document_path_normalizes_json_path_prefixes() {
        assert_eq!(DocumentPath::new("$.author.name").as_str(), "author.name");
        assert_eq!(DocumentPath::new(".title").as_str(), "title");
        assert_eq!(DocumentPath::new("revision").as_str(), "revision");
    }

    #[test]
    fn document_path_reads_canonical_record_props() {
        let record = document_record();
        let value = document_value_at_path(&record, &DocumentPath::new("$.author.name"));
        assert!(matches!(value, Some(ProximaValue::String(value)) if value == "Ada"));
    }

    #[test]
    fn collection_membership_uses_record_label_and_canonical_property() {
        let record = document_record();
        assert!(record_belongs_to_document_collection(&record, "docs"));
        assert!(!record_belongs_to_document_collection(&record, "other"));
    }

    #[test]
    fn projection_descriptor_extracts_available_source_values() {
        let record = document_record();
        let descriptor = DocumentProjectionDescriptor::new(
            "docs_title_revision",
            "docs",
            DocumentProjectionKind::ColumnarVariation,
            vec![
                DocumentPath::new("title"),
                DocumentPath::new("revision"),
                DocumentPath::new("missing"),
            ],
        );

        let values = projection_source_values(&record, &descriptor);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].0, "title");
        assert_eq!(values[1].0, "revision");
    }
}
