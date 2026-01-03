// Inverted index for array containment queries
//
// Maps values found in arrays to the documents containing them.
// Useful for queries like "find all documents where tags contains 'important'"

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use anyhow::Result;

use super::IndexValue;

/// Inverted index for array fields
pub struct ArrayIndex {
    /// JSON path expression (e.g., "$.tags")
    path: String,
    /// Whether this is a sparse index
    sparse: bool,
    /// Inverted index: value -> document IDs
    index: RwLock<HashMap<ArrayValueKey, HashSet<String>>>,
    /// Forward lookup: document ID -> values
    forward: RwLock<HashMap<String, Vec<ArrayValueKey>>>,
}

/// Hashable wrapper for IndexValue
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ArrayValueKey(ArrayValueHash);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ArrayValueHash {
    Null,
    Bool(bool),
    Int(i64),
    Float(u64), // Use bit representation for hashing
    String(String),
    Bytes(Vec<u8>),
}

impl From<IndexValue> for ArrayValueKey {
    fn from(v: IndexValue) -> Self {
        ArrayValueKey(match v {
            IndexValue::Null => ArrayValueHash::Null,
            IndexValue::Bool(b) => ArrayValueHash::Bool(b),
            IndexValue::Int(i) => ArrayValueHash::Int(i),
            IndexValue::Float(f) => ArrayValueHash::Float(f.to_bits()),
            IndexValue::String(s) => ArrayValueHash::String(s),
            IndexValue::Bytes(b) => ArrayValueHash::Bytes(b),
        })
    }
}

impl ArrayIndex {
    /// Create a new array index
    pub fn new(path: &str, sparse: bool) -> Self {
        Self {
            path: path.to_string(),
            sparse,
            index: RwLock::new(HashMap::new()),
            forward: RwLock::new(HashMap::new()),
        }
    }

    /// Insert document with array values
    pub fn insert(&self, doc_id: &str, values: Vec<IndexValue>) -> Result<()> {
        // Skip empty arrays for sparse indexes
        if self.sparse && values.is_empty() {
            return Ok(());
        }

        let keys: Vec<ArrayValueKey> = values
            .into_iter()
            .filter(|v| !self.sparse || !matches!(v, IndexValue::Null))
            .map(ArrayValueKey::from)
            .collect();

        // Update inverted index
        {
            let mut index = self.index.write().unwrap();
            for key in &keys {
                index
                    .entry(key.clone())
                    .or_insert_with(HashSet::new)
                    .insert(doc_id.to_string());
            }
        }

        // Update forward lookup
        {
            let mut forward = self.forward.write().unwrap();
            forward.insert(doc_id.to_string(), keys);
        }

        Ok(())
    }

    /// Remove document from the index
    pub fn remove(&self, doc_id: &str) -> Result<()> {
        // Get values from forward lookup
        let keys = {
            let mut forward = self.forward.write().unwrap();
            forward.remove(doc_id)
        };

        // Remove from inverted index
        if let Some(keys) = keys {
            let mut index = self.index.write().unwrap();
            for key in keys {
                if let Some(ids) = index.get_mut(&key) {
                    ids.remove(doc_id);
                    if ids.is_empty() {
                        index.remove(&key);
                    }
                }
            }
        }

        Ok(())
    }

    /// Query for documents containing a value
    pub fn query_contains(&self, value: &IndexValue) -> Result<Vec<String>> {
        let key = ArrayValueKey::from(value.clone());
        let index = self.index.read().unwrap();

        if let Some(ids) = index.get(&key) {
            Ok(ids.iter().cloned().collect())
        } else {
            Ok(vec![])
        }
    }

    /// Query for documents containing any of the values
    pub fn query_contains_any(&self, values: &[IndexValue]) -> Result<Vec<String>> {
        let index = self.index.read().unwrap();
        let mut results = HashSet::new();

        for value in values {
            let key = ArrayValueKey::from(value.clone());
            if let Some(ids) = index.get(&key) {
                results.extend(ids.iter().cloned());
            }
        }

        Ok(results.into_iter().collect())
    }

    /// Query for documents containing all of the values
    pub fn query_contains_all(&self, values: &[IndexValue]) -> Result<Vec<String>> {
        let index = self.index.read().unwrap();

        let mut result_sets: Vec<&HashSet<String>> = Vec::new();

        for value in values {
            let key = ArrayValueKey::from(value.clone());
            if let Some(ids) = index.get(&key) {
                result_sets.push(ids);
            } else {
                // If any value is not found, intersection is empty
                return Ok(vec![]);
            }
        }

        if result_sets.is_empty() {
            return Ok(vec![]);
        }

        // Intersect all sets
        let mut result = result_sets[0].clone();
        for set in result_sets.iter().skip(1) {
            result = result.intersection(set).cloned().collect();
        }

        Ok(result.into_iter().collect())
    }

    /// Get the number of unique values indexed
    pub fn unique_values_count(&self) -> usize {
        self.index.read().unwrap().len()
    }

    /// Get the number of documents indexed
    pub fn documents_count(&self) -> usize {
        self.forward.read().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_array_index_contains() {
        let index = ArrayIndex::new("$.tags", false);

        index
            .insert(
                "doc1",
                vec![
                    IndexValue::String("rust".to_string()),
                    IndexValue::String("database".to_string()),
                ],
            )
            .unwrap();

        index
            .insert(
                "doc2",
                vec![
                    IndexValue::String("rust".to_string()),
                    IndexValue::String("web".to_string()),
                ],
            )
            .unwrap();

        let results = index
            .query_contains(&IndexValue::String("rust".to_string()))
            .unwrap();
        assert_eq!(results.len(), 2);

        let results = index
            .query_contains(&IndexValue::String("database".to_string()))
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results.contains(&"doc1".to_string()));
    }

    #[test]
    fn test_array_index_contains_all() {
        let index = ArrayIndex::new("$.tags", false);

        index
            .insert(
                "doc1",
                vec![
                    IndexValue::String("a".to_string()),
                    IndexValue::String("b".to_string()),
                    IndexValue::String("c".to_string()),
                ],
            )
            .unwrap();

        index
            .insert(
                "doc2",
                vec![
                    IndexValue::String("a".to_string()),
                    IndexValue::String("b".to_string()),
                ],
            )
            .unwrap();

        let results = index
            .query_contains_all(&[
                IndexValue::String("a".to_string()),
                IndexValue::String("b".to_string()),
            ])
            .unwrap();
        assert_eq!(results.len(), 2);

        let results = index
            .query_contains_all(&[
                IndexValue::String("a".to_string()),
                IndexValue::String("c".to_string()),
            ])
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results.contains(&"doc1".to_string()));
    }
}
