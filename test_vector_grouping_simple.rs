fn generate_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    // Generate simple test patterns instead of random
    (0..count)
        .map(|i| {
            (0..dimension)
                .map(|d| {
                    // Create patterns: gradients, waves, and some noise
                    let gradient = (d as f32) / (dimension as f32);
                    let wave = ((d as f32 * 0.1).sin() + 1.0) / 2.0;
                    let pattern = (i as f32 * 0.01).cos();
                    (gradient * 0.3 + wave * 0.4 + pattern * 0.3)
                })
                .collect()
        })
        .collect()
}

fn estimate_entropy(data: &[f32]) -> f32 {
    // Simple entropy estimate based on value distribution
    let mut buckets = vec![0u32; 256];
    for &val in data {
        let bucket = ((val.clamp(0.0, 1.0) * 255.0) as usize).min(255);
        buckets[bucket] += 1;
    }

    let total = data.len() as f32;
    let mut entropy = 0.0;
    for count in buckets {
        if count > 0 {
            let p = count as f32 / total;
            entropy -= p * p.log2();
        }
    }
    entropy
}

fn analyze_full_vector(vectors: &[Vec<f32>]) -> (usize, f32, String) {
    let mut all_data = Vec::new();
    for vector in vectors {
        all_data.extend_from_slice(vector);
    }

    let size = all_data.len() * 4; // f32 = 4 bytes
    let entropy = estimate_entropy(&all_data);

    (size, entropy, "All dimensions together".to_string())
}

fn analyze_grouped_64d(vectors: &[Vec<f32>], group_size: usize) -> (usize, f32, String) {
    let dimension = vectors[0].len();
    let num_groups = (dimension + group_size - 1) / group_size;

    let mut total_entropy = 0.0;
    let mut total_size = 0;

    for group_idx in 0..num_groups {
        let start_dim = group_idx * group_size;
        let end_dim = ((group_idx + 1) * group_size).min(dimension);

        let mut group_data = Vec::new();
        for vector in vectors {
            for dim in start_dim..end_dim {
                group_data.push(vector[dim]);
            }
        }

        total_size += group_data.len() * 4;
        total_entropy += estimate_entropy(&group_data);
    }

    let avg_entropy = total_entropy / num_groups as f32;

    (total_size, avg_entropy, format!("{}D groups (row-wise within)", group_size))
}

fn analyze_hybrid_columnar(vectors: &[Vec<f32>], group_size: usize) -> (usize, f32, String) {
    let dimension = vectors[0].len();
    let num_groups = (dimension + group_size - 1) / group_size;

    let mut total_entropy = 0.0;
    let mut total_size = 0;

    for group_idx in 0..num_groups {
        let start_dim = group_idx * group_size;
        let end_dim = ((group_idx + 1) * group_size).min(dimension);

        // Transpose within group
        let mut group_data = Vec::new();
        for dim in start_dim..end_dim {
            for vector in vectors {
                group_data.push(vector[dim]);
            }
        }

        total_size += group_data.len() * 4;
        total_entropy += estimate_entropy(&group_data);
    }

    let avg_entropy = total_entropy / num_groups as f32;

    (total_size, avg_entropy, format!("{}D groups (columnar within)", group_size))
}

fn main() {
    println!("🔬 Vector Grouping Strategy Analysis");
    println!("=====================================\n");
    println!("Note: Lower entropy = better compressibility\n");

    let test_cases = vec![
        (100, 64),
        (100, 128),
        (100, 256),
        (100, 384),
        (100, 768),
        (100, 1536),
        (500, 768),
    ];

    for (vector_count, dimension) in test_cases {
        println!("📊 Testing {} vectors × {} dimensions", vector_count, dimension);
        println!("{}", "-".repeat(50));

        let vectors = generate_test_vectors(vector_count, dimension);

        // Test different strategies
        let strategies = vec![
            analyze_full_vector(&vectors),
            analyze_grouped_64d(&vectors, 64),
            analyze_grouped_64d(&vectors, 128),
            analyze_hybrid_columnar(&vectors, 64),
            analyze_hybrid_columnar(&vectors, 128),
        ];

        for (size, entropy, strategy) in strategies {
            println!("  {}:", strategy);
            println!("    Size: {} KB", size / 1024);
            println!("    Entropy: {:.3} bits", entropy);
            println!("    Est. compression: {:.1}x", 8.0 / entropy.max(0.1));
        }

        // Analyze locality
        println!("\n  🔍 Locality Analysis:");

        // Check correlation between adjacent dimensions
        let mut local_correlation = 0.0;
        for vector in &vectors[..10.min(vectors.len())] {
            for i in 0..vector.len()-1 {
                let diff = (vector[i] - vector[i+1]).abs();
                local_correlation += 1.0 - diff.min(1.0);
            }
        }
        local_correlation /= (10.0 * (dimension - 1) as f32);
        println!("    Adjacent dimension correlation: {:.3}", local_correlation);

        // Check correlation within 64D windows
        let mut window_correlation = 0.0;
        let window_size = 64;
        for vector in &vectors[..10.min(vectors.len())] {
            for start in (0..vector.len()).step_by(window_size) {
                let end = (start + window_size).min(vector.len());
                if end - start > 1 {
                    let window = &vector[start..end];
                    let mean: f32 = window.iter().sum::<f32>() / window.len() as f32;
                    let variance: f32 = window.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / window.len() as f32;
                    window_correlation += 1.0 / (1.0 + variance);
                }
            }
        }
        window_correlation /= (10.0 * ((dimension + window_size - 1) / window_size) as f32);
        println!("    64D window coherence: {:.3}", window_correlation);

        println!("\n");
    }

    println!("🎯 Key Insights:");
    println!("================");
    println!("
1. ENTROPY ANALYSIS:
   - Lower entropy = better compression potential
   - Grouping can reduce average entropy by processing similar values together

2. LOCALITY BENEFITS:
   - Adjacent dimensions often have correlation (gradient patterns)
   - 64D windows can capture local patterns in embeddings
   - Transposing within groups preserves dimensional patterns

3. PRACTICAL CONSIDERATIONS:

   For FullVector with 64D grouping:

   PROS:
   ✓ Better cache locality (64 * 4 bytes = 256 bytes fits in cache lines)
   ✓ Can mix strategies per group (adaptive compression)
   ✓ Parallel processing of groups
   ✓ Reduced memory footprint during encoding

   CONS:
   ✗ More metadata overhead (group boundaries)
   ✗ Potential loss of cross-group patterns
   ✗ Complexity in implementation

4. RECOMMENDATIONS:

   • D ≤ 128: Keep as single block (current FullVector)
   • 128 < D ≤ 512: Consider 64D or 128D groups
   • D > 512: Hybrid columnar within groups likely beneficial
   • D > 1024: Definitely use grouping to avoid memory pressure

5. REAL-WORLD CONSIDERATIONS:
   - Real embeddings have more structure than test data
   - Model-specific patterns (e.g., BERT layers, GPT attention)
   - Quantization can work better with grouped approaches
");
}