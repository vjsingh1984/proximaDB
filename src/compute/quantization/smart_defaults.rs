//! Smart Defaults for Quantization Configuration
//!
//! This module provides intelligent defaults for quantization based on vector dimension,
//! use case patterns, and performance requirements.

use anyhow::Result;
use tracing::debug;

use crate::proto::proximadb_v1::{
    QuantizationConfig, QuantizationLevel, quantization_config::Strategy,
    quantization_level::QuantizationType,
};

/// Smart defaults generator for quantization configuration
pub struct QuantizationSmartDefaults;

impl QuantizationSmartDefaults {
    /// Generate smart default quantization config based on vector dimension and use case
    pub fn generate_for_dimension(dimension: usize) -> Result<QuantizationConfig> {
        debug!(
            "🧠 Generating smart quantization defaults for dimension: {}",
            dimension
        );

        let config = match dimension {
            // Invalid dimension
            0 => return Err(anyhow::anyhow!("Invalid dimension: 0")),

            // Small dimensions (d < 64): Minimal quantization to preserve quality
            1..=63 => Self::create_minimal_config(dimension),

            // Medium dimensions (64 <= d < 128): Binary + INT8 for good balance
            64..=127 => Self::create_balanced_config(dimension),

            // Large dimensions (128 <= d < 512): Full progressive with PQ
            128..=511 => Self::create_progressive_config(dimension),

            // Very large dimensions (d >= 512): Aggressive compression
            512.. => Self::create_aggressive_config(dimension),
        };

        debug!(
            "📊 Generated {} quantization levels for dimension {}",
            config.custom_levels.len(),
            dimension
        );

        Ok(config)
    }

    /// Create minimal quantization for small dimensions (preserve quality)
    fn create_minimal_config(_dimension: usize) -> QuantizationConfig {
        QuantizationConfig {
            enabled: Some(true),
            strategy: Some(Strategy::Minimal as i32),
            custom_levels: vec![
                // Only INT8 quantization for minimal compression
                QuantizationLevel {
                    level_id: "int8".to_string(),
                    r#type: QuantizationType::Scalar as i32,
                    bits: 8,
                    num_subvectors: 0,
                    adaptive_subvectors: false,
                    scale: 1.0,
                    offset: 0.0,
                    clamp_values: true,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 0,
                    min_recall: 0.95,
                    enable_validation: true,
                },
            ],
            enable_progressive_search: Some(true),
            binary_filter_selectivity: Some(0.0), // No binary filter for small dimensions
            int8_ranking_selectivity: Some(0.3),  // More conservative for quality
            pq_ranking_selectivity: Some(0.0),    // No PQ for small dimensions
            training_sample_size: Some(5000),     // Smaller training set
            quality_threshold: Some(0.95),
            enable_adaptive_training: Some(true),
            optimize_for_storage: Some(false),
            optimize_for_memory: Some(false),
            enable_simd_acceleration: Some(true),
            // New direct fields
            enable_binary: Some(false), // No binary for small dimensions
            enable_int8: Some(true),    // Use INT8
            enable_pq: Some(false),     // No PQ for small dimensions
            pq_segments: Some(0),
            pq_bits: Some(0),
            pq_codebooks: Some(0),
            binary_threshold: Some(0.0),
            int8_threshold: Some(0.3),
            pq_threshold: Some(0.0),
        }
    }

    /// Create balanced config for medium dimensions
    fn create_balanced_config(_dimension: usize) -> QuantizationConfig {
        QuantizationConfig {
            enabled: Some(true),
            strategy: Some(Strategy::SmartDefaults as i32),
            custom_levels: vec![
                // Binary filter (first stage)
                QuantizationLevel {
                    level_id: "binary".to_string(),
                    r#type: QuantizationType::Binary as i32,
                    bits: 1,
                    num_subvectors: 0,
                    adaptive_subvectors: false,
                    scale: 0.0,
                    offset: 0.0,
                    clamp_values: false,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 0, // First filter
                    min_recall: 0.7,    // Lower recall for filter stage
                    enable_validation: true,
                },
                // INT8 ranking (second stage)
                QuantizationLevel {
                    level_id: "int8".to_string(),
                    r#type: QuantizationType::Scalar as i32,
                    bits: 8,
                    num_subvectors: 0,
                    adaptive_subvectors: false,
                    scale: 1.0,
                    offset: 0.0,
                    clamp_values: true,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 1, // Second ranking
                    min_recall: 0.9,
                    enable_validation: true,
                },
            ],
            enable_progressive_search: Some(true),
            binary_filter_selectivity: Some(0.3), // 30% reduction in first stage
            int8_ranking_selectivity: Some(0.1),  // 10% pass to final rerank
            pq_ranking_selectivity: Some(0.0),    // No PQ yet for medium dimensions
            training_sample_size: Some(10000),
            quality_threshold: Some(0.95),
            enable_adaptive_training: Some(true),
            optimize_for_storage: Some(false),
            optimize_for_memory: Some(false),
            enable_simd_acceleration: Some(true),
            // New direct fields
            enable_binary: Some(true), // Enable binary for filtering
            enable_int8: Some(true),   // Enable INT8 for ranking
            enable_pq: Some(false),    // No PQ for medium dimensions
            pq_segments: Some(0),
            pq_bits: Some(0),
            pq_codebooks: Some(0),
            binary_threshold: Some(0.3),
            int8_threshold: Some(0.1),
            pq_threshold: Some(0.0),
        }
    }

    /// Create full progressive config for large dimensions
    fn create_progressive_config(dimension: usize) -> QuantizationConfig {
        let num_subvectors = Self::calculate_optimal_subvectors(dimension);

        QuantizationConfig {
            enabled: Some(true),
            strategy: Some(Strategy::SmartDefaults as i32),
            custom_levels: vec![
                // Binary filter (first stage)
                QuantizationLevel {
                    level_id: "binary".to_string(),
                    r#type: QuantizationType::Binary as i32,
                    bits: 1,
                    num_subvectors: 0,
                    adaptive_subvectors: false,
                    scale: 0.0,
                    offset: 0.0,
                    clamp_values: false,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 0,
                    min_recall: 0.7,
                    enable_validation: true,
                },
                // INT8 ranking (second stage)
                QuantizationLevel {
                    level_id: "int8".to_string(),
                    r#type: QuantizationType::Scalar as i32,
                    bits: 8,
                    num_subvectors: 0,
                    adaptive_subvectors: false,
                    scale: 1.0,
                    offset: 0.0,
                    clamp_values: true,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 1,
                    min_recall: 0.85,
                    enable_validation: true,
                },
                // PQ8 ranking (third stage)
                QuantizationLevel {
                    level_id: "pq8".to_string(),
                    r#type: QuantizationType::Product as i32,
                    bits: 8,
                    num_subvectors: num_subvectors as u32,
                    adaptive_subvectors: false,
                    scale: 0.0,
                    offset: 0.0,
                    clamp_values: false,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 2,
                    min_recall: 0.95,
                    enable_validation: true,
                },
            ],
            enable_progressive_search: Some(true),
            binary_filter_selectivity: Some(0.3), // 30% reduction
            int8_ranking_selectivity: Some(0.1),  // 10% pass to PQ
            pq_ranking_selectivity: Some(0.05),   // 5% pass to FP32 rerank
            training_sample_size: Some(10000),
            quality_threshold: Some(0.95),
            enable_adaptive_training: Some(true),
            optimize_for_storage: Some(true), // Enable storage optimization for large dims
            optimize_for_memory: Some(false),
            enable_simd_acceleration: Some(true),
            // New direct fields
            enable_binary: Some(true), // Enable binary for filtering
            enable_int8: Some(true),   // Enable INT8 for ranking
            enable_pq: Some(true),     // Enable PQ for large dimensions
            pq_segments: Some(num_subvectors as u32),
            pq_bits: Some(8),
            pq_codebooks: Some(0),
            binary_threshold: Some(0.3),
            int8_threshold: Some(0.1),
            pq_threshold: Some(0.05),
        }
    }

    /// Create aggressive config for very large dimensions
    fn create_aggressive_config(dimension: usize) -> QuantizationConfig {
        let num_subvectors = Self::calculate_optimal_subvectors(dimension);

        QuantizationConfig {
            enabled: Some(true),
            strategy: Some(Strategy::Aggressive as i32),
            custom_levels: vec![
                // Binary filter (first stage)
                QuantizationLevel {
                    level_id: "binary".to_string(),
                    r#type: QuantizationType::Binary as i32,
                    bits: 1,
                    num_subvectors: 0,
                    adaptive_subvectors: false,
                    scale: 0.0,
                    offset: 0.0,
                    clamp_values: false,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 0,
                    min_recall: 0.6, // Lower recall for aggressive compression
                    enable_validation: true,
                },
                // PQ4 ranking (second stage - aggressive compression)
                QuantizationLevel {
                    level_id: "pq4".to_string(),
                    r#type: QuantizationType::Product as i32,
                    bits: 4,
                    num_subvectors: num_subvectors as u32,
                    adaptive_subvectors: true, // Enable adaptive for very large dims
                    scale: 0.0,
                    offset: 0.0,
                    clamp_values: false,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 1,
                    min_recall: 0.8,
                    enable_validation: true,
                },
                // PQ8 ranking (third stage - quality ranking)
                QuantizationLevel {
                    level_id: "pq8".to_string(),
                    r#type: QuantizationType::Product as i32,
                    bits: 8,
                    num_subvectors: num_subvectors as u32,
                    adaptive_subvectors: false,
                    scale: 0.0,
                    offset: 0.0,
                    clamp_values: false,
                    threshold: 0.0,
                    sign_based: false,
                    enable_in_storage: true,
                    enable_in_index: true,
                    search_priority: 2,
                    min_recall: 0.9,
                    enable_validation: true,
                },
            ],
            enable_progressive_search: Some(true),
            binary_filter_selectivity: Some(0.4), // More aggressive filtering
            int8_ranking_selectivity: Some(0.0),  // Skip INT8 for aggressive mode
            pq_ranking_selectivity: Some(0.03),   // Smaller final set for rerank
            training_sample_size: Some(15000),    // Larger training set for complex data
            quality_threshold: Some(0.9),         // Slightly lower for aggressive compression
            enable_adaptive_training: Some(true),
            optimize_for_storage: Some(true), // Prioritize storage savings
            optimize_for_memory: Some(true),  // Prioritize memory usage
            enable_simd_acceleration: Some(true),
            // New direct fields
            enable_binary: Some(true), // Enable binary for aggressive filtering
            enable_int8: Some(false),  // Skip INT8 in aggressive mode
            enable_pq: Some(true),     // Enable PQ4 for aggressive compression
            pq_segments: Some(num_subvectors as u32),
            pq_bits: Some(4), // Use PQ4 for aggressive compression
            pq_codebooks: Some(0),
            binary_threshold: Some(0.4),
            int8_threshold: Some(0.0),
            pq_threshold: Some(0.03),
        }
    }

    /// Calculate optimal number of subvectors for PQ based on dimension
    fn calculate_optimal_subvectors(dimension: usize) -> i32 {
        // Rule of thumb: subvectors = dimension / 8, with bounds [8, 64]
        let optimal = (dimension / 8).clamp(8, 64);

        // Ensure dimension is divisible by subvectors for clean splits
        let mut subvectors = optimal;
        while !dimension.is_multiple_of(subvectors) && subvectors > 8 {
            subvectors -= 1;
        }

        debug!(
            "📐 Calculated {} subvectors for dimension {}",
            subvectors, dimension
        );
        subvectors as i32
    }

    /// Generate config based on use case pattern
    pub fn generate_for_use_case(use_case: &str, dimension: usize) -> Result<QuantizationConfig> {
        debug!(
            "🎯 Generating quantization config for use case: {}, dimension: {}",
            use_case, dimension
        );

        let mut config = Self::generate_for_dimension(dimension)?;

        // Adjust config based on use case
        match use_case {
            "real_time" => {
                // Optimize for speed over quality
                config.binary_filter_selectivity = Some(0.5); // More aggressive filtering
                config.optimize_for_memory = Some(true);
                config.quality_threshold = Some(0.85); // Lower quality threshold
            }
            "high_quality" => {
                // Optimize for quality over speed
                config.binary_filter_selectivity = Some(0.1); // Less aggressive filtering
                config.quality_threshold = Some(0.98); // Higher quality threshold
                config.training_sample_size = Some(20000); // More training data
            }
            "storage_optimized" => {
                // Optimize for storage compression
                config.strategy = Some(Strategy::Aggressive as i32);
                config.optimize_for_storage = Some(true);
                config.binary_filter_selectivity = Some(0.4);
            }
            "memory_constrained" => {
                // Optimize for minimal memory usage
                config.optimize_for_memory = Some(true);
                config.training_sample_size = Some(5000); // Smaller training set
                // Keep only essential levels
                config.custom_levels.truncate(2);
            }
            _ => {
                // Keep default smart configuration
                debug!(
                    "Using default smart configuration for unknown use case: {}",
                    use_case
                );
            }
        }

        Ok(config)
    }

    /// Validate quantization configuration for correctness
    pub fn validate_config(config: &QuantizationConfig, dimension: usize) -> Result<()> {
        if !config.enabled.unwrap_or(false) {
            return Ok(()); // No validation needed for disabled quantization
        }

        // Check maximum levels limit (5 for system protection)
        if config.custom_levels.len() > 5 {
            return Err(anyhow::anyhow!(
                "Too many quantization levels: {} (max 5 allowed)",
                config.custom_levels.len()
            ));
        }

        // Validate each level
        for (i, level) in config.custom_levels.iter().enumerate() {
            Self::validate_level(level, dimension, i)?;
        }

        // Validate progressive search thresholds
        if config.enable_progressive_search.unwrap_or(true) {
            let selectivity = config.binary_filter_selectivity.unwrap_or(0.1);
            if !(0.0..=1.0).contains(&selectivity) {
                return Err(anyhow::anyhow!(
                    "Invalid binary filter selectivity: {:?} (must be 0.0-1.0)",
                    selectivity
                ));
            }
        }

        debug!(
            "✅ Quantization config validation passed for dimension {}",
            dimension
        );
        Ok(())
    }

    /// Validate individual quantization level
    fn validate_level(level: &QuantizationLevel, dimension: usize, index: usize) -> Result<()> {
        // Validate bits per element
        match level.r#type() {
            QuantizationType::Binary => {
                if level.bits != 1 {
                    return Err(anyhow::anyhow!(
                        "Binary quantization level {} must use 1 bit, got {}",
                        index,
                        level.bits
                    ));
                }
            }
            QuantizationType::Scalar => {
                if ![4, 8, 16].contains(&level.bits) {
                    return Err(anyhow::anyhow!(
                        "Scalar quantization level {} bits must be 4, 8, or 16, got {}",
                        index,
                        level.bits
                    ));
                }
            }
            QuantizationType::Product => {
                if ![4, 6, 8, 16].contains(&level.bits) {
                    return Err(anyhow::anyhow!(
                        "Product quantization level {} bits must be 4, 6, 8, or 16, got {}",
                        index,
                        level.bits
                    ));
                }

                // Validate subvectors for PQ
                if level.num_subvectors > 0 {
                    if level.num_subvectors < 1 || level.num_subvectors > 64 {
                        return Err(anyhow::anyhow!(
                            "PQ level {} subvectors must be 1-64, got {}",
                            index,
                            level.num_subvectors
                        ));
                    }

                    if !dimension.is_multiple_of(level.num_subvectors as usize) {
                        return Err(anyhow::anyhow!(
                            "PQ level {} subvectors {} must divide dimension {} evenly",
                            index,
                            level.num_subvectors,
                            dimension
                        ));
                    }
                }
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown quantization type for level {}",
                    index
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smart_defaults_small_dimension() {
        let config = QuantizationSmartDefaults::generate_for_dimension(32).unwrap();
        assert_eq!(config.strategy, Some(Strategy::Minimal as i32));
        assert_eq!(config.custom_levels.len(), 1);
        assert_eq!(config.custom_levels[0].level_id, "int8");
    }

    #[test]
    fn test_smart_defaults_medium_dimension() {
        let config = QuantizationSmartDefaults::generate_for_dimension(96).unwrap();
        assert_eq!(config.strategy, Some(Strategy::SmartDefaults as i32));
        assert_eq!(config.custom_levels.len(), 2);
        assert_eq!(config.custom_levels[0].level_id, "binary");
        assert_eq!(config.custom_levels[1].level_id, "int8");
    }

    #[test]
    fn test_smart_defaults_large_dimension() {
        let config = QuantizationSmartDefaults::generate_for_dimension(384).unwrap();
        assert_eq!(config.strategy, Some(Strategy::SmartDefaults as i32));
        assert_eq!(config.custom_levels.len(), 3);
        assert_eq!(config.custom_levels[0].level_id, "binary");
        assert_eq!(config.custom_levels[1].level_id, "int8");
        assert_eq!(config.custom_levels[2].level_id, "pq8");
    }

    #[test]
    fn test_smart_defaults_very_large_dimension() {
        let config = QuantizationSmartDefaults::generate_for_dimension(1024).unwrap();
        assert_eq!(config.strategy, Some(Strategy::Aggressive as i32));
        assert_eq!(config.custom_levels.len(), 3);
        assert_eq!(config.custom_levels[1].level_id, "pq4"); // Aggressive uses PQ4
    }

    #[test]
    fn test_subvector_calculation() {
        assert_eq!(
            QuantizationSmartDefaults::calculate_optimal_subvectors(128),
            16
        );
        assert_eq!(
            QuantizationSmartDefaults::calculate_optimal_subvectors(384),
            48
        );
        assert_eq!(
            QuantizationSmartDefaults::calculate_optimal_subvectors(768),
            64
        ); // Capped at 64
    }

    #[test]
    fn test_use_case_real_time() {
        let config = QuantizationSmartDefaults::generate_for_use_case("real_time", 384).unwrap();
        assert_eq!(config.optimize_for_memory, Some(true));
        assert_eq!(config.quality_threshold, Some(0.85));
    }

    #[test]
    fn test_use_case_high_quality() {
        let config = QuantizationSmartDefaults::generate_for_use_case("high_quality", 384).unwrap();
        assert_eq!(config.quality_threshold, Some(0.98));
        assert_eq!(config.training_sample_size, Some(20000));
    }

    #[test]
    fn test_validation_success() {
        let config = QuantizationSmartDefaults::generate_for_dimension(384).unwrap();
        assert!(QuantizationSmartDefaults::validate_config(&config, 384).is_ok());
    }

    #[test]
    fn test_validation_too_many_levels() {
        let mut config = QuantizationSmartDefaults::generate_for_dimension(384).unwrap();
        // Add too many levels
        for i in 0..10 {
            config.custom_levels.push(QuantizationLevel {
                level_id: format!("extra_{}", i),
                r#type: QuantizationType::Scalar as i32,
                bits: 8,
                num_subvectors: 0,
                adaptive_subvectors: false,
                scale: 0.0,
                offset: 0.0,
                clamp_values: false,
                threshold: 0.0,
                sign_based: false,
                enable_in_storage: false,
                enable_in_index: false,
                search_priority: 0,
                min_recall: 0.0,
                enable_validation: false,
            });
        }

        assert!(QuantizationSmartDefaults::validate_config(&config, 384).is_err());
    }
}
