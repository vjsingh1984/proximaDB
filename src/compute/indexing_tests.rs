//! Tests for advanced indexing algorithms

#[cfg(test)]
mod tests {
    use super::super::*;
    use std::sync::Arc;
    
    fn create_test_vectors(n: usize, dim: usize) -> Vec<Vec<f32>> {
        (0..n)
            .map(|i| {
                (0..dim)
                    .map(|j| ((i * dim + j) as f32).sin())
                    .collect()
            })
            .collect()
    }
    
    fn create_test_record(id: &str, vector: Vec<f32>) -> Arc<VectorRecord> {
        Arc::new(VectorRecord {
            id: Some(id.to_string()),
            vector,
            metadata: vec![],
            timestamp: 0,
            created_at: 0,
            updated_at: 0,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        })
    }
    
    #[test]
    fn test_ivf_index_basic() {
        let config = IvfConfig {
            n_clusters: 10,
            n_probe: 3,
            train_size: 100,
            max_iterations: 10,
            distance_metric: DistanceMetric::Euclidean,
        };
        
        let dim = 128;
        let mut index = IvfIndex::new(config, dim);
        
        // Train the index
        let training_vectors = create_test_vectors(100, dim);
        index.train(&training_vectors).unwrap();
        
        // Add vectors
        for (i, vector) in training_vectors.iter().enumerate() {
            let record = create_test_record(&format!("vec_{}", i), vector.clone());
            index.add(format!("vec_{}", i), record).unwrap();
        }
        
        // Search
        let query = create_test_vectors(1, dim)[0].clone();
        let results = index.search(&query, 5).unwrap();
        
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0, "vec_0"); // Should find itself as nearest
        assert!(results[0].1 < 0.01); // Distance should be very small
    }
    
    #[test]
    fn test_lsh_index_basic() {
        let config = LshConfig {
            n_tables: 10,
            n_hashes: 8,
            seed: 42,
        };
        
        let dim = 64;
        let index = LshIndex::new(config, dim);
        
        // Add vectors
        let vectors = create_test_vectors(50, dim);
        for (i, vector) in vectors.iter().enumerate() {
            let record = create_test_record(&format!("vec_{}", i), vector.clone());
            index.add(format!("vec_{}", i), record).unwrap();
        }
        
        // Search
        let query = vectors[10].clone();
        let results = index.search(&query, 5).unwrap();
        
        assert!(!results.is_empty());
        // Should find vec_10 among the top results
        let found = results.iter().any(|(id, _)| id == "vec_10");
        assert!(found, "Should find the query vector itself");
    }
    
    #[test]
    fn test_ivf_clustering_convergence() {
        let config = IvfConfig {
            n_clusters: 5,
            n_probe: 2,
            train_size: 50,
            max_iterations: 100,
            distance_metric: DistanceMetric::Euclidean,
        };
        
        let dim = 32;
        let mut index = IvfIndex::new(config, dim);
        
        // Create well-separated clusters
        let mut training_vectors = Vec::new();
        for cluster in 0..5 {
            for _ in 0..10 {
                let mut vec = vec![0.0; dim];
                vec[cluster * 6] = 10.0; // Separate clusters in different dimensions
                training_vectors.push(vec);
            }
        }
        
        index.train(&training_vectors).unwrap();
        
        // Verify centroids are well-separated
        for i in 0..5 {
            let centroid = &index.centroids[i];
            let max_val = centroid.iter().fold(0.0f32, |a, &b| a.max(b));
            assert!(max_val > 5.0, "Centroid should have significant values");
        }
    }
    
    #[test]
    fn test_lsh_hash_distribution() {
        let config = LshConfig {
            n_tables: 5,
            n_hashes: 16,
            seed: 123,
        };
        
        let dim = 128;
        let index = LshIndex::new(config, dim);
        
        // Add many vectors
        let vectors = create_test_vectors(1000, dim);
        for (i, vector) in vectors.iter().enumerate() {
            let record = create_test_record(&format!("vec_{}", i), vector.clone());
            index.add(format!("vec_{}", i), record).unwrap();
        }
        
        // Check that vectors are distributed across buckets
        let mut total_buckets = 0;
        let mut non_empty_buckets = 0;
        
        for table in &index.hash_tables {
            total_buckets += table.len();
            non_empty_buckets += table.len();
        }
        
        // Should have reasonable distribution
        assert!(non_empty_buckets > 10, "Should have multiple non-empty buckets");
        assert!(non_empty_buckets < 900, "Should not have too many buckets");
    }
}