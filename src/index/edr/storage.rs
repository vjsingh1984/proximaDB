//! Document Storage for EDR
//!
//! This module provides storage for multi-vector documents needed for EDR.
//! Each document can have multiple vector representations for better retrieval.

use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;

/// Multi-vector document for EDR
#[derive(Debug, Clone)]
pub struct MultiVectorDocument {
    /// Document ID
    pub id: String,
    /// Multiple vector representations
    pub vectors: Vec<Vec<f32>>,
    /// Document metadata
    pub metadata: serde_json::Value,
}

/// Document store for EDR with multi-vector support
pub struct EdrDocumentStore {
    /// Number of vector representations per document
    num_vectors_per_doc: usize,
    /// Document storage: doc_id -> MultiVectorDocument
    documents: Arc<DashMap<String, MultiVectorDocument>>,
}

impl EdrDocumentStore {
    /// Create a new EDR document store
    pub fn new(num_vectors_per_doc: usize) -> Self {
        Self {
            num_vectors_per_doc,
            documents: Arc::new(DashMap::new()),
        }
    }

    /// Insert a document with multiple vectors
    pub async fn insert(&self, id: String, vectors: Vec<Vec<f32>>) -> Result<()> {
        if vectors.len() != self.num_vectors_per_doc {
            return Err(anyhow::anyhow!(
                "Expected {} vectors, got {}",
                self.num_vectors_per_doc,
                vectors.len()
            ));
        }

        let doc = MultiVectorDocument {
            id: id.clone(),
            vectors,
            metadata: serde_json::json!({}),
        };

        self.documents.insert(id, doc);

        Ok(())
    }

    /// Get a document by ID
    pub async fn get(&self, id: &str) -> Option<MultiVectorDocument> {
        self.documents.get(id).map(|ref_val| ref_val.clone())
    }

    /// Get all documents
    pub async fn get_all_documents(&self) -> Result<Vec<(String, Vec<Vec<f32>>)>> {
        let mut result = Vec::new();

        for ref_val in self.documents.iter() {
            let (id, doc) = ref_val.pair();
            result.push((id.clone(), doc.vectors.clone()));
        }

        Ok(result)
    }

    /// Remove a document
    pub async fn remove(&self, id: &str) -> Result<()> {
        self.documents.remove(id);
        Ok(())
    }

    /// Count total documents
    pub async fn count(&self) -> usize {
        self.documents.len()
    }

    /// Estimate memory usage
    pub async fn estimate_memory_usage(&self) -> usize {
        let mut total = 0;

        for ref_val in self.documents.iter() {
            let doc = ref_val.value();
            // Estimate: id + vectors + metadata
            total += doc.id.len() * 4; // String ID
            if !doc.vectors.is_empty() {
                total += doc.vectors.len() * doc.vectors[0].len() * 4; // Vector data
            }
            total += 100; // Metadata estimate
        }

        total
    }

    /// Clear all documents
    pub async fn clear(&self) -> Result<()> {
        self.documents.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_document_store_creation() {
        let store = EdrDocumentStore::new(3);
        assert_eq!(store.count().await, 0);
    }

    #[tokio::test]
    async fn test_insert_document() {
        let store = EdrDocumentStore::new(2);

        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        store.insert("doc1".to_string(), vectors).await.unwrap();

        assert_eq!(store.count().await, 1);

        let retrieved = store.get("doc1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().vectors.len(), 2);
    }

    #[tokio::test]
    async fn test_vector_count_validation() {
        let store = EdrDocumentStore::new(2);

        let wrong_vectors = vec![vec![1.0]]; // Only 1 vector, but config expects 2
        let result = store.insert("doc1".to_string(), wrong_vectors).await;

        assert!(
            result.is_err(),
            "Should reject documents with wrong vector count"
        );
    }

    #[tokio::test]
    async fn test_remove_document() {
        let store = EdrDocumentStore::new(2);

        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        store.insert("doc1".to_string(), vectors).await.unwrap();

        store.remove("doc1").await.unwrap();
        assert_eq!(store.count().await, 0);
    }

    #[tokio::test]
    async fn test_memory_estimation() {
        let store = EdrDocumentStore::new(2);

        let vectors = vec![vec![1.0; 128], vec![0.5; 128]];
        store.insert("doc1".to_string(), vectors).await.unwrap();

        let memory = store.estimate_memory_usage().await;
        assert!(memory > 1000); // Should be substantial
    }
}
