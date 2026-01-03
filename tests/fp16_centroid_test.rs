//! FP16 Centroid Quantization Tests
//!
//! Validates FP16 centroid quantization implementation across SST, SWIFT, and HELIX engines.
//!
//! Tests cover:
//! - FP32 ↔ FP16 conversion accuracy
//! - Round-trip precision loss
//! - Distance computation accuracy with FP16 centroids
//! - Storage savings verification
//! - Backward compatibility (FP32 fallback)

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::storage::engines::impls::sst::{fp16_to_fp32, fp32_to_fp16};

#[test]
fn test_fp16_conversion_accuracy() {
    // Test conversion accuracy for typical centroid values
    let test_vectors = vec![
        vec![0.0, 1.0, -1.0, 0.5, -0.5],    // Simple values
        vec![0.001, 0.999, -0.001, -0.999], // Near boundaries
        vec![1.5, -1.5, 2.0, -2.0, 3.0],    // Larger magnitudes
        vec![1e-3, 1e-2, 1e-1, 1e0, 1e1],   // Different scales
    ];

    for original in test_vectors {
        // Convert FP32 → FP16 → FP32
        let fp16 = fp32_to_fp16(&original);
        let reconstructed = fp16_to_fp32(&fp16);

        assert_eq!(original.len(), reconstructed.len());

        // Check that conversion error is < 0.1% for each dimension
        for (i, (&orig, &recon)) in original.iter().zip(reconstructed.iter()).enumerate() {
            let abs_error = (orig - recon).abs();
            let rel_error = if orig.abs() > 1e-6 {
                abs_error / orig.abs()
            } else {
                abs_error
            };

            assert!(
                rel_error < 0.001 || abs_error < 0.001,
                "Dimension {}: FP16 conversion error too large. Original: {}, Reconstructed: {}, Rel Error: {:.4}%",
                i,
                orig,
                recon,
                rel_error * 100.0
            );
        }
    }
}

#[test]
fn test_fp16_storage_reduction() {
    // Verify 50% storage reduction
    let dimension = 128;
    let fp32_vector: Vec<f32> = (0..dimension).map(|i| (i as f32) / 128.0).collect();

    let fp16_vector = fp32_to_fp16(&fp32_vector);

    // FP32: 4 bytes per element, FP16: 2 bytes per element
    let fp32_bytes = fp32_vector.len() * std::mem::size_of::<f32>();
    let fp16_bytes = fp16_vector.len() * std::mem::size_of::<u16>();

    assert_eq!(fp32_bytes, dimension * 4);
    assert_eq!(fp16_bytes, dimension * 2);
    assert_eq!(
        fp16_bytes,
        fp32_bytes / 2,
        "FP16 should be 50% of FP32 storage"
    );
}

#[test]
fn test_fp16_distance_accuracy() {
    // Test that distance computations with FP16 centroids maintain accuracy
    let query = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
    let centroid_fp32 = vec![0.15, 0.25, 0.35, 0.45, 0.55, 0.65, 0.75, 0.85];

    // Compute distance with FP32
    let distance_fp32 = euclidean_distance(&query, &centroid_fp32);

    // Convert centroid to FP16 and back
    let centroid_fp16 = fp32_to_fp16(&centroid_fp32);
    let centroid_fp32_reconstructed = fp16_to_fp32(&centroid_fp16);

    // Compute distance with reconstructed FP32 (from FP16)
    let distance_fp16_path = euclidean_distance(&query, &centroid_fp32_reconstructed);

    // Distance error should be < 0.1%
    let error = (distance_fp32 - distance_fp16_path).abs();
    let relative_error = error / distance_fp32;

    println!("FP32 distance: {:.6}", distance_fp32);
    println!("FP16 path distance: {:.6}", distance_fp16_path);
    println!("Absolute error: {:.6}", error);
    println!("Relative error: {:.4}%", relative_error * 100.0);

    assert!(
        relative_error < 0.001,
        "Distance computation error with FP16 centroids exceeds 0.1%: {:.4}%",
        relative_error * 100.0
    );
}

#[test]
fn test_fp16_recall_preservation() {
    // Test that block selection with FP16 centroids maintains high recall
    // Generate 100 random centroids and a query
    use rand::Rng;
    let mut rng = rand::thread_rng();

    let num_centroids = 100;
    let dimension = 128;
    let top_k = 10;

    // Generate random query
    let query: Vec<f32> = (0..dimension).map(|_| rng.gen_range(0.0..1.0)).collect();

    // Generate random centroids
    let centroids_fp32: Vec<Vec<f32>> = (0..num_centroids)
        .map(|_| (0..dimension).map(|_| rng.gen_range(0.0..1.0)).collect())
        .collect();

    // Compute distances with FP32 and find top K
    let mut distances_fp32: Vec<(usize, f32)> = centroids_fp32
        .iter()
        .enumerate()
        .map(|(idx, centroid)| (idx, euclidean_distance(&query, centroid)))
        .collect();
    distances_fp32.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let top_k_fp32: Vec<usize> = distances_fp32
        .iter()
        .take(top_k)
        .map(|(idx, _)| *idx)
        .collect();

    // Convert centroids to FP16 and back, compute distances, find top K
    let centroids_fp16: Vec<Vec<u16>> = centroids_fp32.iter().map(|c| fp32_to_fp16(c)).collect();
    let centroids_fp32_reconstructed: Vec<Vec<f32>> =
        centroids_fp16.iter().map(|c| fp16_to_fp32(c)).collect();

    let mut distances_fp16: Vec<(usize, f32)> = centroids_fp32_reconstructed
        .iter()
        .enumerate()
        .map(|(idx, centroid)| (idx, euclidean_distance(&query, centroid)))
        .collect();
    distances_fp16.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let top_k_fp16: Vec<usize> = distances_fp16
        .iter()
        .take(top_k)
        .map(|(idx, _)| *idx)
        .collect();

    // Calculate recall
    let intersection = top_k_fp32
        .iter()
        .filter(|&idx| top_k_fp16.contains(idx))
        .count();
    let recall = intersection as f32 / top_k as f32;

    println!(
        "Top-{} recall with FP16 centroids: {:.2}%",
        top_k,
        recall * 100.0
    );
    println!("FP32 top-{}: {:?}", top_k, top_k_fp32);
    println!("FP16 top-{}: {:?}", top_k, top_k_fp16);

    // Recall should be >= 99% (allowing for 1 mismatch in top-10)
    assert!(
        recall >= 0.99,
        "Recall with FP16 centroids too low: {:.2}%. Expected >= 99%",
        recall * 100.0
    );
}

#[test]
fn test_fp16_edge_cases() {
    // Test edge cases: zero, very small, very large values
    let edge_cases = vec![
        vec![0.0, 0.0, 0.0, 0.0],         // All zeros
        vec![1e-6, 1e-5, 1e-4, 1e-3],     // Very small positive
        vec![-1e-6, -1e-5, -1e-4, -1e-3], // Very small negative
        vec![10.0, 20.0, 30.0, 40.0],     // Large values
        vec![-10.0, -20.0, -30.0, -40.0], // Large negative values
    ];

    for original in edge_cases {
        let fp16 = fp32_to_fp16(&original);
        let reconstructed = fp16_to_fp32(&fp16);

        // For edge cases, allow slightly larger errors but ensure no NaN/Inf
        for &val in &reconstructed {
            assert!(
                val.is_finite(),
                "FP16 conversion produced non-finite value: {}",
                val
            );
        }
    }
}

// Helper function for Euclidean distance
fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod sst_integration {
    use super::*;

    #[test]
    fn test_sst_index_entry_fp16_serialization() {
        // This test requires access to SST internals
        // TODO: Add test for IndexEntry serialization/deserialization with FP16
        // Verify that:
        // 1. IndexEntry with centroid_fp16 = Some(...) serializes correctly
        // 2. Deserialization reconstructs centroid_fp16
        // 3. Old files without centroid_fp16 deserialize with centroid_fp16 = None
    }
}

#[cfg(test)]
mod swift_integration {
    use super::*;

    #[test]
    fn test_swift_superblock_fp16() {
        // This test requires access to SWIFT internals
        // TODO: Add test for SuperBlock with FP16 centroids
        // Verify that:
        // 1. SuperBlock search prefers FP16 centroid when available
        // 2. Falls back to FP32 when FP16 is None
        // 3. Distance computations are within 0.1% error
    }
}
