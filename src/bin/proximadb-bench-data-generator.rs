use rand::{thread_rng, Rng};
use std::f32::consts::PI;

/// Different types of realistic vector data patterns
#[derive(Debug, Clone, Copy)]
pub enum VectorPattern {
    /// Random uniform distribution (current implementation - worst case)
    RandomUniform,
    /// Normalized vectors (common in ML - unit vectors)
    Normalized,
    /// Sparse vectors (many zeros, common in NLP)
    Sparse { sparsity: f32 },
    /// Gaussian/Normal distribution (common in embeddings)
    Gaussian { mean: f32, std_dev: f32 },
    /// Quantized vectors (discrete levels, common after quantization)
    Quantized { levels: usize },
    /// Structured patterns (sine waves, common in signal processing)
    Sinusoidal,
    /// Mixed patterns (realistic embeddings combine multiple patterns)
    Mixed,
    /// Constant regions (common in CNN features)
    ConstantRegions,
    /// Sequential/incremental (common in time series)
    Sequential,
}

/// Generate vectors with specified pattern
pub fn generate_patterned_vectors(
    num_vectors: usize,
    dimension: usize,
    pattern: VectorPattern,
) -> Vec<Vec<f32>> {
    let mut rng = thread_rng();

    match pattern {
        VectorPattern::RandomUniform => {
            // Current implementation - pure random noise (worst case for compression)
            (0..num_vectors)
                .map(|_| {
                    (0..dimension)
                        .map(|_| rng.gen_range(-1.0..1.0))
                        .collect()
                })
                .collect()
        }

        VectorPattern::Normalized => {
            // Unit vectors - common in cosine similarity use cases
            (0..num_vectors)
                .map(|_| {
                    let mut vec: Vec<f32> = (0..dimension)
                        .map(|_| rng.gen_range(-1.0..1.0))
                        .collect();

                    // Normalize to unit length
                    let magnitude = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                    if magnitude > 0.0 {
                        vec.iter_mut().for_each(|x| *x /= magnitude);
                    }
                    vec
                })
                .collect()
        }

        VectorPattern::Sparse { sparsity } => {
            // Sparse vectors with controlled sparsity (common in NLP/recommendation)
            (0..num_vectors)
                .map(|_| {
                    (0..dimension)
                        .map(|_| {
                            if rng.gen_bool(sparsity as f64) {
                                0.0
                            } else {
                                rng.gen_range(-1.0..1.0)
                            }
                        })
                        .collect()
                })
                .collect()
        }

        VectorPattern::Gaussian { mean, std_dev } => {
            // Gaussian distribution (common in neural network embeddings)
            // Use Box-Muller transform instead of rand_distr
            (0..num_vectors)
                .map(|_| {
                    (0..dimension)
                        .map(|_| {
                            let u1: f32 = rng.gen_range(0.001..1.0);
                            let u2: f32 = rng.gen_range(0.0..1.0);
                            let z0 = ((-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos());
                            mean + std_dev * z0
                        })
                        .collect()
                })
                .collect()
        }

        VectorPattern::Quantized { levels } => {
            // Discrete levels (common after quantization)
            let step = 2.0 / levels as f32;
            (0..num_vectors)
                .map(|_| {
                    (0..dimension)
                        .map(|_| {
                            let level = rng.gen_range(0..levels);
                            -1.0 + level as f32 * step
                        })
                        .collect()
                })
                .collect()
        }

        VectorPattern::Sinusoidal => {
            // Structured sine wave patterns (common in Fourier features)
            (0..num_vectors)
                .map(|v| {
                    let phase = v as f32 * 0.1;
                    let frequency = 2.0 * PI / 32.0; // Period of 32 dimensions
                    (0..dimension)
                        .map(|d| {
                            ((d as f32 * frequency + phase).sin() * 0.5
                             + rng.gen_range(-0.1..0.1)) // Add small noise
                        })
                        .collect()
                })
                .collect()
        }

        VectorPattern::Mixed => {
            // Realistic mix of patterns (most representative of real embeddings)
            (0..num_vectors)
                .map(|v| {
                    let mut vec = Vec::with_capacity(dimension);
                    let chunk_size = dimension / 4;

                    // First quarter: sparse
                    for _ in 0..chunk_size {
                        vec.push(if rng.gen_bool(0.7) { 0.0 } else { rng.gen_range(-1.0..1.0) });
                    }

                    // Second quarter: gaussian
                    // Box-Muller transform for gaussian distribution
                    for _ in 0..chunk_size {
                        let u1: f32 = rng.gen_range(0.001..1.0);
                        let u2: f32 = rng.gen_range(0.0..1.0);
                        let gaussian = ((-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos()) * 0.3f32;
                        vec.push(gaussian.clamp(-1.0, 1.0));
                    }

                    // Third quarter: quantized
                    for _ in 0..chunk_size {
                        let level = rng.gen_range(0..8);
                        vec.push(-1.0 + level as f32 * 0.25);
                    }

                    // Fourth quarter: sinusoidal
                    let phase = v as f32 * 0.1;
                    for d in 0..(dimension - 3 * chunk_size) {
                        vec.push((d as f32 * 0.2 + phase).sin() * 0.5);
                    }

                    vec
                })
                .collect()
        }

        VectorPattern::ConstantRegions => {
            // Vectors with constant regions (common in CNN features)
            (0..num_vectors)
                .map(|_| {
                    let mut vec = Vec::with_capacity(dimension);
                    let num_regions = 8;
                    let region_size = dimension / num_regions;

                    for _ in 0..num_regions {
                        let value = rng.gen_range(-1.0..1.0);
                        for _ in 0..region_size {
                            vec.push(value + rng.gen_range(-0.05..0.05)); // Small variation
                        }
                    }

                    // Fill remainder
                    while vec.len() < dimension {
                        vec.push(rng.gen_range(-1.0..1.0));
                    }

                    vec
                })
                .collect()
        }

        VectorPattern::Sequential => {
            // Sequential/incremental patterns (time series, positional encodings)
            (0..num_vectors)
                .map(|v| {
                    let base = (v as f32) * 0.01;
                    (0..dimension)
                        .map(|d| {
                            let value = base + (d as f32) * 0.001;
                            (value % 2.0 - 1.0) // Wrap to [-1, 1]
                        })
                        .collect()
                })
                .collect()
        }
    }
}

/// Analyze the compressibility of generated vectors
pub fn analyze_compressibility(vectors: &[Vec<f32>]) -> String {
    if vectors.is_empty() || vectors[0].is_empty() {
        return "Empty vectors".to_string();
    }

    let dimension = vectors[0].len();
    let total_values = vectors.len() * dimension;

    // Count zeros (sparsity)
    let zeros: usize = vectors.iter()
        .flat_map(|v| v.iter())
        .filter(|&&x| x.abs() < 1e-6)
        .count();

    // Check for repeated values
    let mut value_counts = std::collections::HashMap::new();
    for v in vectors.iter().flat_map(|v| v.iter()) {
        let quantized = (*v * 1000.0).round() as i32; // Quantize to 3 decimal places
        *value_counts.entry(quantized).or_insert(0) += 1;
    }

    let unique_values = value_counts.len();
    let most_common = value_counts.values().max().copied().unwrap_or(0);

    // Check for sequential patterns
    let mut deltas = Vec::new();
    for vec in vectors.iter() {
        for window in vec.windows(2) {
            deltas.push(window[1] - window[0]);
        }
    }

    let avg_delta = deltas.iter().sum::<f32>() / deltas.len() as f32;
    let delta_variance = deltas.iter()
        .map(|d| (d - avg_delta).powi(2))
        .sum::<f32>() / deltas.len() as f32;

    format!(
        "Sparsity: {:.1}%, Unique values: {} ({:.1}%), Most common: {:.1}%, Delta variance: {:.6}",
        (zeros as f32 / total_values as f32) * 100.0,
        unique_values,
        (unique_values as f32 / total_values as f32) * 100.0,
        (most_common as f32 / total_values as f32) * 100.0,
        delta_variance
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_generation() {
        let patterns = [
            VectorPattern::RandomUniform,
            VectorPattern::Normalized,
            VectorPattern::Sparse { sparsity: 0.8 },
            VectorPattern::Gaussian { mean: 0.0, std_dev: 0.3 },
            VectorPattern::Quantized { levels: 16 },
            VectorPattern::Sinusoidal,
            VectorPattern::Mixed,
            VectorPattern::ConstantRegions,
            VectorPattern::Sequential,
        ];

        for pattern in patterns {
            let vectors = generate_patterned_vectors(10, 128, pattern);
            assert_eq!(vectors.len(), 10);
            assert_eq!(vectors[0].len(), 128);

            println!("{:?}: {}", pattern, analyze_compressibility(&vectors));
        }
    }
}

fn main() {
    println!("ProximaDB Data Generator - Testing Various Patterns\n");

    let patterns = [
        VectorPattern::RandomUniform,
        VectorPattern::Normalized,
        VectorPattern::Sparse { sparsity: 0.8 },
        VectorPattern::Gaussian { mean: 0.0, std_dev: 0.3 },
        VectorPattern::Quantized { levels: 16 },
        VectorPattern::Sinusoidal,
        VectorPattern::Mixed,
        VectorPattern::ConstantRegions,
        VectorPattern::Sequential,
    ];

    println!("Generating test vectors with different patterns:");
    println!("{}", "=".repeat(60));

    for pattern in patterns {
        let vectors = generate_patterned_vectors(100, 384, pattern);
        let compressibility = analyze_compressibility(&vectors);
        println!("{:20?} -> {}", pattern, compressibility);
    }

    println!("\nData generation complete.");
}