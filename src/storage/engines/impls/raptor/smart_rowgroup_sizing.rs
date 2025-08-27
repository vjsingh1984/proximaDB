// Smart Row Group Sizing for RAPTOR Engine
// Optimizes row group size based on vector dimensions, metadata cost, and cloud I/O characteristics

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Cloud storage I/O characteristics for optimal row group sizing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudIOProfile {
    /// Optimal I/O size for this storage backend (1MB-4MB for S3/GCS/ADLS)
    pub optimal_io_size_bytes: usize,
    /// Maximum IOPS for sequential reads
    pub max_sequential_iops: u32,
    /// Latency per I/O operation in microseconds
    pub latency_per_io_us: u32,
    /// Storage tier (Hot/Warm/Cold affects I/O patterns)
    pub storage_tier: StorageTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageTier {
    Hot,    // S3 Standard, GCS Standard, ADLS Hot
    Warm,   // S3 IA, GCS Nearline, ADLS Cool
    Cold,   // S3 Glacier IR, GCS Coldline, ADLS Archive
}

impl Default for CloudIOProfile {
    fn default() -> Self {
        // S3 Standard defaults - most common case
        Self {
            optimal_io_size_bytes: 2 * 1024 * 1024, // 2MB - sweet spot for S3
            max_sequential_iops: 5500, // S3 Standard throughput
            latency_per_io_us: 20_000, // ~20ms typical S3 latency
            storage_tier: StorageTier::Hot,
        }
    }
}

impl CloudIOProfile {
    /// S3 Standard profile (most common)
    pub fn s3_standard() -> Self {
        Self {
            optimal_io_size_bytes: 2 * 1024 * 1024, // 2MB
            max_sequential_iops: 5500,
            latency_per_io_us: 20_000,
            storage_tier: StorageTier::Hot,
        }
    }
    
    /// GCS Standard profile
    pub fn gcs_standard() -> Self {
        Self {
            optimal_io_size_bytes: 4 * 1024 * 1024, // 4MB - GCS prefers larger chunks
            max_sequential_iops: 10000,
            latency_per_io_us: 15_000, // ~15ms typical GCS latency
            storage_tier: StorageTier::Hot,
        }
    }
    
    /// Azure Data Lake Storage (ADLS) Gen2 profile
    pub fn adls_gen2() -> Self {
        Self {
            optimal_io_size_bytes: 2 * 1024 * 1024, // 2MB
            max_sequential_iops: 8000,
            latency_per_io_us: 25_000, // ~25ms typical ADLS latency
            storage_tier: StorageTier::Hot,
        }
    }
    
    /// Local NVMe profile for comparison
    pub fn local_nvme() -> Self {
        Self {
            optimal_io_size_bytes: 64 * 1024, // 64KB - much smaller chunks optimal
            max_sequential_iops: 500_000,
            latency_per_io_us: 100, // ~0.1ms NVMe latency
            storage_tier: StorageTier::Hot,
        }
    }
}

/// Smart row group sizing calculator
#[derive(Debug, Clone)]
pub struct SmartRowGroupSizer {
    /// Cloud I/O profile for this deployment
    pub io_profile: CloudIOProfile,
    /// Vector dimension
    pub vector_dimension: usize,
    /// Average metadata size per vector (bytes)
    pub avg_metadata_bytes: usize,
    /// Matrix Trinity overhead per vector (P² + P×K contribution)
    pub matrix_overhead_bytes: usize,
    /// Quantization enabled and level
    pub quantization_config: Option<QuantizationConfig>,
    /// Target query pattern (affects sizing strategy)
    pub query_pattern: QueryPattern,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationConfig {
    /// Primary quantization level (Binary/INT8/PQ4/PQ8)
    pub primary_level: QuantizationLevel,
    /// Whether FP32 is also stored (dual storage)
    pub store_fp32: bool,
    /// Compression ratio achieved
    pub compression_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuantizationLevel {
    Binary,   // 1 bit per dimension
    INT8,     // 8 bits per dimension  
    PQ4,      // 4 bits per dimension
    PQ8,      // 8 bits per dimension
    FP32,     // 32 bits per dimension (unquantized)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryPattern {
    /// k < 10, need very fast response
    HighSelectivity,
    /// k = 10-100, balanced
    MediumSelectivity, 
    /// k > 100, throughput focused
    LowSelectivity,
    /// Mixed workload
    Mixed,
}

impl SmartRowGroupSizer {
    pub fn new(
        io_profile: CloudIOProfile,
        vector_dimension: usize,
        avg_metadata_bytes: usize,
    ) -> Self {
        Self {
            io_profile,
            vector_dimension,
            avg_metadata_bytes,
            matrix_overhead_bytes: Self::estimate_matrix_overhead(vector_dimension),
            quantization_config: None,
            query_pattern: QueryPattern::Mixed,
        }
    }
    
    /// Estimate Matrix Trinity overhead per vector
    fn estimate_matrix_overhead(_dimension: usize) -> usize {
        // Matrix Trinity overhead:
        // - P² matrix: ~1 byte per vector pair (INT8 quantized) / P vectors = ~1KB for P=1024
        // - P×K matrix: Selective storage based on K/D ratio (10-100% coverage)
        // Much more efficient than HNSW's graph edges!
        let p2_overhead = 1; // ~1 byte per vector for P² contribution
        let pxk_overhead = 4; // ~4 bytes average for P×K (selective storage)
        
        p2_overhead + pxk_overhead  // ~5 bytes total vs ~144 bytes for HNSW
    }
    
    /// Calculate optimal row group size in number of vectors
    pub fn calculate_optimal_rowgroup_size(&self) -> Result<OptimalRowGroupSize> {
        // Step 1: Calculate bytes per vector
        let vector_bytes = self.calculate_vector_storage_bytes();
        let total_bytes_per_vector = vector_bytes + self.avg_metadata_bytes + self.matrix_overhead_bytes;
        
        // Step 2: Calculate base row group size from I/O profile
        let base_vectors_per_rowgroup = self.io_profile.optimal_io_size_bytes / total_bytes_per_vector;
        
        // Step 3: Adjust based on query pattern
        let pattern_adjusted = self.adjust_for_query_pattern(base_vectors_per_rowgroup);
        
        // Step 4: Ensure minimum/maximum bounds
        let bounded_size = self.apply_bounds(pattern_adjusted);
        
        // Step 5: Calculate actual I/O size and efficiency
        let actual_io_size = bounded_size * total_bytes_per_vector;
        let io_efficiency = actual_io_size as f32 / self.io_profile.optimal_io_size_bytes as f32;
        
        Ok(OptimalRowGroupSize {
            vectors_per_rowgroup: bounded_size,
            bytes_per_vector: total_bytes_per_vector,
            total_rowgroup_bytes: actual_io_size,
            io_efficiency_ratio: io_efficiency,
            estimated_read_latency_ms: self.estimate_read_latency(actual_io_size),
            rationale: self.generate_rationale(bounded_size, actual_io_size),
        })
    }
    
    /// Calculate bytes needed to store one vector based on quantization
    fn calculate_vector_storage_bytes(&self) -> usize {
        let dimension = self.vector_dimension;
        
        match &self.quantization_config {
            None => {
                // FP32 storage
                dimension * 4
            }
            Some(config) => {
                let quantized_bytes = match config.primary_level {
                    QuantizationLevel::Binary => dimension / 8,
                    QuantizationLevel::INT8 => dimension,
                    QuantizationLevel::PQ4 => dimension / 2,
                    QuantizationLevel::PQ8 => dimension,
                    QuantizationLevel::FP32 => dimension * 4,
                };
                
                if config.store_fp32 {
                    // Dual storage: quantized + FP32
                    quantized_bytes + (dimension * 4)
                } else {
                    quantized_bytes
                }
            }
        }
    }
    
    /// Adjust row group size based on query patterns and semantic accuracy
    fn adjust_for_query_pattern(&self, base_size: usize) -> usize {
        // Step 1: Apply semantic accuracy factor based on vector dimension
        let semantic_factor = self.calculate_semantic_accuracy_factor();
        let semantic_adjusted = (base_size as f32 * semantic_factor) as usize;
        
        // Step 2: Apply query pattern adjustment
        match self.query_pattern {
            QueryPattern::HighSelectivity => {
                // k < 10: Smaller row groups to minimize waste
                (semantic_adjusted as f32 * 0.7) as usize
            }
            QueryPattern::MediumSelectivity => {
                // k = 10-100: Use semantic-adjusted size
                semantic_adjusted
            }
            QueryPattern::LowSelectivity => {
                // k > 100: Larger row groups for throughput, but still respect semantic accuracy
                (semantic_adjusted as f32 * 1.4) as usize
            }
            QueryPattern::Mixed => {
                // Balanced approach with semantic consideration
                semantic_adjusted
            }
        }
    }
    
    /// Calculate semantic accuracy factor - higher dimensions = smaller row groups for precision
    fn calculate_semantic_accuracy_factor(&self) -> f32 {
        // Higher dimensional vectors provide more accurate semantic distance
        // Therefore we can use smaller row groups for better precision/recall
        match self.vector_dimension {
            // Very low dimensions (e.g., word2vec): Need larger row groups for statistical significance
            d if d <= 128 => 1.3,
            // Low-medium dimensions (e.g., sentence transformers): Moderate adjustment  
            d if d <= 384 => 1.1,
            // Medium dimensions (e.g., BERT): Baseline
            d if d <= 768 => 1.0,
            // High dimensions (e.g., OpenAI): Can use smaller row groups
            d if d <= 1536 => 0.8,
            // Very high dimensions (e.g., research models): Smaller row groups for precision
            d if d <= 2048 => 0.7,
            // Ultra-high dimensions: Very small row groups
            _ => 0.6,
        }
    }
    
    /// Apply reasonable bounds to row group size
    fn apply_bounds(&self, size: usize) -> usize {
        // Minimum: 100 vectors (avoid too many small I/Os)
        // Maximum: 10,000 vectors (avoid excessive memory usage)
        size.max(100).min(10_000)
    }
    
    /// Estimate read latency for a row group of this size
    fn estimate_read_latency(&self, bytes: usize) -> f32 {
        let base_latency = self.io_profile.latency_per_io_us as f32 / 1000.0; // Convert to ms
        
        // Add transfer time based on size
        let transfer_time_ms = match self.io_profile.storage_tier {
            StorageTier::Hot => (bytes as f32 / (100.0 * 1024.0 * 1024.0)) * 1000.0, // 100MB/s
            StorageTier::Warm => (bytes as f32 / (50.0 * 1024.0 * 1024.0)) * 1000.0,  // 50MB/s
            StorageTier::Cold => (bytes as f32 / (10.0 * 1024.0 * 1024.0)) * 1000.0,  // 10MB/s
        };
        
        base_latency + transfer_time_ms
    }
    
    /// Generate human-readable rationale for the sizing decision
    fn generate_rationale(&self, vectors: usize, bytes: usize) -> String {
        let semantic_factor = self.calculate_semantic_accuracy_factor();
        let semantic_desc = match self.vector_dimension {
            d if d <= 128 => "low-dim (larger groups for stats)",
            d if d <= 384 => "med-dim (moderate adjustment)",
            d if d <= 768 => "standard-dim (baseline)",
            d if d <= 1536 => "high-dim (smaller for precision)",
            d if d <= 2048 => "very-high-dim (small for accuracy)",
            _ => "ultra-high-dim (minimal groups)",
        };
        
        format!(
            "Sized for {} vectors ({:.1}MB) based on {}MB optimal I/O, {} query pattern, {}-d {} vectors (semantic factor: {:.1}x)",
            vectors,
            bytes as f32 / (1024.0 * 1024.0),
            self.io_profile.optimal_io_size_bytes / (1024 * 1024),
            format!("{:?}", self.query_pattern).to_lowercase(),
            self.vector_dimension,
            semantic_desc,
            semantic_factor
        )
    }
    
    /// Create configuration for specific cloud providers
    pub fn for_s3_standard(dimension: usize, metadata_bytes: usize) -> Self {
        Self::new(CloudIOProfile::s3_standard(), dimension, metadata_bytes)
    }
    
    pub fn for_gcs_standard(dimension: usize, metadata_bytes: usize) -> Self {
        Self::new(CloudIOProfile::gcs_standard(), dimension, metadata_bytes)
    }
    
    pub fn for_adls_gen2(dimension: usize, metadata_bytes: usize) -> Self {
        Self::new(CloudIOProfile::adls_gen2(), dimension, metadata_bytes)
    }
    
    /// Configure quantization
    pub fn with_quantization(mut self, config: QuantizationConfig) -> Self {
        self.quantization_config = Some(config);
        self
    }
    
    /// Configure query pattern
    pub fn with_query_pattern(mut self, pattern: QueryPattern) -> Self {
        self.query_pattern = pattern;
        self
    }
}

/// Result of optimal row group size calculation
#[derive(Debug, Clone)]
pub struct OptimalRowGroupSize {
    /// Number of vectors per row group
    pub vectors_per_rowgroup: usize,
    /// Bytes per vector (including metadata and Matrix Trinity overhead)
    pub bytes_per_vector: usize,
    /// Total bytes per row group
    pub total_rowgroup_bytes: usize,
    /// How well this aligns with optimal I/O size (1.0 = perfect)
    pub io_efficiency_ratio: f32,
    /// Estimated read latency for one row group (ms)
    pub estimated_read_latency_ms: f32,
    /// Human-readable explanation
    pub rationale: String,
}

/// Precomputed configurations for common scenarios
pub struct CommonConfigurations;

impl CommonConfigurations {
    /// OpenAI embeddings (1536 dimensions) on S3
    pub fn openai_s3() -> SmartRowGroupSizer {
        SmartRowGroupSizer::for_s3_standard(1536, 200) // 200 bytes avg metadata
            .with_quantization(QuantizationConfig {
                primary_level: QuantizationLevel::PQ8,
                store_fp32: true, // Dual storage for accuracy
                compression_ratio: 4.0,
            })
            .with_query_pattern(QueryPattern::HighSelectivity)
    }
    
    /// BERT embeddings (768 dimensions) on GCS
    pub fn bert_gcs() -> SmartRowGroupSizer {
        SmartRowGroupSizer::for_gcs_standard(768, 150)
            .with_quantization(QuantizationConfig {
                primary_level: QuantizationLevel::INT8,
                store_fp32: false,
                compression_ratio: 4.0,
            })
            .with_query_pattern(QueryPattern::MediumSelectivity)
    }
    
    /// High-dimensional research vectors (2048 dimensions) on ADLS
    pub fn research_adls() -> SmartRowGroupSizer {
        SmartRowGroupSizer::for_adls_gen2(2048, 500) // Larger metadata for research
            .with_quantization(QuantizationConfig {
                primary_level: QuantizationLevel::PQ4,
                store_fp32: true,
                compression_ratio: 8.0,
            })
            .with_query_pattern(QueryPattern::LowSelectivity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_openai_s3_sizing() {
        let sizer = CommonConfigurations::openai_s3();
        let result = sizer.calculate_optimal_rowgroup_size().unwrap();
        
        // Should be reasonable for S3 2MB I/O
        assert!(result.vectors_per_rowgroup >= 100);
        assert!(result.vectors_per_rowgroup <= 10000);
        assert!(result.total_rowgroup_bytes <= 4 * 1024 * 1024); // Max 4MB
        
        println!("OpenAI/S3: {} vectors, {:.1}MB, efficiency: {:.2}", 
                result.vectors_per_rowgroup,
                result.total_rowgroup_bytes as f32 / (1024.0 * 1024.0),
                result.io_efficiency_ratio);
    }
    
    #[test]
    fn test_bert_gcs_sizing() {
        let sizer = CommonConfigurations::bert_gcs();
        let result = sizer.calculate_optimal_rowgroup_size().unwrap();
        
        // GCS prefers 4MB chunks
        assert!(result.total_rowgroup_bytes <= 6 * 1024 * 1024); // Max 6MB
        
        println!("BERT/GCS: {} vectors, {:.1}MB, efficiency: {:.2}",
                result.vectors_per_rowgroup,
                result.total_rowgroup_bytes as f32 / (1024.0 * 1024.0),
                result.io_efficiency_ratio);
    }
    
    #[test] 
    fn test_dimension_scaling() {
        // Test how row group size scales with vector dimension
        println!("Testing semantic accuracy factor across dimensions:");
        for dimension in [128, 384, 768, 1536, 2048, 4096] {
            let sizer = SmartRowGroupSizer::for_s3_standard(dimension, 100);
            let result = sizer.calculate_optimal_rowgroup_size().unwrap();
            
            println!("Dim {}: {} vectors, {:.1}KB per vector, {:.2}x semantic factor",
                    dimension,
                    result.vectors_per_rowgroup,
                    result.bytes_per_vector as f32 / 1024.0,
                    sizer.calculate_semantic_accuracy_factor());
        }
    }
    
    #[test]
    fn test_semantic_accuracy_rationale() {
        let high_dim_sizer = SmartRowGroupSizer::for_s3_standard(1536, 200); // OpenAI
        let low_dim_sizer = SmartRowGroupSizer::for_s3_standard(128, 100);   // Word2Vec
        
        let high_result = high_dim_sizer.calculate_optimal_rowgroup_size().unwrap();
        let low_result = low_dim_sizer.calculate_optimal_rowgroup_size().unwrap();
        
        println!("High-dim (1536d): {}", high_result.rationale);
        println!("Low-dim (128d): {}", low_result.rationale);
        
        // High dimensional vectors should have smaller row groups for better precision
        assert!(high_result.vectors_per_rowgroup < low_result.vectors_per_rowgroup,
                "High-dim vectors should have smaller row groups for better semantic precision");
    }
}