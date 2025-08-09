/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#[cfg(test)]
mod tests {
    use crate::index::axis::annoy_index::{AxisAnnoyConfig, AxisAnnoyIndex};
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::VectorRecord;
    use crate::proto::proximadb::MetadataItem;
    use crate::index::axis::index_factory::AxisVectorIndex;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn get_timestamp() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as i64
    }
    
    fn create_test_record(id: String, vector: Vec<f32>, metadata: Vec<MetadataItem>) -> Arc<VectorRecord> {
        Arc::new(VectorRecord {
            id: Some(id),
            vector,
            metadata,
            timestamp: get_timestamp() as u32,
            updated_at: Some(get_timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        
        })
    }

    fn create_test_vectors(n: usize, dim: usize) -> Vec<(String, Vec<f32>)> {
        (0..n)
            .map(|i| {
                let mut vec = vec![0.0; dim];
                vec[i % dim] = 1.0;
                (format!("vec_{}", i), vec)
            })
            .collect()
    }

    #[tokio::test]
    async fn test_annoy_basic_functionality() {
        let config = AxisAnnoyConfig {
            n_trees: 3,
            search_k: -1,
            max_leaf_size: 5,
            seed: 42,
            distance_metric: DistanceMetric::Euclidean,
        };

        let index = AxisAnnoyIndex::new(config, 8).unwrap();

        // Add vectors
        let vectors = create_test_vectors(20, 8);
        for (id, vec) in &vectors {
            let record = create_test_record(id.clone(), vec.clone(), vec![]);
            index.add(id.clone(), record).await.unwrap();
        }

        // Build index
        index.build().await.unwrap();

        // Search
        let query = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 5, None).await.unwrap();

        assert_eq!(results.len(), 5);
        // Annoy is approximate, so we just check that results are reasonable
        assert!(results[0].1 < 1.0, "Top result should have low distance");
        // Check that vec_0 is in top results (it's an exact match)
        let has_exact_match = results.iter().any(|(id, dist)| id == "vec_0" && *dist < 0.1);
        assert!(has_exact_match, "Should find the exact match in top results");
    }

    #[tokio::test]
    async fn test_annoy_cosine_distance() {
        let config = AxisAnnoyConfig {
            n_trees: 5,
            search_k: -1,
            max_leaf_size: 10,
            seed: 42,
            distance_metric: DistanceMetric::Cosine,
        };

        let mut index = AxisAnnoyIndex::new(config, 4).unwrap();

        // Add normalized vectors
        let vectors = vec![
            ("v1", vec![1.0, 0.0, 0.0, 0.0]),
            ("v2", vec![0.707, 0.707, 0.0, 0.0]),
            ("v3", vec![0.577, 0.577, 0.577, 0.0]),
            ("v4", vec![0.5, 0.5, 0.5, 0.5]),
        ];

        for (id, vec) in &vectors {
            let record = create_test_record(id.to_string(), vec.clone(), vec![]);
            index.add(id.to_string(), record).await.unwrap();
        }

        // Build index
        index.build().await.unwrap();

        // Search with query close to v2
        let query = vec![0.8, 0.6, 0.0, 0.0];
        let results = index.search(&query, 2, None).await.unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "v2"); // Closest by cosine similarity
    }

    #[tokio::test]
    async fn test_annoy_with_filter() {
        let config = AxisAnnoyConfig {
            n_trees: 3,
            search_k: -1,
            max_leaf_size: 5,
            seed: 42,
            distance_metric: DistanceMetric::Euclidean,
        };

        let mut index = AxisAnnoyIndex::new(config, 4).unwrap();

        // Add vectors with metadata
        for i in 0..10 {
            let mut vec = vec![0.0; 4];
            vec[i % 4] = 1.0;
            
            let metadata = vec![MetadataItem {
                key: "category".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue((i % 2).to_string())),
            }];
            
            let record = create_test_record(format!("vec_{}", i), vec, metadata);
            index.add(format!("vec_{}", i), record).await.unwrap();
        }

        // Build index
        index.build().await.unwrap();

        // Search with filter
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let filter = |record: &VectorRecord| -> bool {
            record.metadata.iter().any(|item| item.key == "category" && matches!(&item.value, Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) if s == "1"))
        };

        let results = index.search(&query, 5, Some(&filter)).await.unwrap();

        // Should only return vectors with category=1
        for (id, _) in &results {
            let idx: usize = id.strip_prefix("vec_").unwrap().parse().unwrap();
            assert_eq!(idx % 2, 1);
        }
    }

    #[tokio::test]
    async fn test_annoy_static_index_behavior() {
        let config = AxisAnnoyConfig::default();
        let mut index = AxisAnnoyIndex::new(config, 4).unwrap();

        let record1 = create_test_record("v1".to_string(), vec![1.0, 0.0, 0.0, 0.0], vec![]);
        let record2 = create_test_record("v2".to_string(), vec![0.0, 1.0, 0.0, 0.0], vec![]);

        // Add before build - should work
        index.add("v1".to_string(), record1).await.unwrap();

        // Build index
        index.build().await.unwrap();

        // Try to add after build - should fail
        let result = index.add("v2".to_string(), record2).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be modified"));

        // Try to remove - should fail
        let result = index.remove("v1").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not support removal"));

        // Search should still work
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 1, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "v1");
    }

    #[tokio::test]
    async fn test_annoy_search_k_parameter() {
        let mut config = AxisAnnoyConfig {
            n_trees: 10,
            search_k: 100, // Fixed search_k
            max_leaf_size: 10,
            seed: 42,
            distance_metric: DistanceMetric::Euclidean,
        };

        let mut index1 = AxisAnnoyIndex::new(config.clone(), 8).unwrap();

        // Add vectors
        let vectors = create_test_vectors(50, 8);
        for (id, vec) in &vectors {
            let record = create_test_record(id.clone(), vec.clone(), vec![]);
            index1.add(id.clone(), record).await.unwrap();
        }

        index1.build().await.unwrap();

        // Create another index with auto search_k
        config.search_k = -1; // Auto
        let mut index2 = AxisAnnoyIndex::new(config, 8).unwrap();

        for (id, vec) in &vectors {
            let record = create_test_record(id.clone(), vec.clone(), vec![]);
            index2.add(id.clone(), record).await.unwrap();
        }

        index2.build().await.unwrap();

        // Both should return results
        let query = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let results1 = index1.search(&query, 5, None).await.unwrap();
        let results2 = index2.search(&query, 5, None).await.unwrap();

        assert_eq!(results1.len(), 5);
        assert_eq!(results2.len(), 5);
        // Both should have found reasonable results (Annoy is approximate)
        assert!(results1[0].1 < 10.0, "First result distance should be reasonable");
        assert!(results2[0].1 < 10.0, "Second result distance should be reasonable");
    }

    #[tokio::test]
    async fn test_annoy_large_leaf_size() {
        let config = AxisAnnoyConfig {
            n_trees: 3,
            search_k: -1,
            max_leaf_size: 100, // Large leaf size
            seed: 42,
            distance_metric: DistanceMetric::Euclidean,
        };

        let mut index = AxisAnnoyIndex::new(config, 4).unwrap();

        // Add only 10 vectors (less than max_leaf_size)
        for i in 0..10 {
            let mut vec = vec![0.0; 4];
            vec[i % 4] = 1.0;
            
            let record = create_test_record(format!("vec_{}", i), vec, vec![]);
            index.add(format!("vec_{}", i), record).await.unwrap();
        }

        // Build should still work
        index.build().await.unwrap();

        // Search should work
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 5, None).await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_annoy_empty_index() {
        let config = AxisAnnoyConfig::default();
        let mut index = AxisAnnoyIndex::new(config, 4).unwrap();

        // Build empty index
        index.build().await.unwrap();

        // Search on empty index
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 5, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_annoy_dimension_mismatch() {
        let config = AxisAnnoyConfig::default();
        let mut index = AxisAnnoyIndex::new(config, 4).unwrap();

        // Try to add vector with wrong dimension
        let record = create_test_record("v1".to_string(), vec![1.0, 0.0], vec![]); // Wrong dimension

        let result = index.add("v1".to_string(), record).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("dimension"));

        // Add correct vector
        let record = create_test_record("v1".to_string(), vec![1.0, 0.0, 0.0, 0.0], vec![]);
        index.add("v1".to_string(), record).await.unwrap();

        // Build
        index.build().await.unwrap();

        // Try to search with wrong dimension
        let query = vec![1.0, 0.0]; // Wrong dimension
        let result = index.search(&query, 5, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("dimension"));
    }

    #[tokio::test]
    async fn test_annoy_stats() {
        let config = AxisAnnoyConfig {
            n_trees: 5,
            search_k: -1,
            max_leaf_size: 5,
            seed: 42,
            distance_metric: DistanceMetric::Euclidean,
        };

        let mut index = AxisAnnoyIndex::new(config, 4).unwrap();

        // Check stats before adding vectors
        let stats = index.stats();
        assert_eq!(stats.vector_count, 0);
        assert_eq!(stats.tree_count, 0);
        assert!(!stats.is_built);

        // Add vectors
        let vectors = create_test_vectors(20, 4);
        for (id, vec) in &vectors {
            let record = create_test_record(id.clone(), vec.clone(), vec![]);
            index.add(id.clone(), record).await.unwrap();
        }

        // Check stats after adding vectors
        let stats = index.stats();
        assert_eq!(stats.vector_count, 20);
        assert_eq!(stats.tree_count, 0); // Not built yet
        assert!(!stats.is_built);

        // Build index
        index.build().await.unwrap();

        // Check stats after building
        let stats = index.stats();
        assert_eq!(stats.vector_count, 20);
        assert_eq!(stats.tree_count, 5);
        assert!(stats.is_built);
        assert!(stats.total_nodes > 0);
        assert!(stats.avg_tree_depth > 0.0);
    }
}