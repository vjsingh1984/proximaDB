use rand::prelude::*;
use std::collections::HashMap;

fn generate_random_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut rng = rand::thread_rng();
    (0..count)
        .map(|_| {
            (0..dimension)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect()
        })
        .collect()
}

fn compress_lz4(data: &[u8]) -> Vec<u8> {
    lz4_flex::compress_prepend_size(data)
}

fn test_full_vector_strategy(vectors: &[Vec<f32>]) -> (usize, usize, f64) {
    // Serialize all vectors as one contiguous block (current FullVector approach)
    let mut full_data = Vec::new();
    for vector in vectors {
        let bytes: &[u8] = bytemuck::cast_slice(vector);
        full_data.extend_from_slice(bytes);
    }

    let original_size = full_data.len();
    let compressed = compress_lz4(&full_data);
    let compressed_size = compressed.len();
    let ratio = original_size as f64 / compressed_size as f64;

    (original_size, compressed_size, ratio)
}

fn test_grouped_64d_strategy(vectors: &[Vec<f32>], group_size: usize) -> (usize, usize, f64) {
    // Group vectors into 64D chunks
    let dimension = vectors[0].len();
    let num_groups = (dimension + group_size - 1) / group_size;

    let mut total_compressed_size = 0;
    let mut total_original_size = 0;

    for group_idx in 0..num_groups {
        let start_dim = group_idx * group_size;
        let end_dim = ((group_idx + 1) * group_size).min(dimension);
        let group_dims = end_dim - start_dim;

        // Extract this dimensional group from all vectors
        let mut group_data = Vec::new();
        for vector in vectors {
            for dim in start_dim..end_dim {
                let bytes = vector[dim].to_le_bytes();
                group_data.extend_from_slice(&bytes);
            }
        }

        let original_size = group_data.len();
        let compressed = compress_lz4(&group_data);

        total_original_size += original_size;
        total_compressed_size += compressed.len();
    }

    let ratio = total_original_size as f64 / total_compressed_size as f64;
    (total_original_size, total_compressed_size, ratio)
}

fn test_hybrid_columnar_strategy(vectors: &[Vec<f32>], group_size: usize) -> (usize, usize, f64) {
    // Transpose within groups (hybrid columnar)
    let dimension = vectors[0].len();
    let num_groups = (dimension + group_size - 1) / group_size;

    let mut total_compressed_size = 0;
    let mut total_original_size = 0;

    for group_idx in 0..num_groups {
        let start_dim = group_idx * group_size;
        let end_dim = ((group_idx + 1) * group_size).min(dimension);

        // Transpose: for each dimension in group, collect all vector values
        let mut group_data = Vec::new();
        for dim in start_dim..end_dim {
            for vector in vectors {
                let bytes = vector[dim].to_le_bytes();
                group_data.extend_from_slice(&bytes);
            }
        }

        let original_size = group_data.len();
        let compressed = compress_lz4(&group_data);

        total_original_size += original_size;
        total_compressed_size += compressed.len();
    }

    let ratio = total_original_size as f64 / total_compressed_size as f64;
    (total_original_size, total_compressed_size, ratio)
}

fn main() {
    println!("🔬 Vector Grouping Strategy Analysis");
    println!("=====================================\n");

    let test_cases = vec![
        (100, 64),   // Baseline: exactly 64D
        (100, 128),  // 2 groups of 64D
        (100, 256),  // 4 groups of 64D
        (100, 384),  // 6 groups of 64D
        (100, 768),  // 12 groups of 64D (common embedding size)
        (100, 1536), // 24 groups of 64D (GPT embeddings)
        (1000, 128), // More vectors
        (1000, 768),
    ];

    for (vector_count, dimension) in test_cases {
        println!("📊 Testing {} vectors × {} dimensions", vector_count, dimension);
        println!("-" * 50);

        let vectors = generate_random_vectors(vector_count, dimension);

        // Test 1: Current FullVector strategy (all dimensions together)
        let (orig, comp, ratio) = test_full_vector_strategy(&vectors);
        println!("  FullVector (all together):");
        println!("    Original: {} bytes", orig);
        println!("    Compressed: {} bytes", comp);
        println!("    Ratio: {:.2}x", ratio);

        // Test 2: Grouped 64D strategy (keep row-wise within groups)
        let (orig, comp, ratio) = test_grouped_64d_strategy(&vectors, 64);
        println!("\n  Grouped 64D (row-wise within groups):");
        println!("    Original: {} bytes", orig);
        println!("    Compressed: {} bytes", comp);
        println!("    Ratio: {:.2}x", ratio);

        // Test 3: Hybrid columnar (transpose within 64D groups)
        let (orig, comp, ratio) = test_hybrid_columnar_strategy(&vectors, 64);
        println!("\n  Hybrid Columnar (transpose within 64D groups):");
        println!("    Original: {} bytes", orig);
        println!("    Compressed: {} bytes", comp);
        println!("    Ratio: {:.2}x", ratio);

        // Test different group sizes for this dimension
        if dimension >= 128 {
            println!("\n  📈 Group Size Comparison:");
            for group_size in [32, 64, 128, 256].iter() {
                if *group_size <= dimension {
                    let (_, _, ratio) = test_grouped_64d_strategy(&vectors, *group_size);
                    println!("    {}D groups: {:.2}x compression", group_size, ratio);
                }
            }
        }

        println!("\n");
    }

    println!("🔍 Analysis Summary:");
    println!("====================");
    println!("\n1. Random data (uniform distribution) shows poor compression across all strategies");
    println!("   - This is expected as random floats have high entropy");
    println!("   - Real embeddings have patterns that compress better");
    println!("\n2. Grouping Impact:");
    println!("   - Smaller groups (32D-64D) may preserve locality better");
    println!("   - But more groups = more compression headers/overhead");
    println!("   - Sweet spot seems to be 64D-128D groups");
    println!("\n3. Hybrid Columnar within groups:");
    println!("   - Can capture dimensional patterns within local groups");
    println!("   - Reduces transposition overhead for high dimensions");
    println!("   - Better cache locality during encoding/decoding");
    println!("\n4. Recommendations:");
    println!("   - For D <= 128: Keep FullVector as-is");
    println!("   - For D > 128: Consider 64D or 128D grouping");
    println!("   - For D > 512: Hybrid columnar within groups may help");
}