#![allow(dead_code)]
// Index management for document storage
//
// Provides:
// - B+ tree indexes for JSON paths (range queries)
// - Inverted indexes for arrays (containment queries)
// - Full-text indexes via Tantivy (text search)

pub mod array_index;
pub mod fulltext;
pub mod geo_index;
pub mod path_index;

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::proto::proximadb_v1::{DocIndexType, IndexDefinition};
use proximadb_records::ProximaRecord;

use self::array_index::ArrayIndex;
use self::fulltext::FullTextIndex;
use self::path_index::PathIndex;
use super::DocumentRecord;
use super::canonical_adapter::proxima_record_to_legacy_document;

/// Index type wrapper for different index implementations
#[allow(clippy::large_enum_variant)]
enum IndexType {
    Path(PathIndex),
    Array(ArrayIndex),
    FullText(FullTextIndex),
    // Hash and Geo are future implementations
}

use self::geo_index::GeoIndex;

/// Collection index set - all indexes for a single collection
#[derive(Default)]
struct CollectionIndexes {
    /// Path-based B+ tree indexes
    path_indexes: HashMap<String, PathIndex>,
    /// Array inverted indexes
    array_indexes: HashMap<String, ArrayIndex>,
    /// Full-text search index (single per collection)
    fulltext_index: Option<FullTextIndex>,
    /// Geospatial indexes by path
    geo_indexes: HashMap<String, GeoIndex>,
}

/// Index manager for document collections
pub struct IndexManager {
    /// Indexes by collection name
    indexes: RwLock<HashMap<String, CollectionIndexes>>,
}

impl IndexManager {
    /// Create a new index manager
    pub fn new() -> Self {
        Self {
            indexes: RwLock::new(HashMap::new()),
        }
    }

    /// Create an index for a collection
    pub async fn create_index(&self, collection: &str, definition: &IndexDefinition) -> Result<()> {
        info!(
            "Creating index {} on {} for path {}",
            definition.name.as_deref().unwrap_or("unnamed"),
            collection,
            definition.path
        );

        let mut indexes = self.indexes.write().await;
        let collection_indexes = indexes
            .entry(collection.to_string())
            .or_insert_with(CollectionIndexes::default);

        let index_type =
            DocIndexType::try_from(definition.index_type).unwrap_or(DocIndexType::Unspecified);

        match index_type {
            DocIndexType::Btree | DocIndexType::Hash => {
                // Both btree and hash use PathIndex for now
                let index = PathIndex::new(&definition.path, definition.unique, definition.sparse);
                collection_indexes
                    .path_indexes
                    .insert(definition.path.clone(), index);
            }
            DocIndexType::Inverted => {
                let index = ArrayIndex::new(&definition.path, definition.sparse);
                collection_indexes
                    .array_indexes
                    .insert(definition.path.clone(), index);
            }
            DocIndexType::Fulltext => {
                if collection_indexes.fulltext_index.is_none() {
                    collection_indexes.fulltext_index = Some(FullTextIndex::new(collection)?);
                }
                // Add field to fulltext index
                if let Some(ref mut ft_index) = collection_indexes.fulltext_index {
                    ft_index.add_field(&definition.path)?;
                }
            }
            DocIndexType::Geo => {
                let index = GeoIndex::new(&definition.path);
                collection_indexes
                    .geo_indexes
                    .insert(definition.path.clone(), index);
            }
            DocIndexType::Unspecified => {
                return Err(anyhow!("Unspecified index type"));
            }
        }

        Ok(())
    }

    /// Drop all indexes for a collection
    pub async fn drop_collection_indexes(&self, collection: &str) -> Result<()> {
        info!("Dropping all indexes for collection: {}", collection);

        let mut indexes = self.indexes.write().await;
        indexes.remove(collection);

        Ok(())
    }

    /// Index a new document
    pub async fn index_document(&self, collection: &str, document: &DocumentRecord) -> Result<()> {
        self.index_document_with_key(collection, document, &document.id)
            .await
    }

    /// Index a document using an explicit projection key.
    ///
    /// Legacy document mode uses the facade document id. Canonical record-backed
    /// mode uses `ProximaRecord::oid` so projection entries can be traced and
    /// rebuilt by canonical record identity.
    async fn index_document_with_key(
        &self,
        collection: &str,
        document: &DocumentRecord,
        projection_key: &str,
    ) -> Result<()> {
        debug!("Indexing document {} in {}", document.id, collection);

        let indexes = self.indexes.read().await;
        if let Some(collection_indexes) = indexes.get(collection) {
            // Update path indexes
            for (path, index) in &collection_indexes.path_indexes {
                if let Some(value) = self.extract_path_value(&document.document, path) {
                    index.insert(projection_key, value)?;
                }
            }

            // Update array indexes
            for (path, index) in &collection_indexes.array_indexes {
                if let Some(values) = self.extract_array_values(&document.document, path) {
                    index.insert(projection_key, values)?;
                }
            }

            // Update full-text index
            if let Some(ref ft_index) = collection_indexes.fulltext_index {
                ft_index.index_document(projection_key, &document.document)?;
            }
        }

        Ok(())
    }

    /// Apply document projection indexes from a canonical record.
    ///
    /// This is the Phase 2 convergence boundary: JSON path, array, and
    /// full-text structures are rebuildable projections over `ProximaRecord`.
    /// Projection entries are keyed by canonical record oid; query execution
    /// accepts canonical oid candidate sets and facade document-id candidate
    /// sets while the public document API remains v1-compatible.
    pub async fn index_record_projection(&self, record: &ProximaRecord) -> Result<Option<String>> {
        let Some(document) = proxima_record_to_legacy_document(record) else {
            return Ok(None);
        };

        self.index_document_with_key(&document.collection_id, &document, &record.oid)
            .await?;
        Ok(Some(record.oid.clone()))
    }

    /// Reindex a document after update
    pub async fn reindex_document(
        &self,
        collection: &str,
        document: &DocumentRecord,
    ) -> Result<()> {
        debug!("Reindexing document {} in {}", document.id, collection);

        // Remove old entries and add new ones
        self.remove_document(collection, &document.id).await?;
        self.index_document(collection, document).await?;

        Ok(())
    }

    /// Rebuild projection entries for one canonical record after an update.
    pub async fn reindex_record_projection(
        &self,
        record: &ProximaRecord,
    ) -> Result<Option<String>> {
        let Some(document) = proxima_record_to_legacy_document(record) else {
            return Ok(None);
        };

        self.remove_document(&document.collection_id, &record.oid)
            .await?;
        self.index_document_with_key(&document.collection_id, &document, &record.oid)
            .await?;
        Ok(Some(record.oid.clone()))
    }

    /// Remove a document from all indexes
    pub async fn remove_document(&self, collection: &str, id: &str) -> Result<()> {
        debug!("Removing document {} from indexes in {}", id, collection);

        let indexes = self.indexes.read().await;
        if let Some(collection_indexes) = indexes.get(collection) {
            // Remove from path indexes
            for index in collection_indexes.path_indexes.values() {
                index.remove(id)?;
            }

            // Remove from array indexes
            for index in collection_indexes.array_indexes.values() {
                index.remove(id)?;
            }

            // Remove from full-text index
            if let Some(ref ft_index) = collection_indexes.fulltext_index {
                ft_index.remove_document(id)?;
            }
        }

        Ok(())
    }

    /// Remove projection entries by projection key.
    ///
    /// Canonical storage owns durable truth; this only removes rebuildable
    /// projection state for the canonical record-backed document facade.
    pub async fn remove_record_projection(
        &self,
        collection: &str,
        projection_key: &str,
    ) -> Result<()> {
        self.remove_document(collection, projection_key).await
    }

    /// Rebuild projection entries for a collection from canonical records.
    ///
    /// Existing entries for supplied records are replaced. A future full
    /// rebuild path should clear all collection projection state before replay,
    /// but this helper gives canonical recovery/query paths a record-shaped
    /// projection API today.
    pub async fn rebuild_record_projections(
        &self,
        collection: &str,
        records: &[ProximaRecord],
    ) -> Result<usize> {
        let mut projected = 0;

        for record in records {
            if let Some(document) = proxima_record_to_legacy_document(record)
                && document.collection_id == collection
            {
                self.reindex_document(collection, &document).await?;
                projected += 1;
            }
        }

        Ok(projected)
    }

    /// Query a path index for documents matching a condition
    pub async fn query_path_index(
        &self,
        collection: &str,
        path: &str,
        condition: &PathQueryCondition,
    ) -> Result<Vec<String>> {
        let indexes = self.indexes.read().await;
        if let Some(collection_indexes) = indexes.get(collection)
            && let Some(index) = collection_indexes.path_indexes.get(path)
        {
            return index.query(condition);
        }
        Ok(vec![])
    }

    /// Query an array index for documents containing a value
    pub async fn query_array_index(
        &self,
        collection: &str,
        path: &str,
        value: &IndexValue,
    ) -> Result<Vec<String>> {
        let indexes = self.indexes.read().await;
        if let Some(collection_indexes) = indexes.get(collection)
            && let Some(index) = collection_indexes.array_indexes.get(path)
        {
            return index.query_contains(value);
        }
        Ok(vec![])
    }

    /// Full-text search across indexed fields
    pub async fn fulltext_search(
        &self,
        collection: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        let indexes = self.indexes.read().await;
        if let Some(collection_indexes) = indexes.get(collection)
            && let Some(ref ft_index) = collection_indexes.fulltext_index
        {
            return ft_index.search(query, limit);
        }
        Ok(vec![])
    }

    // Helper methods for value extraction

    fn extract_path_value(
        &self,
        document: &crate::proto::proximadb_v1::SqlObject,
        path: &str,
    ) -> Option<IndexValue> {
        use super::query::path_parser::JsonPath;
        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

        // Parse and evaluate the path
        let json_path = JsonPath::parse(&format!("$.{}", path.trim_start_matches("$."))).ok()?;
        let values = json_path.evaluate(document);

        // Get the first value and convert to IndexValue
        values.into_iter().next().and_then(|v| {
            match v.value {
                Some(SqlVal::NullValue(_)) => Some(IndexValue::Null),
                Some(SqlVal::BoolValue(b)) => Some(IndexValue::Bool(b)),
                Some(SqlVal::Int64Value(i)) => Some(IndexValue::Int(i)),
                Some(SqlVal::NumberValue(f)) => Some(IndexValue::Float(f)),
                Some(SqlVal::StringValue(s)) => Some(IndexValue::String(s)),
                Some(SqlVal::BytesValue(b)) => Some(IndexValue::Bytes(b)),
                _ => None, // Objects and arrays aren't directly indexable
            }
        })
    }

    fn extract_array_values(
        &self,
        document: &crate::proto::proximadb_v1::SqlObject,
        path: &str,
    ) -> Option<Vec<IndexValue>> {
        use super::query::path_parser::JsonPath;
        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

        // Parse and evaluate the path
        let json_path = JsonPath::parse(&format!("$.{}", path.trim_start_matches("$."))).ok()?;
        let values = json_path.evaluate(document);

        // Get the first value (should be an array)
        let first = values.into_iter().next()?;

        if let Some(SqlVal::ArrayValue(arr)) = first.value {
            let index_values: Vec<IndexValue> = arr
                .values
                .into_iter()
                .filter_map(|v| match v.value {
                    Some(SqlVal::NullValue(_)) => Some(IndexValue::Null),
                    Some(SqlVal::BoolValue(b)) => Some(IndexValue::Bool(b)),
                    Some(SqlVal::Int64Value(i)) => Some(IndexValue::Int(i)),
                    Some(SqlVal::NumberValue(f)) => Some(IndexValue::Float(f)),
                    Some(SqlVal::StringValue(s)) => Some(IndexValue::String(s)),
                    Some(SqlVal::BytesValue(b)) => Some(IndexValue::Bytes(b)),
                    _ => None,
                })
                .collect();

            if !index_values.is_empty() {
                return Some(index_values);
            }
        }

        None
    }
}

impl Default for IndexManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Value type for indexes
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum IndexValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
}

/// Query condition for path indexes
#[derive(Debug, Clone)]
pub enum PathQueryCondition {
    Eq(IndexValue),
    Ne(IndexValue),
    Gt(IndexValue),
    Gte(IndexValue),
    Lt(IndexValue),
    Lte(IndexValue),
    In(Vec<IndexValue>),
    NotIn(Vec<IndexValue>),
    Exists(bool),
    Regex(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_index_manager_new() {
        let manager = IndexManager::new();
        assert!(manager.indexes.read().await.is_empty());
    }
}
