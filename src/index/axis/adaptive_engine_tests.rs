//! Unit tests for AXIS Adaptive Engine

use super::adaptive_engine::*;
use super::*;
use crate::core::CollectionId;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_adaptive_engine_creation() {
        let config = AxisConfig::default();
        let result = AdaptiveIndexEngine::new(config).await;
        
        assert!(result.is_ok(), "Adaptive engine should be created successfully");
    }

    #[tokio::test]
    async fn test_collection_analysis() {
        let config = AxisConfig::default();
        let engine = AdaptiveIndexEngine::new(config).await.unwrap();
        
        let collection_id: CollectionId = "test_collection".to_string();
        let result = engine.analyze_collection(&collection_id).await;
        
        assert!(result.is_ok(), "Collection analysis should succeed");
    }
}