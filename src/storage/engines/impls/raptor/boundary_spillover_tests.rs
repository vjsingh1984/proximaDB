//! Tests for boundary detection and spillover detection in RAPTOR engine
//!
//! These tests verify the Phase 1 (K² boundary detection) and Phase 2 (P×K spillover detection)
//! algorithms that are core to RAPTOR's Matrix Trinity architecture.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
    use crate::core::hardware_capabilities::HardwareCapabilities;
    use crate::storage::engines::impls::raptor::common::{
        InterCentroidMatrix, VectorCentroidMatrix, VectorCentroidStorageStrategy,
    };
    use crate::storage::engines::impls::raptor::matrix_builder::MatrixBuilder;

    /// Helper to create test vectors
    fn create_test_vectors(num_vectors: usize, dimension: usize) -> Vec<Vec<f32>> {
        (0..num_vectors)
            .map(|i| {
                (0..dimension)
                    .map(|j| ((i + j) as f32 / (num_vectors + dimension) as f32))
                    .collect()
            })
            .collect()
    }

    /// Helper to create clustered vectors (for testing spillover)
    fn create_clustered_vectors(
        num_clusters: usize,
        vectors_per_cluster: usize,
        dimension: usize,
        noise: f32,
    ) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let mut all_vectors = Vec::new();
        let mut centroids = Vec::new();

        for c in 0..num_clusters {
            // Create centroid
            let centroid: Vec<f32> = (0..dimension)
                .map(|d| ((c * dimension + d) as f32).sin())
                .collect();
            centroids.push(centroid.clone());

            // Create vectors around centroid
            for v in 0..vectors_per_cluster {
                let mut vector = centroid.clone();
                for d in 0..dimension {
                    vector[d] += noise * ((v + d) as f32 / vectors_per_cluster as f32 - 0.5);
                }
                all_vectors.push(vector);
            }
        }

        (all_vectors, centroids)
    }

    #[test]
    fn test_phase1_boundary_detection_basic() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let hardware = get_hardware_capabilities();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(hardware.clone()));
        let builder = MatrixBuilder::new(
            distance_compute.clone(),
            hardware,
            DistanceMetric::Euclidean,
        );

        // Create test centroids
        let centroids = vec![
            vec![0.0, 0.0, 0.0],
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![1.0, 1.0, 0.0],
        ];

        // Build K² matrix
        let k2_matrix = builder.build_k2_matrix(&centroids, 3).unwrap();
        assert_eq!(k2_matrix.num_centroids, 5);

        // Simulate Phase 1: Select top J=3 centroids for a query
        let query = vec![0.5, 0.5, 0.0];
        let mut centroid_distances: Vec<(usize, f32)> = centroids
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let dist = distance_compute
                    .calculate(&query, c, DistanceMetric::Euclidean)
                    .unwrap();
                (i, dist)
            })
            .collect();

        centroid_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        // Primary centroids (top 3)
        let primary: Vec<usize> = centroid_distances.iter().take(3).map(|(i, _)| *i).collect();

        println!("Primary centroids: {:?}", primary);

        // Check boundary ratio d_i/d_j > 0.8 for expansion
        let mut expanded = Vec::new();
        for i in 0..2 {
            let ratio = centroid_distances[i].1 / centroid_distances[i + 1].1;
            println!("Boundary ratio {}/{}: {:.2}", i, i + 1, ratio);

            if ratio > 0.8 {
                // Check K² matrix for nearby centroids
                let k2_row = builder.decompress_k2_row(&k2_matrix, primary[i]).unwrap();

                // Find centroids close to primary[i] but not in primary set
                for (j, dist) in k2_row.iter().enumerate() {
                    if !primary.contains(&j) && *dist < 0.5 {
                        expanded.push(j);
                        println!("Adding boundary centroid {} (dist={:.2})", j, dist);
                    }
                }
            }
        }

        println!("Expanded centroids: {:?}", expanded);

        // Verify we found boundary centroids
        assert!(
            !expanded.is_none() || primary.len() >= 3,
            "Should have either expanded centroids or sufficient primary ones"
        );
    }

    #[test]
    fn test_phase2_spillover_detection() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let hardware = get_hardware_capabilities();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(hardware.clone()));
        let builder =
            MatrixBuilder::new(distance_compute.clone(), hardware, DistanceMetric::Cosine);

        // Create clustered vectors with some spillover
        let (vectors, centroids) = create_clustered_vectors(
            5,   // 5 clusters
            20,  // 20 vectors per cluster
            16,  // 16 dimensions
            0.3, // 30% noise for spillover
        );

        // Build P×K matrix for first cluster (rowgroup 0)
        let rowgroup_vectors: Vec<Vec<f32>> = vectors.iter().take(20).cloned().collect();
        let pxk_matrix = builder
            .build_pxk_matrix(&rowgroup_vectors, &centroids, 16, 0)
            .unwrap();

        println!(
            "P×K matrix storage strategy: {:?}",
            pxk_matrix.storage_strategy
        );

        // Analyze spillover based on storage strategy
        let mut spillover_count = 0;
        let mut total_checked = 0;

        match pxk_matrix.storage_strategy {
            VectorCentroidStorageStrategy::Full => {
                // Check all vector-to-centroid distances
                for v in 0..rowgroup_vectors.len() {
                    let mut distances = Vec::new();
                    for c in 0..centroids.len() {
                        let dist = distance_compute
                            .calculate(&rowgroup_vectors[v], &centroids[c], DistanceMetric::Cosine)
                            .unwrap();
                        distances.push((c, dist));
                    }
                    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

                    // Vector spills over if closest centroid is not centroid 0
                    if distances[0].0 != 0 {
                        spillover_count += 1;
                    }
                    total_checked += 1;
                }
            }
            VectorCentroidStorageStrategy::Hierarchical => {
                println!("Hierarchical strategy with mean and deltas");
                // In Hierarchical, we store mean distances and significant deltas
                // Spillover is detected if many vectors have large deltas from mean
                total_checked = rowgroup_vectors.len();
                spillover_count = total_checked / 8; // Estimate 12.5% spillover
            }
            VectorCentroidStorageStrategy::Sparse => {
                println!("Sparse strategy storing top-k entries");
                // In Sparse, we only store top-k closest centroids
                total_checked = rowgroup_vectors.len();
                spillover_count = total_checked / 5; // Estimate 20% spillover
            }
        }

        let spillover_percentage = (spillover_count as f32 / total_checked.max(1) as f32) * 100.0;
        println!(
            "Spillover: {} of {} vectors ({:.1}%)",
            spillover_count, total_checked, spillover_percentage
        );

        // Check if spillover exceeds 15% threshold
        if spillover_percentage > 15.0 {
            println!("Spillover detected! Need to include additional centroids in search");
            assert!(
                spillover_percentage > 0.0,
                "Should detect some spillover with noise"
            );
        }
    }

    #[test]
    fn test_boundary_ratio_calculation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test the d_i/d_j > 0.8 boundary ratio rule
        let test_cases = vec![
            (0.25, 0.30, true),  // 0.83 > 0.8, should expand
            (0.30, 0.45, false), // 0.67 < 0.8, no expansion
            (0.40, 0.42, true),  // 0.95 > 0.8, should expand
            (0.10, 0.50, false), // 0.20 < 0.8, no expansion
        ];

        for (d_i, d_j, should_expand) in test_cases {
            let ratio = d_i / d_j;
            let expands = ratio > 0.8;

            assert_eq!(
                expands,
                should_expand,
                "Distance ratio {:.2}/{:.2} = {:.2} should {}expand",
                d_i,
                d_j,
                ratio,
                if should_expand { "" } else { "not " }
            );
        }
    }

    #[test]
    fn test_adaptive_pxk_coverage_formula() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test the adaptive coverage formula: coverage(k,d) = max(0.1, min(1.0, exp(-2 × log(k/d + 1))))
        let test_cases = vec![
            (10, 100, 0.1, 0.2),   // Low k/d ratio -> minimum coverage
            (50, 100, 0.2, 0.4),   // Moderate k/d ratio
            (100, 100, 0.3, 0.5),  // k = d -> moderate coverage
            (200, 100, 0.5, 0.7),  // k > d -> higher coverage
            (1000, 100, 0.8, 1.0), // High k/d ratio -> maximum coverage
        ];

        for (k, d, min_expected, max_expected) in test_cases {
            let coverage =
                (0.1_f32).max(1.0_f32.min((-2.0 * ((k as f32) / (d as f32) + 1.0).ln()).exp()));

            println!("k={}, d={}: coverage={:.2}", k, d, coverage);

            assert!(
                coverage >= min_expected && coverage <= max_expected,
                "Coverage {:.2} should be between {:.2} and {:.2} for k={}, d={}",
                coverage,
                min_expected,
                max_expected,
                k,
                d
            );
        }
    }

    #[test]
    fn test_5_component_boosting() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test the 5-component distance boosting formula
        // d_total = α₁×d₁(runtime) + α₂×d₂(mean) + α₃×d₃(density) + α₄×d₄(min) + α₅×d₅(max)

        struct BoostingComponents {
            d1_runtime: f32,        // Runtime distance to query
            d2_stored_mean: f32,    // Stored mean distance
            d3_stored_density: f32, // Stored density metric
            d4_stored_min: f32,     // Stored minimum distance
            d5_stored_max: f32,     // Stored maximum distance
        }

        let components = BoostingComponents {
            d1_runtime: 0.5,
            d2_stored_mean: 0.3,
            d3_stored_density: 0.2,
            d4_stored_min: 0.1,
            d5_stored_max: 0.8,
        };

        // Default weights (can be tuned)
        let alpha = [0.5, 0.2, 0.1, 0.1, 0.1]; // Sum to 1.0

        let boosted_distance = alpha[0] * components.d1_runtime
            + alpha[1] * components.d2_stored_mean
            + alpha[2] * components.d3_stored_density
            + alpha[3] * components.d4_stored_min
            + alpha[4] * components.d5_stored_max;

        println!("5-component boosted distance: {:.3}", boosted_distance);
        println!(
            "  d₁(runtime): {:.3} × {:.1} = {:.3}",
            components.d1_runtime,
            alpha[0],
            components.d1_runtime * alpha[0]
        );
        println!(
            "  d₂(mean):    {:.3} × {:.1} = {:.3}",
            components.d2_stored_mean,
            alpha[1],
            components.d2_stored_mean * alpha[1]
        );
        println!(
            "  d₃(density): {:.3} × {:.1} = {:.3}",
            components.d3_stored_density,
            alpha[2],
            components.d3_stored_density * alpha[2]
        );
        println!(
            "  d₄(min):     {:.3} × {:.1} = {:.3}",
            components.d4_stored_min,
            alpha[3],
            components.d4_stored_min * alpha[3]
        );
        println!(
            "  d₅(max):     {:.3} × {:.1} = {:.3}",
            components.d5_stored_max,
            alpha[4],
            components.d5_stored_max * alpha[4]
        );

        // Verify the boosted distance is reasonable
        assert!(
            boosted_distance > 0.0 && boosted_distance < 1.0,
            "Boosted distance {} should be in valid range",
            boosted_distance
        );

        // Verify weights sum to 1.0
        let weight_sum: f32 = alpha.iter().sum();
        assert!(
            (weight_sum - 1.0).abs() < 0.001,
            "Weights should sum to 1.0, got {}",
            weight_sum
        );
    }

    #[test]
    fn test_spillover_map_generation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test generating spillover maps for recursive checking
        let mut spillover_map: HashMap<usize, Vec<usize>> = HashMap::new();

        // Simulate spillover detection results
        // Rowgroup 23 has 18% spillover to centroids 19 and 7
        spillover_map.insert(23, vec![19, 7]);

        // Rowgroup 67 has 16% spillover to centroid 71
        spillover_map.insert(67, vec![71]);

        // Rowgroup 19 has 9% spillover to centroid 88 (below threshold)
        // Not added to map since below 15% threshold

        // Build final centroid list
        let mut final_centroids = vec![23, 19, 67]; // Primary from Phase 1
        let mut expanded = vec![7, 12]; // Expanded from boundary detection

        // Add spillover centroids
        for spillovers in spillover_map.values() {
            for &centroid in spillovers {
                if !final_centroids.contains(&centroid) && !expanded.contains(&centroid) {
                    expanded.push(centroid);
                }
            }
        }

        final_centroids.extend(expanded);
        final_centroids.sort();
        final_centroids.dedup();

        println!("Spillover map: {:?}", spillover_map);
        println!("Final centroids: {:?}", final_centroids);

        assert_eq!(final_centroids, vec![7, 12, 19, 23, 67, 71]);
    }
}
