//! Quantization Engine Selection Logic
//!
//! Shared logic for intelligently selecting between persistent StorageQuantizationEngine
//! and stateless UnifiedQuantizationEngine based on operation context.

use crate::storage::traits::FlushParameters;
use tracing::debug;

/// Smart quantization engine selection logic
///
/// This module provides the shared decision logic used by all storage engines
/// to determine whether to use persistent collection-based quantization or
/// stateless ad-hoc quantization.
pub struct QuantizationSelector;

impl QuantizationSelector {
    /// Check if we should use persistent quantization for this operation (simple version)
    ///
    /// Returns true for operations that benefit from persistent quantization:
    /// - Large collections (>1000 vectors)
    /// - Flush operations (writes to persistent storage)
    /// - Frequent operations that benefit from cached codebooks
    pub fn should_use_persistent_quantization_simple(
        operation_context: &str,
        collection_size: Option<usize>,
    ) -> bool {
        match operation_context {
            "flush" | "compact" => true, // Write operations benefit from persistent codebooks
            "search" | "query" => {
                // Large collections benefit from persistent codebooks
                collection_size.map_or(false, |size| size > 1000)
            }
            _ => false, // Default to stateless for unknown operations
        }
    }

    /// Check if we should use persistent quantization for this operation
    ///
    /// Returns true when ALL conditions are met:
    /// - Operation has collection configuration
    /// - Quantization is explicitly enabled
    /// - Collection ID is provided
    ///
    /// This indicates a persistent collection-based operation that benefits
    /// from cached codebooks and collection-partitioned storage.
    pub fn should_use_persistent_quantization(params: &FlushParameters, engine_name: &str) -> bool {
        let has_collection_config = params.collection_config.is_some();
        // Check if quantization is enabled in collection config
        let quantization_enabled = params
            .collection_config
            .as_ref()
            .and_then(|collection| collection.config.as_ref())
            .and_then(|config| config.quantization.as_ref())
            .map(|q| q.enabled);
        let has_collection_id = params.collection_id.is_some();

        if has_collection_config
            && quantization_enabled.flatten().unwrap_or(false)
            && has_collection_id
        {
            debug!(
                "🎯 {}: Using persistent StorageQuantizationEngine for collection: {:?}",
                engine_name, params.collection_id
            );
            true
        } else {
            debug!(
                "🔄 {}: Using stateless UnifiedQuantizationEngine for ad-hoc query",
                engine_name
            );
            false
        }
    }

    /// Detailed analysis of why a particular quantization engine was selected
    /// Useful for debugging and monitoring
    pub fn get_selection_reason(
        params: &FlushParameters,
        engine_name: &str,
    ) -> QuantizationSelectionReason {
        let has_collection_config = params.collection_config.is_some();
        // Check if quantization is enabled in collection config
        let quantization_enabled = params
            .collection_config
            .as_ref()
            .and_then(|collection| collection.config.as_ref())
            .and_then(|config| config.quantization.as_ref())
            .map(|q| q.enabled);
        let has_collection_id = params.collection_id.is_some();

        if has_collection_config
            && quantization_enabled.flatten().unwrap_or(false)
            && has_collection_id
        {
            QuantizationSelectionReason::Persistent {
                engine: engine_name.to_string(),
                collection_id: params.collection_id.clone().unwrap_or_default(),
                reason: "Collection-based operation with quantization enabled".to_string(),
            }
        } else {
            let mut missing_requirements = Vec::new();

            if !has_collection_config {
                missing_requirements.push("collection_config");
            }
            if !quantization_enabled.flatten().unwrap_or(false) {
                missing_requirements.push("quantization_enabled");
            }
            if !has_collection_id {
                missing_requirements.push("collection_id");
            }

            QuantizationSelectionReason::Stateless {
                engine: engine_name.to_string(),
                reason: format!("Missing requirements: {}", missing_requirements.join(", ")),
                missing_requirements,
            }
        }
    }

    /// Check if operation context supports quantization at all
    pub fn supports_quantization(params: &FlushParameters) -> bool {
        // Basic checks for quantization support
        !params.vector_records.is_empty()
            && params
                .vector_records
                .iter()
                .all(|record| !record.vector.is_empty())
    }

    /// Get recommended quantization level based on operation context
    pub fn get_recommended_quantization_level(
        params: &FlushParameters,
    ) -> RecommendedQuantizationLevel {
        let vector_count = params.vector_records.len();
        let dimension = params
            .vector_records
            .first()
            .map(|record| record.vector.len())
            .unwrap_or(0);

        // Recommendations based on data characteristics
        match (vector_count, dimension) {
            // Small datasets: prefer INT8 (fast, no training)
            (0..=1000, _) => RecommendedQuantizationLevel::Int8 {
                reason: "Small dataset - INT8 provides fast quantization without training overhead"
                    .to_string(),
            },

            // Medium datasets with small dimensions: PQ4
            (1001..=10000, 1..=256) => RecommendedQuantizationLevel::Pq4 {
                subvectors: std::cmp::max(4, dimension / 64),
                reason: "Medium dataset, small dimensions - PQ4 provides good compression"
                    .to_string(),
            },

            // Medium datasets with large dimensions: PQ8
            (1001..=10000, 257..) => RecommendedQuantizationLevel::Pq8 {
                subvectors: std::cmp::max(8, dimension / 32),
                reason: "Medium dataset, large dimensions - PQ8 balances compression and quality"
                    .to_string(),
            },

            // Large datasets: PQ8 or PQ16 based on dimension
            (10001.., 1..=512) => RecommendedQuantizationLevel::Pq8 {
                subvectors: std::cmp::max(16, dimension / 32),
                reason: "Large dataset - PQ8 with more subvectors for quality".to_string(),
            },

            (10001.., 513..) => RecommendedQuantizationLevel::Pq16 {
                subvectors: std::cmp::max(24, dimension / 32),
                reason: "Large dataset, high dimensions - PQ16 for maximum quality".to_string(),
            },

            // Edge case: empty or invalid data
            _ => RecommendedQuantizationLevel::None {
                reason: "Invalid or empty dataset - no quantization recommended".to_string(),
            },
        }
    }
}

/// Reason why a particular quantization engine was selected
#[derive(Debug, Clone)]
pub enum QuantizationSelectionReason {
    Persistent {
        engine: String,
        collection_id: String,
        reason: String,
    },
    Stateless {
        engine: String,
        reason: String,
        missing_requirements: Vec<&'static str>,
    },
}

/// Recommended quantization level for an operation
#[derive(Debug, Clone)]
pub enum RecommendedQuantizationLevel {
    None { reason: String },
    Binary { reason: String },
    Int8 { reason: String },
    Pq4 { subvectors: usize, reason: String },
    Pq8 { subvectors: usize, reason: String },
    Pq16 { subvectors: usize, reason: String },
}

impl QuantizationSelectionReason {
    /// Get a human-readable description
    pub fn description(&self) -> String {
        match self {
            QuantizationSelectionReason::Persistent {
                engine,
                collection_id,
                reason,
            } => {
                format!(
                    "[{}] Persistent quantization for '{}': {}",
                    engine, collection_id, reason
                )
            }
            QuantizationSelectionReason::Stateless { engine, reason, .. } => {
                format!("[{}] Stateless quantization: {}", engine, reason)
            }
        }
    }

    /// Check if this selection uses persistent quantization
    pub fn is_persistent(&self) -> bool {
        matches!(self, QuantizationSelectionReason::Persistent { .. })
    }
}

impl RecommendedQuantizationLevel {
    /// Get a human-readable description
    pub fn description(&self) -> String {
        match self {
            RecommendedQuantizationLevel::None { reason } => format!("No quantization: {}", reason),
            RecommendedQuantizationLevel::Binary { reason } => {
                format!("Binary quantization: {}", reason)
            }
            RecommendedQuantizationLevel::Int8 { reason } => {
                format!("INT8 quantization: {}", reason)
            }
            RecommendedQuantizationLevel::Pq4 { subvectors, reason } => {
                format!("PQ4 with {} subvectors: {}", subvectors, reason)
            }
            RecommendedQuantizationLevel::Pq8 { subvectors, reason } => {
                format!("PQ8 with {} subvectors: {}", subvectors, reason)
            }
            RecommendedQuantizationLevel::Pq16 { subvectors, reason } => {
                format!("PQ16 with {} subvectors: {}", subvectors, reason)
            }
        }
    }
}

// TODO: Fix compilation errors - enabled field is now Option<bool>
// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::proto::proximadb_v1::VectorRecord;
//     use std::collections::HashMap;
//
//     fn create_test_params(
//         collection_id: Option<String>,
//         enable_quantization: Option<bool>,
//         has_collection_config: bool,
//         vector_count: usize,
//     ) -> FlushParameters {
//         let vectors = (0..vector_count)
//             .map(|i| VectorRecord {
//                 id: format!("vec_{}", i),
//                 vector: vec![1.0, 2.0, 3.0, 4.0], // 4D vectors
//                 metadata: HashMap::new(),
//                 ..Default::default()
//             })
//             .collect();
//
//         // Create collection config with quantization settings
//         let collection_config = if has_collection_config {
//             let mut config = crate::proto::proximadb_v1::CollectionConfig::default();
//             if let Some(enabled) = enable_quantization {
//                 config.quantization = Some(crate::proto::proximadb_v1::QuantizationConfig {
//                     enabled,
//                     ..Default::default()
//                 });
//             }
//             let collection = crate::proto::proximadb_v1::Collection {
//                 config: Some(config),
//                 ..Default::default()
//             };
//             Some(collection)
//         } else {
//             None
//         };
//
//         FlushParameters {
//             collection_id,
//             vector_records: vectors,
//             collection_config,
//             ..Default::default()
//         }
//     }
//
//     #[test]
//     fn test_persistent_quantization_selection() {
//         let params = create_test_params(
//             Some("test_collection".to_string()),
//             Some(true),
//             true,
//             100,
//         );
//
//         assert!(QuantizationSelector::should_use_persistent_quantization(&params, "TEST"));
//
//         let reason = QuantizationSelector::get_selection_reason(&params, "TEST");
//         assert!(reason.is_persistent());
//     }
//
//     #[test]
//     fn test_stateless_quantization_selection() {
//         // Missing collection_id
//         let params1 = create_test_params(None, Some(true), true, 100);
//         assert!(!QuantizationSelector::should_use_persistent_quantization(&params1, "TEST"));
//
//         // Quantization disabled
//         let params2 = create_test_params(
//             Some("test".to_string()),
//             Some(false),
//             true,
//             100,
//         );
//         assert!(!QuantizationSelector::should_use_persistent_quantization(&params2, "TEST"));
//
//         // No collection config
//         let params3 = create_test_params(
//             Some("test".to_string()),
//             Some(true),
//             false,
//             100,
//         );
//         assert!(!QuantizationSelector::should_use_persistent_quantization(&params3, "TEST"));
//     }
//
//     #[test]
//     fn test_quantization_recommendations() {
//         // Small dataset
//         let params1 = create_test_params(Some("test".to_string()), Some(true), true, 500);
//         let rec1 = QuantizationSelector::get_recommended_quantization_level(&params1);
//         assert!(matches!(rec1, RecommendedQuantizationLevel::Int8 { .. }));
//
//         // Medium dataset
//         let params2 = create_test_params(Some("test".to_string()), Some(true), true, 5000);
//         let rec2 = QuantizationSelector::get_recommended_quantization_level(&params2);
//         assert!(matches!(rec2, RecommendedQuantizationLevel::Pq4 { .. }));
//
//         // Large dataset
//         let params3 = create_test_params(Some("test".to_string()), Some(true), true, 50000);
//         let rec3 = QuantizationSelector::get_recommended_quantization_level(&params3);
//         assert!(matches!(rec3, RecommendedQuantizationLevel::Pq8 { .. }));
//     }
// }
