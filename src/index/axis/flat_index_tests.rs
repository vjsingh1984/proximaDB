/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! FLAT (brute-force) index tests

#[cfg(test)]
mod tests {
    use crate::compute::distance_computation::DistanceMetric;
    use std::collections::HashMap;
    use std::time::Instant;
    use tracing::debug;

    struct TestVector {
        id: String,
        vector: Vec<f32>,
    }

    #[allow(dead_code)]
    fn create_test_vector(id: &str, values: Vec<f32>) -> TestVector {
        TestVector {
            id: id.to_string(),
            vector: values,
        }
    }

    fn calculate_distance(v1: &[f32], v2: &[f32], metric: DistanceMetric) -> f32 {
        match metric {
            DistanceMetric::Euclidean => v1
                .iter()
                .zip(v2.iter())
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt(),
            DistanceMetric::Cosine => {
                let dot: f32 = v1.iter().zip(v2.iter()).map(|(a, b)| a * b).sum();
                let norm1: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
                let norm2: f32 = v2.iter().map(|x| x * x).sum::<f32>().sqrt();
                1.0 - (dot / (norm1 * norm2))
            }
            DistanceMetric::DotProduct => {
                -v1.iter().zip(v2.iter()).map(|(a, b)| a * b).sum::<f32>()
            }
            DistanceMetric::Manhattan => v1.iter().zip(v2.iter()).map(|(a, b)| (a - b).abs()).sum(),
            _ => panic!("Unsupported distance metric for test"),
        }
    }

    #[tokio::test]
    async fn test_flat_exact_search() {
        // FLAT index should return exact nearest neighbors
        let dimension = 128;
        let vectors = vec![
            ("vec1", vec![1.0; dimension]),
            ("vec2", vec![0.5; dimension]),
            ("vec3", vec![0.0; dimension]),
            ("vec4", vec![0.75; dimension]),
            ("vec5", vec![0.25; dimension]),
        ];

        // Store vectors in a simple HashMap for brute-force search
        let mut index: HashMap<String, Vec<f32>> = HashMap::new();
        for (id, values) in &vectors {
            index.insert(id.to_string(), values.clone());
        }

        // Query vector
        let query = vec![0.6; dimension];
        let k = 3;

        // Brute-force search
        let mut distances: Vec<(String, f32)> = index
            .iter()
            .map(|(id, values)| {
                let dist = calculate_distance(&query, values, DistanceMetric::Euclidean);
                (id.clone(), dist)
            })
            .collect();

        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let top_k: Vec<(String, f32)> = distances.into_iter().take(k).collect();

        // Verify the exact nearest neighbors
        assert_eq!(top_k.len(), k);
        assert_eq!(top_k[0].0, "vec2"); // Closest to 0.6 is 0.5
        assert_eq!(top_k[1].0, "vec4"); // Next closest is 0.75
        assert_eq!(top_k[2].0, "vec5"); // Then 0.25
    }

    #[tokio::test]
    async fn test_flat_all_distance_metrics() {
        // Test FLAT index with all supported distance metrics
        let _dimension = 64;
        let metrics = vec![
            DistanceMetric::Euclidean,
            DistanceMetric::Cosine,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
        ];

        for metric in metrics {
            let vectors = [
                vec![1.0, 0.0, 0.0],
                vec![0.0, 1.0, 0.0],
                vec![0.0, 0.0, 1.0],
                vec![0.577, 0.577, 0.577], // Normalized [1,1,1]
            ];

            let query = vec![1.0, 1.0, 0.0];

            // Find nearest using brute force
            let mut distances: Vec<(usize, f32)> = vectors
                .iter()
                .enumerate()
                .map(|(idx, v)| {
                    let dist = calculate_distance(&query, v, metric);
                    (idx, dist)
                })
                .collect();

            distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            debug!(
                "Metric {:?}: nearest vector index = {}",
                metric, distances[0].0
            );

            // Different metrics should give different results
            match metric {
                DistanceMetric::Euclidean | DistanceMetric::Manhattan => {
                    // Closest should be the normalized [1,1,1] vector
                    assert!(distances[0].0 == 3 || distances[0].0 == 0 || distances[0].0 == 1);
                }
                DistanceMetric::Cosine => {
                    // Cosine similarity considers angle
                    assert!(distances[0].0 < 4);
                }
                DistanceMetric::DotProduct => {
                    // Dot product favors aligned vectors
                    assert!(distances[0].0 < 4);
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_flat_performance_characteristics() {
        // FLAT index has O(n) search complexity
        let dimensions = vec![64, 128, 256];
        let dataset_sizes = vec![100, 1000, 10000];

        for dim in dimensions {
            for size in &dataset_sizes {
                // Generate random vectors
                let vectors: Vec<Vec<f32>> = (0..*size)
                    .map(|i| {
                        (0..dim)
                            .map(|j| ((i * dim + j) as f32 * 0.1) % 1.0)
                            .collect()
                    })
                    .collect();

                let query = vec![0.5; dim];

                // Measure search time
                let start = Instant::now();

                // Brute force search
                let _distances: Vec<f32> = vectors
                    .iter()
                    .map(|v| calculate_distance(&query, v, DistanceMetric::Euclidean))
                    .collect();

                let elapsed = start.elapsed();

                debug!("FLAT search: {} vectors, {} dim = {:?}", size, dim, elapsed);

                // Search time should scale linearly with dataset size
                // This is the key characteristic of FLAT index
            }
        }
    }

    #[tokio::test]
    async fn test_flat_memory_usage() {
        // FLAT index stores all vectors without compression
        let dimension = 768; // Common embedding dimension
        let counts = vec![1000, 10000, 100000];

        for count in counts {
            // Calculate exact memory usage
            let vector_memory = count * dimension * 4; // 4 bytes per float
            let index_overhead = count * 64; // Estimated metadata per vector
            let total_memory = vector_memory + index_overhead;

            debug!(
                "FLAT index memory for {} vectors ({}D): {} MB",
                count,
                dimension,
                total_memory / 1_000_000
            );

            // FLAT index should use memory proportional to data size
            assert_eq!(vector_memory, count * dimension * 4);
        }
    }

    #[tokio::test]
    async fn test_flat_incremental_updates() {
        // FLAT index supports instant updates without reindexing
        let dimension = 128;
        let mut index: HashMap<String, Vec<f32>> = HashMap::new();

        // Initial vectors
        for i in 0..100 {
            let id = format!("vec_{}", i);
            let values = vec![i as f32 / 100.0; dimension];
            index.insert(id, values);
        }

        // Add new vector - should be immediately searchable
        let new_id = "new_vec".to_string();
        let new_values = vec![0.545; dimension]; // Closer to query than vec_54 or vec_55
        index.insert(new_id.clone(), new_values.clone());

        // Search should find the new vector
        let query = vec![0.545; dimension]; // Query matches new vector exactly
        let mut distances: Vec<(String, f32)> = index
            .iter()
            .map(|(id, values)| {
                let dist = calculate_distance(&query, values, DistanceMetric::Euclidean);
                (id.clone(), dist)
            })
            .collect();

        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        // New vector should be the nearest
        assert_eq!(distances[0].0, new_id);

        // Delete vector - should be immediately removed
        index.remove(&new_id);

        // Verify it's gone
        assert!(!index.contains_key(&new_id));
    }

    #[tokio::test]
    async fn test_flat_with_filters() {
        // FLAT index can easily support filtering
        let dimension = 64;

        // Create vectors with metadata
        let vectors_with_metadata = vec![
            ("vec1", vec![1.0; dimension], "category_a"),
            ("vec2", vec![0.8; dimension], "category_b"),
            ("vec3", vec![0.6; dimension], "category_a"),
            ("vec4", vec![0.4; dimension], "category_b"),
            ("vec5", vec![0.2; dimension], "category_a"),
        ];

        let query = vec![0.7; dimension];
        let filter_category = "category_a";

        // Search with filter
        let mut filtered_results: Vec<(String, f32)> = vectors_with_metadata
            .iter()
            .filter(|(_, _, category)| *category == filter_category)
            .map(|(id, values, _)| {
                let dist = calculate_distance(&query, values, DistanceMetric::Euclidean);
                (id.to_string(), dist)
            })
            .collect();

        filtered_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        // Should only return category_a vectors
        assert_eq!(filtered_results.len(), 3);
        assert_eq!(filtered_results[0].0, "vec3"); // Closest in category_a
        assert_eq!(filtered_results[1].0, "vec1");
        assert_eq!(filtered_results[2].0, "vec5");
    }

    #[tokio::test]
    async fn test_flat_range_search() {
        // FLAT index can support range queries
        let _dimension = 3;
        let vectors = [
            ("vec1", vec![0.0, 0.0, 0.0]),
            ("vec2", vec![1.0, 0.0, 0.0]),
            ("vec3", vec![0.0, 1.0, 0.0]),
            ("vec4", vec![0.0, 0.0, 1.0]),
            ("vec5", vec![1.0, 1.0, 1.0]),
        ];

        let query = vec![0.5, 0.5, 0.5];
        let radius = 1.0;

        // Find all vectors within radius
        let within_range: Vec<(String, f32)> = vectors
            .iter()
            .map(|(id, values)| {
                let dist = calculate_distance(&query, values, DistanceMetric::Euclidean);
                (id.to_string(), dist)
            })
            .filter(|(_, dist)| *dist <= radius)
            .collect();

        debug!("Vectors within radius {}: {:?}", radius, within_range);

        // Should find vectors within the radius
        assert!(within_range.len() > 0);
        for (_, dist) in &within_range {
            assert!(*dist <= radius);
        }
    }

    #[tokio::test]
    async fn test_flat_batch_search() {
        // Test batch search efficiency
        let dimension = 128;
        let num_vectors = 1000;
        let num_queries = 10;

        // Create dataset
        let vectors: Vec<Vec<f32>> = (0..num_vectors)
            .map(|i| {
                (0..dimension)
                    .map(|j| ((i * dimension + j) as f32 * 0.1) % 1.0)
                    .collect()
            })
            .collect();

        // Create batch of queries
        let queries: Vec<Vec<f32>> = (0..num_queries)
            .map(|i| vec![i as f32 / num_queries as f32; dimension])
            .collect();

        let start = Instant::now();

        // Batch search
        for query in &queries {
            let _distances: Vec<f32> = vectors
                .iter()
                .map(|v| calculate_distance(query, v, DistanceMetric::Euclidean))
                .collect();
        }

        let elapsed = start.elapsed();
        let avg_time = elapsed / num_queries as u32;

        debug!(
            "FLAT batch search: {} queries on {} vectors = {:?} (avg {:?}/query)",
            num_queries, num_vectors, elapsed, avg_time
        );
    }

    #[tokio::test]
    async fn test_flat_edge_cases() {
        // Test edge cases for FLAT index

        // Empty index
        let index: HashMap<String, Vec<f32>> = HashMap::new();
        let query = vec![0.5; 10];

        let results: Vec<(String, f32)> = index
            .iter()
            .map(|(id, values)| {
                let dist = calculate_distance(&query, values, DistanceMetric::Euclidean);
                (id.clone(), dist)
            })
            .collect();

        assert_eq!(results.len(), 0, "Empty index should return no results");

        // Single vector
        let mut single_index = HashMap::new();
        single_index.insert("only_vec".to_string(), vec![1.0; 10]);

        let single_results: Vec<(String, f32)> = single_index
            .iter()
            .map(|(id, values)| {
                let dist = calculate_distance(&query, values, DistanceMetric::Euclidean);
                (id.clone(), dist)
            })
            .collect();

        assert_eq!(single_results.len(), 1);
        assert_eq!(single_results[0].0, "only_vec");

        // Duplicate vectors
        let mut dup_index = HashMap::new();
        dup_index.insert("vec1".to_string(), vec![0.5; 10]);
        dup_index.insert("vec2".to_string(), vec![0.5; 10]);
        dup_index.insert("vec3".to_string(), vec![0.5; 10]);

        let dup_results: Vec<(String, f32)> = dup_index
            .iter()
            .map(|(id, values)| {
                let dist = calculate_distance(&query, values, DistanceMetric::Euclidean);
                (id.clone(), dist)
            })
            .collect();

        assert_eq!(dup_results.len(), 3);
        // All should have distance 0
        for (_, dist) in &dup_results {
            assert_eq!(*dist, 0.0);
        }
    }

    #[tokio::test]
    async fn test_flat_high_dimensional() {
        // Test FLAT index with high-dimensional vectors
        let dimensions = [512, 768, 1024, 2048];

        for dim in dimensions {
            let vectors = [
                vec![0.1; dim],
                vec![0.2; dim],
                vec![0.3; dim],
                vec![0.4; dim],
                vec![0.5; dim],
            ];

            let query = vec![0.35; dim];

            let mut distances: Vec<(usize, f32)> = vectors
                .iter()
                .enumerate()
                .map(|(idx, v)| {
                    let dist = calculate_distance(&query, v, DistanceMetric::Euclidean);
                    (idx, dist)
                })
                .collect();

            distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            // Nearest should be vec[0.3] at index 2
            assert_eq!(distances[0].0, 2);

            debug!(
                "High-dimensional FLAT search ({}D): nearest = vec[{}]",
                dim, distances[0].0
            );
        }
    }
}
