//! Realistic embedding generators for benchmarking
//!
//! This module provides generators that create vectors with distributions
//! matching real embedding models like BERT and OpenAI's text-embedding-ada-002.

use rand::prelude::*;

/// Embedding model types with realistic value distributions
#[derive(Debug, Clone, Copy)]
pub enum EmbeddingModel {
    /// BERT embeddings: normally distributed around 0 with std ~1.5, range typically [-5, 5]
    Bert,
    /// OpenAI text-embedding-ada-002: mostly positive values, slight negative tail
    OpenAIAda,
    /// Generic normalized embeddings: unit norm (L2 = 1)
    Normalized,
    /// Random uniform distribution (less realistic, for compatibility)
    RandomUniform,
}

/// Generate realistic embeddings based on model characteristics
pub struct EmbeddingGenerator {
    model: EmbeddingModel,
    rng: ThreadRng,
}

impl EmbeddingGenerator {
    pub fn new(model: EmbeddingModel) -> Self {
        Self {
            model,
            rng: thread_rng(),
        }
    }

    /// Generate a single embedding vector
    pub fn generate(&mut self, dimension: usize) -> Vec<f32> {
        match self.model {
            EmbeddingModel::Bert => self.generate_bert_embedding(dimension),
            EmbeddingModel::OpenAIAda => self.generate_openai_embedding(dimension),
            EmbeddingModel::Normalized => self.generate_normalized_embedding(dimension),
            EmbeddingModel::RandomUniform => self.generate_random_uniform(dimension),
        }
    }

    /// Generate multiple embedding vectors
    pub fn generate_batch(&mut self, count: usize, dimension: usize) -> Vec<Vec<f32>> {
        (0..count).map(|_| self.generate(dimension)).collect()
    }

    /// BERT-style embeddings: Normal distribution with mean=0, std=1.5
    /// Typical range: [-5, 5] with most values in [-3, 3]
    fn generate_bert_embedding(&mut self, dimension: usize) -> Vec<f32> {
        let mut embedding: Vec<f32> = (0..dimension)
            .map(|_| {
                // Box-Muller transform for normal distribution
                let u1: f32 = self.rng.gen_range(0.0001..1.0);
                let u2: f32 = self.rng.gen_range(0.0..1.0);
                let val =
                    (-2.0f32 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos() * 1.5;
                // Soft clamp to [-5, 5] range (rare values might exceed)
                val.max(-5.0).min(5.0)
            })
            .collect();

        // BERT embeddings often have some structure - add slight correlations
        // Every 8th dimension has slight positive correlation with neighbors
        for i in (0..dimension).step_by(8) {
            if i + 1 < dimension {
                embedding[i + 1] += embedding[i] * 0.2;
            }
        }

        embedding
    }

    /// OpenAI Ada embeddings: Mostly positive with slight negative tail
    /// Uses a shifted and skewed distribution
    fn generate_openai_embedding(&mut self, dimension: usize) -> Vec<f32> {
        let mut embedding = Vec::with_capacity(dimension);

        for _ in 0..dimension {
            // Generate from normal distribution using Box-Muller transform
            let u1: f32 = self.rng.gen_range(0.0001..1.0);
            let u2: f32 = self.rng.gen_range(0.0..1.0);
            let normal_val: f32 =
                (-2.0f32 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();

            // Apply transformation to create OpenAI-like distribution:
            // 1. Shift mean to positive (around 0.3)
            // 2. Add slight positive skew
            // 3. Allow small negative values (about 10% of values)
            let transformed = if normal_val < -1.5 {
                // Small negative tail (about 7% of values)
                normal_val * 0.2
            } else {
                // Mostly positive values
                (normal_val * 0.8 + 0.3).abs() * (1.0 + normal_val * 0.1)
            };

            embedding.push(transformed);
        }

        // OpenAI embeddings often have high-magnitude components in specific dimensions
        // Randomly boost ~5% of dimensions
        for i in 0..dimension {
            if self.rng.gen_range(0.0..1.0) < 0.05 {
                embedding[i] *= 2.5;
            }
        }

        embedding
    }

    /// Generate normalized embeddings with unit L2 norm
    fn generate_normalized_embedding(&mut self, dimension: usize) -> Vec<f32> {
        // Start with normal distribution using Box-Muller transform
        let mut embedding: Vec<f32> = (0..dimension)
            .map(|_| {
                let u1: f32 = self.rng.gen_range(0.0001..1.0);
                let u2: f32 = self.rng.gen_range(0.0..1.0);
                (-2.0f32 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
            })
            .collect();

        // Normalize to unit length
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut embedding {
                *val /= norm;
            }
        }

        embedding
    }

    /// Legacy random uniform distribution for backwards compatibility
    fn generate_random_uniform(&mut self, dimension: usize) -> Vec<f32> {
        (0..dimension)
            .map(|_| self.rng.gen_range(-1.0..1.0))
            .collect()
    }
}

/// Generate dataset with sparse embeddings
pub fn generate_sparse_embeddings(
    model: EmbeddingModel,
    count: usize,
    dimension: usize,
    sparsity_percent: usize,
) -> Vec<Vec<f32>> {
    let mut generator = EmbeddingGenerator::new(model);
    let mut embeddings = generator.generate_batch(count, dimension);

    // Apply sparsity by zeroing out random dimensions
    let zero_count = (dimension * sparsity_percent) / 100;
    let mut rng = thread_rng();

    for embedding in &mut embeddings {
        let mut indices: Vec<usize> = (0..dimension).collect();
        indices.shuffle(&mut rng);

        for &idx in indices.iter().take(zero_count) {
            embedding[idx] = 0.0;
        }
    }

    embeddings
}

/// Statistics about generated embeddings (for verification)
pub struct EmbeddingStats {
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    pub std_dev: f32,
    pub zero_count: usize,
    pub negative_count: usize,
}

impl EmbeddingStats {
    pub fn from_embedding(embedding: &[f32]) -> Self {
        let min = embedding.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max = embedding.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let mean = embedding.iter().sum::<f32>() / embedding.len() as f32;

        let variance = embedding
            .iter()
            .map(|&x| {
                let diff = x - mean;
                diff * diff
            })
            .sum::<f32>()
            / embedding.len() as f32;

        let std_dev = variance.sqrt();
        let zero_count = embedding.iter().filter(|&&x| x == 0.0).count();
        let negative_count = embedding.iter().filter(|&&x| x < 0.0).count();

        Self {
            min,
            max,
            mean,
            std_dev,
            zero_count,
            negative_count,
        }
    }

    pub fn print_summary(&self, model_name: &str) {
        println!("=== {} Embedding Stats ===", model_name);
        println!("Range: [{:.3}, {:.3}]", self.min, self.max);
        println!("Mean: {:.3}, Std Dev: {:.3}", self.mean, self.std_dev);
        println!(
            "Zero values: {} | Negative values: {}",
            self.zero_count, self.negative_count
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bert_embedding_characteristics() {
        let mut generator = EmbeddingGenerator::new(EmbeddingModel::Bert);
        let embedding = generator.generate(768);
        let stats = EmbeddingStats::from_embedding(&embedding);

        // BERT embeddings should be roughly centered around 0
        assert!(stats.mean.abs() < 0.5, "BERT mean should be near 0");
        // Most values should be in [-5, 5]
        assert!(
            stats.min >= -6.0 && stats.max <= 6.0,
            "BERT range should be roughly [-5, 5]"
        );
        // Should have both positive and negative values
        assert!(stats.negative_count > 0, "BERT should have negative values");
    }

    #[test]
    fn test_openai_embedding_characteristics() {
        let mut generator = EmbeddingGenerator::new(EmbeddingModel::OpenAIAda);
        let embedding = generator.generate(1536);
        let stats = EmbeddingStats::from_embedding(&embedding);

        // OpenAI embeddings are mostly positive
        assert!(stats.mean > 0.0, "OpenAI mean should be positive");
        // Should have few negative values
        let negative_ratio = stats.negative_count as f32 / embedding.len() as f32;
        assert!(
            negative_ratio < 0.2,
            "OpenAI should have <20% negative values"
        );
    }

    #[test]
    fn test_normalized_embedding_characteristics() {
        let mut generator = EmbeddingGenerator::new(EmbeddingModel::Normalized);
        let embedding = generator.generate(512);

        // Check L2 norm is approximately 1
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 0.01,
            "Normalized embedding should have unit norm"
        );
    }

    #[test]
    fn test_sparse_embeddings() {
        let embeddings = generate_sparse_embeddings(
            EmbeddingModel::Bert,
            10,
            100,
            50, // 50% sparsity
        );

        for embedding in &embeddings {
            let stats = EmbeddingStats::from_embedding(embedding);
            // Should have approximately 50% zeros
            let zero_ratio = stats.zero_count as f32 / embedding.len() as f32;
            assert!(
                zero_ratio > 0.4 && zero_ratio < 0.6,
                "Should have ~50% sparsity"
            );
        }
    }
}
