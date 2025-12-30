// Full-text search index using Tantivy
//
// Provides:
// - Tokenized text indexing
// - BM25 ranking
// - Phrase queries
// - Fuzzy matching

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::{Context, Result};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, Value, STORED, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::proto::proximadb_v1::{sql_value::Value as SqlValueVariant, SqlObject, SqlValue};
use super::super::query::path_parser::JsonPath;

/// Full-text search index for a collection
pub struct FullTextIndex {
    /// Collection name
    collection: String,
    /// Tantivy index
    index: Index,
    /// Index writer (mutex-protected for thread safety)
    writer: RwLock<IndexWriter>,
    /// Index reader for searching
    reader: IndexReader,
    /// Schema
    schema: Schema,
    /// Document ID field
    id_field: Field,
    /// Indexed text fields by path
    text_fields: RwLock<HashMap<String, Field>>,
}

impl FullTextIndex {
    /// Create a new full-text index
    pub fn new(collection: &str) -> Result<Self> {
        // Build schema with document ID
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("_id", STORED);

        let schema = schema_builder.build();

        // Create in-memory index (TODO: support disk-based index)
        let index = Index::create_in_ram(schema.clone());

        // Create writer with 50MB heap
        let writer = index
            .writer(50_000_000)
            .context("Failed to create index writer")?;

        // Create reader
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
        })
    }

    /// Add a field to be indexed
    pub fn add_field(&mut self, path: &str) -> Result<()> {
        // Rebuild schema with new field
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("_id", STORED);

        // Copy existing fields
        let existing_fields = self.text_fields.read().unwrap();
        for (existing_path, _) in existing_fields.iter() {
            schema_builder.add_text_field(existing_path, TEXT);
        }

        // Add new field
        let field = schema_builder.add_text_field(path, TEXT);
        drop(existing_fields);

        // Update text fields
        let mut text_fields = self.text_fields.write().unwrap();
        text_fields.insert(path.to_string(), field);

        Ok(())
    }

    /// Index a document
    pub fn index_document(&self, doc_id: &str, document: &SqlObject) -> Result<()> {
        let mut tantivy_doc = TantivyDocument::default();

        // Add document ID
        tantivy_doc.add_text(self.id_field, doc_id);

        // Extract text from indexed paths
        let text_fields = self.text_fields.read().unwrap();
        for (path, field) in text_fields.iter() {
            if let Some(text) = self.extract_text(document, path) {
                tantivy_doc.add_text(*field, &text);
            }
        }

        // Add to index
        let mut writer = self.writer.write().unwrap();
        writer.add_document(tantivy_doc)?;

        Ok(())
    }

    /// Remove a document from the index
    pub fn remove_document(&self, doc_id: &str) -> Result<()> {
        let term = tantivy::Term::from_field_text(self.id_field, doc_id);

        let mut writer = self.writer.write().unwrap();
        writer.delete_term(term);

        Ok(())
    }

    /// Commit pending changes
    pub fn commit(&self) -> Result<()> {
        let mut writer = self.writer.write().unwrap();
        writer.commit()?;
        Ok(())
    }

    /// Search for documents matching query
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>> {
        // Get all text fields for query parser
        let text_fields: Vec<Field> = self
            .text_fields
            .read()
            .unwrap()
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
            if let Some(id_value) = doc.get_first(self.id_field) {
                if let Some(id) = id_value.as_str() {
                    results.push((id.to_string(), score));
                }
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
                let parts: Vec<String> = arr.values
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
                let parts: Vec<String> = obj.fields
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

    #[test]
    fn test_fulltext_index_new() {
        let index = FullTextIndex::new("test_collection");
        assert!(index.is_ok());
    }
}
