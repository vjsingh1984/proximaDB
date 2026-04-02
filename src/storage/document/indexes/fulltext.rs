// Full-text search index using Tantivy
//
// Provides:
// - Tokenized text indexing
// - BM25 ranking
// - Phrase queries
// - Fuzzy matching

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use anyhow::{Context, Result};
use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, STORED, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

use super::super::query::path_parser::JsonPath;
use crate::proto::proximadb_v1::{SqlObject, SqlValue, sql_value::Value as SqlValueVariant};

/// Full-text search index for a collection
pub struct FullTextIndex {
    /// Collection name
    #[allow(dead_code)]
    collection: String,
    /// Tantivy index
    index: Index,
    /// Index writer (mutex-protected for thread safety)
    writer: RwLock<IndexWriter>,
    /// Index reader for searching
    reader: IndexReader,
    /// Schema
    #[allow(dead_code)]
    schema: Schema,
    /// Document ID field
    id_field: Field,
    /// Indexed text fields by path
    text_fields: RwLock<HashMap<String, Field>>,
    /// Disk directory for persistent indexes (None = in-memory)
    persistent_dir: Option<std::path::PathBuf>,
}

impl FullTextIndex {
    /// Create a new in-memory full-text index (for tests and backward compatibility)
    pub fn new(collection: &str) -> Result<Self> {
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("_id", STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        Self::from_index(collection, index, schema, id_field, None)
    }

    /// Create a disk-persisted full-text index at the given directory
    ///
    /// `text_fields` lists the field names to index (e.g., `["title", "body"]`).
    /// If the directory already contains a valid index with matching schema,
    /// it is reopened — preserving previously indexed data across restarts.
    /// If the directory is empty, a new index is created with the given fields.
    pub fn new_persistent(collection: &str, data_dir: &Path) -> Result<Self> {
        let index_dir = data_dir.join("fulltext").join(collection);
        std::fs::create_dir_all(&index_dir)
            .context("Failed to create full-text index directory")?;

        let mmap_dir = MmapDirectory::open(&index_dir)
            .context("Failed to open mmap directory for full-text index")?;

        // Try to open an existing index first
        if let Ok(index) = Index::open(mmap_dir.clone()) {
            let schema = index.schema();
            let id_field = schema
                .get_field("_id")
                .context("Existing index missing _id field")?;
            return Self::from_index(collection, index, schema, id_field, Some(index_dir));
        }

        // No existing index — create a fresh one with just _id
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("_id", STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_dir(&index_dir, schema.clone())
            .context("Failed to create persistent full-text index")?;

        Self::from_index(collection, index, schema, id_field, Some(index_dir))
    }

    /// Shared constructor from an already-created Tantivy Index
    fn from_index(
        collection: &str,
        index: Index,
        schema: Schema,
        id_field: Field,
        persistent_dir: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        let writer = index
            .writer(50_000_000)
            .context("Failed to create index writer")?;

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context("Failed to create index reader")?;

        Ok(Self {
            collection: collection.to_string(),
            index,
            writer: RwLock::new(writer),
            reader,
            schema,
            id_field,
            text_fields: RwLock::new(HashMap::new()),
            persistent_dir,
        })
    }

    /// Add a field to be indexed
    ///
    /// If the field already exists in the index schema (e.g., from a prior
    /// persistent session), it is reused.  Otherwise, the field is dynamically
    /// registered.  Note: Tantivy does not support adding fields to an existing
    /// schema, so we look up the field by name — if it was pre-registered at
    /// index creation time it is available; otherwise we store a mapping for
    /// text extraction and the `_id` field is the only field written.
    pub fn add_field(&mut self, path: &str) -> Result<()> {
        let mut text_fields = self
            .text_fields
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if text_fields.contains_key(path) {
            return Ok(()); // Already registered
        }

        // Try to find the field in the existing index schema (works for re-opened indexes)
        if let Ok(field) = self.index.schema().get_field(path) {
            text_fields.insert(path.to_string(), field);
            return Ok(());
        }

        // Field doesn't exist yet — we need to register it.
        // Tantivy doesn't allow dynamic field addition after index creation.
        // Workaround: store all text content under the _id field's document,
        // and use the body text for search by concatenating into a "body" field.
        // Better approach: recreate the index with the additional field.
        drop(text_fields);

        // Rebuild the entire index schema with all existing + new fields
        let mut schema_builder = Schema::builder();
        let new_id_field = schema_builder.add_text_field("_id", STORED);

        let existing = self
            .text_fields
            .read()
            .unwrap_or_else(|p| p.into_inner());
        for (existing_path, _) in existing.iter() {
            schema_builder.add_text_field(existing_path, TEXT);
        }
        let new_text_field = schema_builder.add_text_field(path, TEXT);
        let new_schema = schema_builder.build();
        drop(existing);

        // Create a new index with the extended schema
        let new_index = if let Some(ref dir) = self.persistent_dir {
            // Persistent: remove old index files and create with new schema.
            // Callers must reindex documents after adding a field.
            if dir.exists() {
                for entry in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            Index::create_in_dir(dir, new_schema.clone())
                .context("Failed to recreate persistent index with new field")?
        } else {
            Index::create_in_ram(new_schema.clone())
        };
        let new_writer = new_index
            .writer(50_000_000)
            .context("Failed to create index writer for extended schema")?;
        let new_reader = new_index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context("Failed to create index reader for extended schema")?;

        // Rebuild text_fields map with fields from the new schema
        let mut new_text_fields = HashMap::new();
        let old_fields = self
            .text_fields
            .read()
            .unwrap_or_else(|p| p.into_inner());
        for (p, _) in old_fields.iter() {
            if let Ok(f) = new_schema.get_field(p) {
                new_text_fields.insert(p.clone(), f);
            }
        }
        drop(old_fields);
        new_text_fields.insert(path.to_string(), new_text_field);

        // Swap all internals
        self.index = new_index;
        *self
            .writer
            .write()
            .unwrap_or_else(|p| p.into_inner()) = new_writer;
        self.reader = new_reader;
        self.schema = new_schema;
        self.id_field = new_id_field;
        *self
            .text_fields
            .write()
            .unwrap_or_else(|p| p.into_inner()) = new_text_fields;

        Ok(())
    }

    /// Index a document
    pub fn index_document(&self, doc_id: &str, document: &SqlObject) -> Result<()> {
        let mut tantivy_doc = TantivyDocument::default();

        // Add document ID
        tantivy_doc.add_text(self.id_field, doc_id);

        // Extract text from indexed paths
        let text_fields = self
            .text_fields
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (path, field) in text_fields.iter() {
            if let Some(text) = self.extract_text(document, path) {
                tantivy_doc.add_text(*field, &text);
            }
        }

        // Add to index
        let writer = self
            .writer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writer.add_document(tantivy_doc)?;

        Ok(())
    }

    /// Remove a document from the index
    pub fn remove_document(&self, doc_id: &str) -> Result<()> {
        let term = tantivy::Term::from_field_text(self.id_field, doc_id);

        let writer = self
            .writer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writer.delete_term(term);

        Ok(())
    }

    /// Commit pending changes
    pub fn commit(&self) -> Result<()> {
        let mut writer = self
            .writer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writer.commit()?;
        Ok(())
    }

    /// Search for documents matching query
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>> {
        // Get all text fields for query parser
        let text_fields: Vec<Field> = self
            .text_fields
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();

        if text_fields.is_empty() {
            return Ok(vec![]);
        }

        // Parse query
        let query_parser = QueryParser::for_index(&self.index, text_fields);
        let parsed_query = query_parser
            .parse_query(query)
            .context("Failed to parse search query")?;

        // Execute search
        let searcher = self.reader.searcher();
        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit))
            .context("Search failed")?;

        // Extract results
        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            if let Some(id_value) = doc.get_first(self.id_field)
                && let Some(id) = id_value.as_str() {
                    results.push((id.to_string(), score));
                }
        }

        Ok(results)
    }

    /// Extract text value from a JSON path in a document
    fn extract_text(&self, document: &SqlObject, path: &str) -> Option<String> {
        // Parse the path (add $ prefix if not present for consistency)
        let path_str = if path.starts_with('$') {
            path.to_string()
        } else {
            format!("$.{}", path)
        };

        let json_path = match JsonPath::parse(&path_str) {
            Ok(p) => p,
            Err(_) => return None,
        };

        // Evaluate path against document
        let values = json_path.evaluate(document);

        // Collect all string values (concatenate for array results)
        let text_parts: Vec<String> = values
            .into_iter()
            .filter_map(|v| self.sql_value_to_string(&v))
            .collect();

        if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(" "))
        }
    }

    /// Convert a SqlValue to a string for indexing
    fn sql_value_to_string(&self, value: &SqlValue) -> Option<String> {
        match &value.value {
            Some(SqlValueVariant::StringValue(s)) => Some(s.clone()),
            Some(SqlValueVariant::Int64Value(i)) => Some(i.to_string()),
            Some(SqlValueVariant::NumberValue(f)) => Some(f.to_string()),
            Some(SqlValueVariant::BoolValue(b)) => Some(b.to_string()),
            Some(SqlValueVariant::ArrayValue(arr)) => {
                // Concatenate array elements
                let parts: Vec<String> = arr
                    .values
                    .iter()
                    .filter_map(|v| self.sql_value_to_string(v))
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(" "))
                }
            }
            Some(SqlValueVariant::ObjectValue(obj)) => {
                // Concatenate all object field values (for nested text extraction)
                let parts: Vec<String> = obj
                    .fields
                    .values()
                    .filter_map(|v| self.sql_value_to_string(v))
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(" "))
                }
            }
            Some(SqlValueVariant::NullValue(_)) | None => None,
            // Bytes don't convert to text meaningfully
            Some(SqlValueVariant::BytesValue(_)) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{SqlObject, SqlValue, sql_value::Value as V};
    use std::collections::HashMap;

    fn make_doc(fields: Vec<(&str, &str)>) -> SqlObject {
        let mut map = HashMap::new();
        for (k, v) in fields {
            map.insert(
                k.to_string(),
                SqlValue {
                    value: Some(V::StringValue(v.to_string())),
                },
            );
        }
        SqlObject { fields: map }
    }

    #[test]
    fn test_fulltext_index_new() {
        let index = FullTextIndex::new("test_collection");
        assert!(index.is_ok());
    }

    #[test]
    fn test_persistent_index_survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();

        // Phase 1: create index with title field, insert document, commit
        {
            let mut idx = FullTextIndex::new_persistent("coll1", data_dir).unwrap();
            idx.add_field("title").unwrap();
            let doc = make_doc(vec![("title", "hello world rust programming")]);
            idx.index_document("doc1", &doc).unwrap();
            idx.commit().unwrap();
        }

        // Phase 2: reopen the same directory — data must still be searchable
        {
            let mut idx = FullTextIndex::new_persistent("coll1", data_dir).unwrap();
            // "title" field already exists in the reopened schema
            idx.add_field("title").unwrap();
            // Reload reader to pick up committed segments from disk
            idx.reader.reload().unwrap();
            let results = idx.search("rust", 10).unwrap();
            assert_eq!(results.len(), 1, "Expected doc1 to survive reopen");
            assert_eq!(results[0].0, "doc1");
        }
    }

    #[test]
    fn test_in_memory_index_basic_search() {
        let mut idx = FullTextIndex::new("test").unwrap();
        idx.add_field("body").unwrap();
        let doc = make_doc(vec![("body", "the quick brown fox")]);
        idx.index_document("d1", &doc).unwrap();
        idx.commit().unwrap();
        // Force reader to see committed segments (OnCommitWithDelay may not be instant)
        idx.reader.reload().unwrap();

        let results = idx.search("quick", 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d1");
    }
}
