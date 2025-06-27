//! Unit tests for AXIS Collection Analyzer

use super::analyzer::*;
use super::*;
use crate::core::{CollectionId, avro_unified::VectorRecord};
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

/// Helper function to create test vector record
fn create_test_vector(collection_id: &str) -> VectorRecord {
    let now = Utc::now().timestamp_millis();
    
    VectorRecord {
        id: Uuid::new_v4().to_string(),
        collection_id: collection_id.to_string(),
        vector: vec![1.0, 2.0, 3.0, 4.0],
        metadata: HashMap::new(),
        timestamp: now,
        created_at: now,
        updated_at: now,
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collection_analyzer_creation() {
        let result = CollectionAnalyzer::new().await;
        assert!(result.is_ok(), "Collection analyzer should be created successfully");
    }

    #[tokio::test]
    async fn test_analyze_collection_characteristics() {
        let analyzer = CollectionAnalyzer::new().await.unwrap();
        let collection_id: CollectionId = "test_collection".to_string();
        
        let result = analyzer.analyze_collection(&collection_id).await;
        assert!(result.is_ok(), "Collection characteristics analysis should succeed");
    }

    #[tokio::test]
    async fn test_analyze_vector_sample() {
        let analyzer = CollectionAnalyzer::new().await.unwrap();
        let collection_id: CollectionId = "test_collection".to_string();
        
        let vectors = vec![
            create_test_vector("test_collection"),
            create_test_vector("test_collection"),
            create_test_vector("test_collection"),
        ];
        
        let result = analyzer.analyze_vector_sample(&collection_id, &vectors).await;
        assert!(result.is_ok(), "Vector sample analysis should succeed");
    }
}