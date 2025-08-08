// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

#[cfg(test)]
mod tests {
    // TODO: These tests need to be migrated to the index module
    // use crate::compute::algorithms::*;
    use crate::compute::DistanceMetric;
    use crate::core::search::SearchResult;
    use std::collections::HashMap;
    
    fn create_test_vectors(n: usize, dim: usize) -> Vec<(String, Vec<f32>)> {
        (0..n)
            .map(|i| {
                let id = format!("vec_{}", i);
                let vector = (0..dim)
                    .map(|j| ((i * dim + j) as f32 * 0.1).sin())
                    .collect();
                (id, vector)
            })
            .collect()
    }
    
    fn create_test_vectors_with_metadata(n: usize, dim: usize) -> Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)> {
        (0..n)
            .map(|i| {
                let id = format!("vec_{}", i);
                let vector = (0..dim)
                    .map(|j| ((i * dim + j) as f32 * 0.1).sin())
                    .collect();
                let mut metadata = HashMap::new();
                metadata.insert("category".to_string(), serde_json::json!(format!("cat_{}", i % 3)));
                metadata.insert("score".to_string(), serde_json::json!(i as f64 * 0.5));
                (id, vector, Some(metadata))
            })
            .collect()
    }
    
    #[test]
    fn test_search_result_ordering() {
        // Test with distance scores (lower is better)
        let mut r1 = SearchResult::new("vec1".to_string(), 0.1);  // Good match (low distance)
        r1.vector_id = Some("vec1".to_string());
        
        let mut r2 = SearchResult::new("vec2".to_string(), 0.5);  // Worse match (higher distance)
        r2.vector_id = Some("vec2".to_string());
        
        let mut r3 = SearchResult::new("vec3".to_string(), 0.05); // Best match (lowest distance)
        r3.vector_id = Some("vec3".to_string());
        
        // The Ord implementation does: other.score.partial_cmp(&self.score)
        // This means: if self.score < other.score, then self > other
        // So for our distance scores: r3(0.05) > r1(0.1) > r2(0.5)
        assert!(r3 > r1); // 0.05 < 0.1 means r3 > r1
        assert!(r1 > r2); // 0.1 < 0.5 means r1 > r2
        
        // Test heap behavior - should give us lowest scores first
        let mut heap = std::collections::BinaryHeap::new();
        heap.push(r1.clone());
        heap.push(r2.clone());
        heap.push(r3.clone());
        
        // BinaryHeap is a max-heap, so it pops the "largest" element first
        // Since r3 > r1 > r2 (due to reversed Ord), we get r3 first
        // So we expect: 0.05, 0.1, 0.5 (best to worst distances)
        assert_eq!(heap.pop().unwrap().score, 0.05);
        assert_eq!(heap.pop().unwrap().score, 0.1);
        assert_eq!(heap.pop().unwrap().score, 0.5);
    }
    
    #[test]
    fn test_hnsw_basic_operations() {
        let mut index = HNSWIndex::new(16, 200, DistanceMetric::Cosine, true);
        
        // Test empty index
        assert_eq!(index.size(), 0);
        let memory = index.memory_usage();
        assert_eq!(memory.total_bytes, memory.index_size_bytes + memory.vector_data_bytes + memory.metadata_bytes);
        
        // Add single vector
        let result = index.add_vector(
            "vec1".to_string(),
            vec![0.1, 0.2, 0.3, 0.4],
            None,
        );
        assert!(result.is_ok());
        assert_eq!(index.size(), 1);
        
        // Search in single-vector index
        let results = index.search(&[0.1, 0.2, 0.3, 0.4], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].vector_id, Some("vec1".to_string()));
        // For cosine distance, perfect match should have score close to 0
        assert!(results[0].score < 0.01); // Should be very small distance
        
        // Add multiple vectors
        let vectors = create_test_vectors(10, 4);
        let batch_vectors: Vec<_> = vectors.into_iter()
            .map(|(id, vec)| (id, vec, None))
            .collect();
        assert!(index.add_vectors(batch_vectors).is_ok());
        assert_eq!(index.size(), 11);
        
        // Test k-NN search
        let query = vec![0.1, 0.2, 0.3, 0.4];
        let results = index.search(&query, 5).unwrap();
        assert_eq!(results.len(), 5);
        
        // Results should be sorted by distance (ascending) - lower is better
        for i in 1..results.len() {
            assert!(results[i-1].score <= results[i].score);
        }
        
        // Remove vector
        assert!(index.remove_vector("vec1").unwrap());
        assert_eq!(index.size(), 10);
        assert!(!index.remove_vector("nonexistent").unwrap());
    }
    
    #[test]
    fn test_hnsw_with_metadata_filter() {
        let mut index = HNSWIndex::new(16, 200, DistanceMetric::Euclidean, false);
        
        // Add vectors with metadata
        let vectors = create_test_vectors_with_metadata(20, 8);
        assert!(index.add_vectors(vectors).is_ok());
        
        // Search without filter
        let query = vec![0.1; 8];
        let all_results = index.search(&query, 10).unwrap();
        assert_eq!(all_results.len(), 10);
        
        // Search with category filter
        let filter = |metadata: &HashMap<String, serde_json::Value>| -> bool {
            metadata.get("category")
                .and_then(|v| v.as_str())
                .map(|cat| cat == "cat_0")
                .unwrap_or(false)
        };
        
        let filtered_results = index.search_with_filter(&query, 10, &filter).unwrap();
        assert!(filtered_results.len() <= 10);
        
        // Verify all results match filter
        for result in &filtered_results {
            if !result.metadata.is_empty() {
                assert_eq!(result.metadata.get("category").unwrap().as_str().unwrap(), "cat_0");
            }
        }
    }
    
    #[test]
    fn test_hnsw_edge_cases() {
        let mut index = HNSWIndex::new(16, 200, DistanceMetric::DotProduct, true);
        
        // Search empty index
        let results = index.search(&[0.1, 0.2], 5);
        assert!(results.is_ok());
        assert_eq!(results.unwrap().len(), 0);
        
        // Add vectors with duplicate IDs (should fail)
        index.add_vector("dup".to_string(), vec![1.0, 2.0], None).unwrap();
        let result = index.add_vector("dup".to_string(), vec![3.0, 4.0], None);
        assert!(result.is_err());
        assert_eq!(index.size(), 1);
        
        // Search should find the first vector (since duplicate was rejected)
        let results = index.search(&[1.0, 2.0], 1).unwrap();
        assert_eq!(results[0].vector_id, Some("dup".to_string()));
        
        // Request more results than available
        index.add_vector("vec2".to_string(), vec![5.0, 6.0], None).unwrap();
        let results = index.search(&[1.0, 1.0], 10).unwrap();
        assert_eq!(results.len(), 2); // Only 2 vectors in index
    }
    
    #[test]
    fn test_hnsw_optimization() {
        let mut index = HNSWIndex::new(8, 100, DistanceMetric::Cosine, false);
        
        // Add many vectors
        let vectors = create_test_vectors(100, 16);
        let batch_vectors: Vec<_> = vectors.into_iter()
            .map(|(id, vec)| (id, vec, None))
            .collect();
        index.add_vectors(batch_vectors).unwrap();
        
        // Get memory before optimization
        let memory_before = index.memory_usage();
        
        // Optimize index
        assert!(index.optimize().is_ok());
        
        // Memory usage might change after optimization
        let memory_after = index.memory_usage();
        assert!(memory_after.total_bytes > 0);
        
        // Search should still work after optimization
        let query = vec![0.5; 16];
        let results = index.search(&query, 10).unwrap();
        assert_eq!(results.len(), 10);
    }
    
    
    #[test]
    fn test_hnsw_different_metrics() {
        let dimensions = 32;
        let num_vectors = 50;
        
        // Test with each distance metric
        for metric in &[
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
        ] {
            let mut index = HNSWIndex::new(16, 200, *metric, true);
            
            // Add test vectors
            let vectors = create_test_vectors(num_vectors, dimensions);
            let batch_vectors: Vec<_> = vectors.iter()
                .map(|(id, vec)| (id.clone(), vec.clone(), None))
                .collect();
            index.add_vectors(batch_vectors).unwrap();
            
            // Search and verify results
            let query = vec![0.5; dimensions];
            let results = index.search(&query, 10).unwrap();
            
            assert_eq!(results.len(), 10);
            assert!(!results.is_empty());
            
            // Scores should be ordered (ascending for distances)
            for i in 1..results.len() {
                assert!(results[i-1].score <= results[i].score,
                    "Scores not ordered for metric {:?}", metric);
            }
        }
    }
    
    #[test]
    fn test_memory_usage_tracking() {
        let mut index = HNSWIndex::new(16, 200, DistanceMetric::Cosine, true);
        
        // Initial memory usage
        let initial_memory = index.memory_usage();
        assert_eq!(initial_memory.vector_data_bytes, 0);
        assert_eq!(initial_memory.metadata_bytes, 0);
        
        // Add vectors and check memory increase
        let vector_size = 128;
        let num_vectors = 10;
        
        for i in 0..num_vectors {
            let vector = vec![i as f32; vector_size];
            let mut metadata = HashMap::new();
            metadata.insert("key".to_string(), serde_json::json!("value"));
            
            index.add_vector(
                format!("vec_{}", i),
                vector,
                Some(metadata),
            ).unwrap();
        }
        
        let final_memory = index.memory_usage();
        assert!(final_memory.vector_data_bytes > initial_memory.vector_data_bytes);
        assert!(final_memory.metadata_bytes > initial_memory.metadata_bytes);
        assert!(final_memory.total_bytes > initial_memory.total_bytes);
        
        // Memory should be reasonable
        let expected_vector_bytes = num_vectors * vector_size * std::mem::size_of::<f32>();
        assert!(final_memory.vector_data_bytes >= expected_vector_bytes);
    }
    
    #[test]
    fn test_concurrent_search() {
        use std::sync::Arc;
        use std::thread;
        
        let mut index = HNSWIndex::new(16, 200, DistanceMetric::Cosine, true);
        
        // Add test data
        let vectors = create_test_vectors(100, 64);
        let batch_vectors: Vec<_> = vectors.into_iter()
            .map(|(id, vec)| (id, vec, None))
            .collect();
        index.add_vectors(batch_vectors).unwrap();
        
        // Wrap in Arc for sharing across threads
        let index = Arc::new(index);
        
        // Spawn multiple search threads
        let mut handles = vec![];
        for i in 0..4 {
            let index_clone = Arc::clone(&index);
            let handle = thread::spawn(move || {
                let query = vec![i as f32 * 0.1; 64];
                let results = index_clone.search(&query, 10);
                assert!(results.is_ok());
                let results = results.unwrap();
                assert_eq!(results.len(), 10);
                results
            });
            handles.push(handle);
        }
        
        // Wait for all threads and verify results
        for handle in handles {
            let results = handle.join().unwrap();
            assert_eq!(results.len(), 10);
        }
    }
}