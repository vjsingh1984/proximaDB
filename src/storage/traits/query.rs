//! Query context types for storage engine operations.
//!
//! This module provides types used by storage engines during query execution,
//! including row-level security predicates, search parameters, and metadata.

use crate::core::search::BlockPruneMode;
use crate::proto::proximadb_v1::Collection;
use crate::security::rbac_service::{TenantContext, UnifiedUserContext};
pub use proximadb_quantization_types::QuantizationType;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{PerformanceTier, StorageEngineStrategy};

/// Predicate evaluated by the storage engine at scan time for row-level security.
///
/// Engines that implement `rls_record_filter()` apply this predicate inside their
/// scan iterator — no application-layer tenant filtering required.
/// See MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc §8.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RlsRecordPredicate {
    /// If `Some`, only records whose `tenant_id` matches are returned.
    pub required_tenant_id: Option<String>,
    /// If `Some`, only records that contain this principal in `permitted_principals`
    /// (or have an empty `permitted_principals` list) are returned.
    pub required_principal: Option<String>,
}

impl RlsRecordPredicate {
    /// Return `true` when no filtering is needed (both fields are `None`).
    pub fn is_passthrough(&self) -> bool {
        self.required_tenant_id.is_none() && self.required_principal.is_none()
    }
}

/// Search context for STORAGE ENGINES - bundles immutable references to search parameters
/// and collection configuration for zero-copy access during search operations.
///
/// IMPORTANT: This is the STORAGE LAYER context. Do not confuse with:
/// - `core::search::SearchPlan` - Used for search planning/optimization
/// - `core::service_types::SearchRequest` - Used for API request representation
///
/// Used by: Storage engines (SST, VIPER, NOVA, SWIFT, RAPTOR)
/// Created by: VectorOperationsService.execute_search_internal()
///
/// Design principles:
/// - Immutable: All references are read-only during search
/// - Zero-copy: Uses Arc for shared ownership without cloning
/// - Cache-friendly: Collection comes directly from cache as Arc
/// - Extensible: Additional context can be added as needed
#[derive(Debug, Clone)]
pub struct StorageQueryContext {
    /// Original search parameters (immutable reference)
    pub search_params: Arc<crate::core::search::SearchParams>,

    /// Collection configuration from cache (immutable reference)
    /// Contains storage_assignment with storage URL
    pub collection: Arc<Collection>,

    /// Additional context that might be needed during search
    /// (can be extended without breaking existing code)
    pub metadata: StorageQueryMetadata,

    /// User context for RBAC authorization checks
    /// Optional for backward compatibility with existing code
    pub user_context: Option<UnifiedUserContext>,

    /// Tenant context for multi-tenant operations
    /// Optional for backward compatibility with existing code
    pub tenant_context: Option<TenantContext>,
}

/// Additional metadata for storage query context.
/// Contains all information storage engines need - no additional cache lookups required.
#[derive(Debug, Clone, Default)]
pub struct StorageQueryMetadata {
    /// Collection ID extracted for convenience
    pub collection_id: String,

    /// Whether this search should use AXIS indexes
    pub use_axis_indexes: bool,

    /// Whether progressive quantization is available
    pub has_quantization: bool,

    /// Dimension of vectors in this collection
    pub dimension: usize,

    /// Distance metric for the collection
    pub distance_metric: crate::compute::distance_computation::DistanceMetric,

    /// Storage engine strategy for this collection
    pub storage_strategy: StorageEngineStrategy,

    /// Base storage path for this collection (extracted from storage_assignment)
    pub storage_path: String,

    /// Parsed quantization configuration for progressive search
    pub quantization_config: Option<ParsedQuantizationConfig>,

    /// Collection size estimates for strategy selection
    pub estimated_vector_count: u64,
    pub estimated_size_bytes: u64,

    /// Performance hints for engines
    pub performance_tier: PerformanceTier,
    pub compression_enabled: bool,
    pub quantization_enabled: bool,
}

/// Parsed quantization configuration for efficient progressive search.
#[derive(Debug, Clone)]
pub struct ParsedQuantizationConfig {
    /// Strategy being used (SmartDefaults, CustomLevels, etc.)
    pub strategy: crate::proto::proximadb_v1::quantization_config::Strategy,

    /// Whether progressive search is enabled
    pub progressive_search_enabled: bool,

    /// Ordered quantization levels for progressive refinement
    pub progressive_levels: Vec<QuantizationLevel>,

    /// Search stage selectivity thresholds
    pub binary_filter_selectivity: f32,
    pub int8_ranking_selectivity: f32,
    pub pq_ranking_selectivity: f32,

    /// Quality and performance settings
    pub quality_threshold: f32,
    pub training_sample_size: i32,
    pub enable_simd_acceleration: bool,
    pub optimize_for_storage: bool,
    pub optimize_for_memory: bool,
}

// Re-export foundation quantization types for backward compatibility
// TODO: Migrate all uses to FoundationQuantizationType (Phase 2.2)

/// Individual quantization level for progressive search.
#[derive(Debug, Clone)]
pub struct QuantizationLevel {
    /// Level identifier (e.g., "binary", "int8", "pq8")
    pub level_id: String,

    /// Quantization type (using foundation type)
    pub quantization_type: QuantizationType,

    /// Bits per element
    pub bits: i32,

    /// Search priority (0 = first filter)
    pub search_priority: i32,

    /// PQ-specific settings
    pub num_subvectors: Option<i32>,

    /// Minimum recall for this level
    pub min_recall: f32,
}

// NOTE: QuantizationType is now re-exported from the proximadb-quantization-types
// foundation crate. The local QuantizationLevel struct remains the parsed
// progressive-search level shape used by storage engines.
//
// Legacy migration:
// - QuantizationType::Uniform → QuantizationType::None
// - All other variants map directly to foundation types

impl StorageQueryContext {
    /// Parse quantization config into ready-to-use format for progressive search.
    fn parse_quantization_config(
        quant_config: &crate::proto::proximadb_v1::QuantizationConfig,
        dimension: usize,
    ) -> Option<ParsedQuantizationConfig> {
        if !quant_config.enabled.unwrap_or(false) {
            return None;
        }

        // Parse or generate progressive levels
        let progressive_levels = if quant_config.custom_levels.is_empty() {
            // Use smart defaults if no custom levels provided
            if let Ok(smart_config) =
                crate::compute::quantization::QuantizationSmartDefaults::generate_for_dimension(
                    dimension,
                )
            {
                Self::parse_proto_levels(&smart_config.custom_levels)
            } else {
                Vec::new()
            }
        } else {
            Self::parse_proto_levels(&quant_config.custom_levels)
        };

        Some(ParsedQuantizationConfig {
            strategy: quant_config.strategy(),
            progressive_search_enabled: quant_config.enable_progressive_search.unwrap_or(false),
            progressive_levels,
            binary_filter_selectivity: quant_config.binary_filter_selectivity.unwrap_or(0.3),
            int8_ranking_selectivity: quant_config.int8_ranking_selectivity.unwrap_or(0.1),
            pq_ranking_selectivity: quant_config.pq_ranking_selectivity.unwrap_or(0.05),
            quality_threshold: quant_config.quality_threshold.unwrap_or(0.95),
            training_sample_size: quant_config.training_sample_size.unwrap_or(10000) as i32,
            enable_simd_acceleration: quant_config.enable_simd_acceleration.unwrap_or(true),
            optimize_for_storage: quant_config.optimize_for_storage.unwrap_or(false),
            optimize_for_memory: quant_config.optimize_for_memory.unwrap_or(false),
        })
    }

    /// Parse proto levels into internal format.
    fn parse_proto_levels(
        proto_levels: &[crate::proto::proximadb_v1::QuantizationLevel],
    ) -> Vec<QuantizationLevel> {
        use crate::proto::proximadb_v1::quantization_level::QuantizationType as ProtoQuantType;

        let mut levels: Vec<_> = proto_levels
            .iter()
            .enumerate()
            .map(|(idx, level)| {
                let quantization_type = match level.r#type() {
                    ProtoQuantType::Binary => QuantizationType::Binary,
                    ProtoQuantType::Scalar => QuantizationType::Scalar,
                    ProtoQuantType::Product => QuantizationType::Product,
                    ProtoQuantType::Uniform => QuantizationType::None,
                    ProtoQuantType::None => QuantizationType::None,
                };

                QuantizationLevel {
                    level_id: level.level_id.clone(),
                    quantization_type,
                    bits: level.bits as i32,
                    search_priority: idx as i32,
                    num_subvectors: Some(level.num_subvectors as i32),
                    min_recall: 0.9,
                }
            })
            .collect();

        levels.sort_by_key(|l| l.search_priority);
        levels
    }

    /// Create a new search context from cached components.
    pub fn new(
        search_params: Arc<crate::core::search::SearchParams>,
        collection: Arc<Collection>,
    ) -> Self {
        let config = collection.config.as_ref();
        let storage_assignment = collection.storage_assignment.as_ref();

        let storage_strategy = config
            .and_then(|c| c.storage_engine)
            .and_then(|e| crate::proto::proximadb_v1::StorageEngine::try_from(e).ok())
            .map_or(StorageEngineStrategy::Sst, |engine| match engine {
                crate::proto::proximadb_v1::StorageEngine::Viper => StorageEngineStrategy::Viper,
                crate::proto::proximadb_v1::StorageEngine::Sst => StorageEngineStrategy::Sst,
                crate::proto::proximadb_v1::StorageEngine::Nova => StorageEngineStrategy::Nova,
                crate::proto::proximadb_v1::StorageEngine::Helix => StorageEngineStrategy::Helix,
                crate::proto::proximadb_v1::StorageEngine::Swift => StorageEngineStrategy::Swift,
                crate::proto::proximadb_v1::StorageEngine::Raptor => StorageEngineStrategy::Raptor,
                crate::proto::proximadb_v1::StorageEngine::Tst => StorageEngineStrategy::TimeSeries,
                _ => StorageEngineStrategy::Sst,
            });

        let mut adjusted_params = (*search_params).clone();
        if matches!(
            storage_strategy,
            StorageEngineStrategy::Viper
                | StorageEngineStrategy::Nova
                | StorageEngineStrategy::Raptor
        ) {
            adjusted_params.block_prune.force_exact = true;
            adjusted_params.block_prune.mode = BlockPruneMode::Sqrt;
            adjusted_params.block_prune.ratio = 0.0;
            adjusted_params.block_prune.min_keep = 0;
            adjusted_params.block_prune.max_keep = 0;
        }

        let metadata = StorageQueryMetadata {
            collection_id: collection.id.clone(),
            use_axis_indexes: config
                .and_then(|c| {
                    if c.index_configs.is_empty() {
                        None
                    } else {
                        Some(true)
                    }
                })
                .unwrap_or(false),
            has_quantization: config.and_then(|c| c.quantization.as_ref()).is_some(),
            dimension: config.map_or(0, |c| c.dimension as usize),
            distance_metric: config
                .and_then(|c| c.distance_metric)
                .and_then(|metric| {
                    crate::proto::proximadb_v1::DistanceMetric::try_from(metric).ok()
                })
                .map_or(
                    crate::compute::distance_computation::DistanceMetric::Cosine,
                    |metric| match metric {
                        crate::proto::proximadb_v1::DistanceMetric::Unspecified
                        | crate::proto::proximadb_v1::DistanceMetric::Cosine => {
                            crate::compute::distance_computation::DistanceMetric::Cosine
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Euclidean => {
                            crate::compute::distance_computation::DistanceMetric::Euclidean
                        }
                        crate::proto::proximadb_v1::DistanceMetric::DotProduct => {
                            crate::compute::distance_computation::DistanceMetric::DotProduct
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Hamming => {
                            crate::compute::distance_computation::DistanceMetric::Hamming
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Manhattan => {
                            crate::compute::distance_computation::DistanceMetric::Manhattan
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Jaccard => {
                            crate::compute::distance_computation::DistanceMetric::Jaccard
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Angular => {
                            crate::compute::distance_computation::DistanceMetric::Angular
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Chebyshev => {
                            crate::compute::distance_computation::DistanceMetric::Chebyshev
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Canberra => {
                            crate::compute::distance_computation::DistanceMetric::Canberra
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Minkowski => {
                            crate::compute::distance_computation::DistanceMetric::Minkowski
                        }
                        crate::proto::proximadb_v1::DistanceMetric::BrayCurtis => {
                            crate::compute::distance_computation::DistanceMetric::BrayCurtis
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Hellinger => {
                            crate::compute::distance_computation::DistanceMetric::Hellinger
                        }
                        crate::proto::proximadb_v1::DistanceMetric::Custom => {
                            crate::compute::distance_computation::DistanceMetric::Custom
                        }
                    },
                ),
            storage_strategy,
            storage_path: storage_assignment
                .map_or_else(|| "./data".to_string(), |sa| sa.base_location.clone()),
            estimated_vector_count: 0,
            estimated_size_bytes: 0,
            performance_tier: PerformanceTier::Warm,
            compression_enabled: config
                .and_then(|c| c.storage_config.as_ref())
                .is_some_and(|s| s.compression.unwrap_or(0) != 0),
            quantization_enabled: config
                .and_then(|c| c.quantization.as_ref())
                .is_some_and(|_| true),
            quantization_config: config.and_then(|c| c.quantization.as_ref()).and_then(|qc| {
                config
                    .map(|c| c.dimension)
                    .and_then(|dim| Self::parse_quantization_config(qc, dim as usize))
            }),
        };

        Self {
            search_params: Arc::new(adjusted_params),
            collection,
            metadata,
            user_context: None,
            tenant_context: None,
        }
    }

    /// Get the query vector (convenience method).
    pub fn query_vector(&self) -> Option<&[f32]> {
        if let Some(ref vector) = self.search_params.vector {
            return Some(vector.as_slice());
        }

        self.search_params
            .query_vectors
            .as_ref()
            .and_then(|vecs| vecs.first())
            .map(|v| v.as_slice())
    }

    /// Get top_k value with fallback to default.
    pub fn top_k(&self) -> usize {
        self.search_params.top_k.unwrap_or(10)
    }

    /// Get distance metric (pre-computed from collection config).
    pub fn distance_metric(&self) -> crate::compute::distance_computation::DistanceMetric {
        self.search_params
            .distance_metric
            .unwrap_or(self.metadata.distance_metric)
    }

    /// Get dimension from metadata (pre-computed).
    pub fn dimension(&self) -> usize {
        self.metadata.dimension
    }

    /// Check if progressive search is enabled.
    pub fn is_progressive_search_enabled(&self) -> bool {
        self.metadata
            .quantization_config
            .as_ref()
            .is_some_and(|qc| qc.progressive_search_enabled)
    }

    /// Get progressive quantization levels ordered by search priority.
    pub fn get_progressive_levels(&self) -> Option<&[QuantizationLevel]> {
        self.metadata
            .quantization_config
            .as_ref()
            .map(|qc| qc.progressive_levels.as_slice())
    }

    /// Get binary filter selectivity for progressive search.
    pub fn binary_filter_selectivity(&self) -> f32 {
        self.metadata
            .quantization_config
            .as_ref()
            .map_or(0.1, |qc| qc.binary_filter_selectivity)
    }

    /// Check if SIMD acceleration should be used.
    pub fn use_simd_acceleration(&self) -> bool {
        self.metadata
            .quantization_config
            .as_ref()
            .is_none_or(|qc| qc.enable_simd_acceleration)
    }

    /// Get the parsed quantization config.
    pub fn quantization_config(&self) -> Option<&ParsedQuantizationConfig> {
        self.metadata.quantization_config.as_ref()
    }

    /// Check if quantization is enabled (pre-computed).
    pub fn has_quantization(&self) -> bool {
        self.metadata.has_quantization
    }

    /// Get storage path (pre-computed from storage assignment).
    pub fn storage_path(&self) -> &str {
        &self.metadata.storage_path
    }

    /// Get storage strategy (pre-computed).
    pub fn storage_strategy(&self) -> StorageEngineStrategy {
        self.metadata.storage_strategy.clone()
    }

    /// Get performance tier hint (pre-computed).
    pub fn performance_tier(&self) -> PerformanceTier {
        self.metadata.performance_tier.clone()
    }

    /// Get collection size estimates (pre-computed).
    pub fn estimated_vector_count(&self) -> u64 {
        self.metadata.estimated_vector_count
    }

    /// Get estimated collection size in bytes (pre-computed).
    pub fn estimated_size_bytes(&self) -> u64 {
        self.metadata.estimated_size_bytes
    }

    /// Check if compression is enabled (pre-computed).
    pub fn compression_enabled(&self) -> bool {
        self.metadata.compression_enabled
    }

    /// Check if quantization is enabled (pre-computed).
    pub fn quantization_enabled(&self) -> bool {
        self.metadata.quantization_enabled
    }

    /// Get collection ID from the collection object directly.
    pub fn collection_id(&self) -> &str {
        &self.collection.id
    }

    /// Get storage URL from collection's storage assignment.
    pub fn storage_url(&self) -> Option<&str> {
        self.collection
            .storage_assignment
            .as_ref()
            .map(|sa| sa.base_location.as_str())
    }

    /// Get collection-specific storage path.
    pub fn collection_storage_path(&self) -> Option<String> {
        self.storage_url().map(|base| {
            proximadb_storage_common::storage_path::StoragePath::collection_data_path(
                base,
                self.collection_id(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rls_predicate_default_is_passthrough() {
        let pred = RlsRecordPredicate::default();
        assert!(
            pred.is_passthrough(),
            "default predicate must not filter anything"
        );
    }

    #[test]
    fn test_rls_predicate_tenant_id_not_passthrough() {
        let pred = RlsRecordPredicate {
            required_tenant_id: Some("acme".to_string()),
            required_principal: None,
        };
        assert!(!pred.is_passthrough());
        assert_eq!(pred.required_tenant_id.as_deref(), Some("acme"));
    }

    #[test]
    fn test_rls_predicate_principal_not_passthrough() {
        let pred = RlsRecordPredicate {
            required_tenant_id: None,
            required_principal: Some("alice".to_string()),
        };
        assert!(!pred.is_passthrough());
    }

    #[test]
    fn test_rls_predicate_both_set() {
        let pred = RlsRecordPredicate {
            required_tenant_id: Some("acme".to_string()),
            required_principal: Some("alice".to_string()),
        };
        assert!(!pred.is_passthrough());
        assert_eq!(pred.required_tenant_id.as_deref(), Some("acme"));
        assert_eq!(pred.required_principal.as_deref(), Some("alice"));
    }

    #[test]
    fn test_storage_query_metadata_default() {
        let metadata = StorageQueryMetadata::default();
        assert_eq!(metadata.collection_id, "");
        assert!(!metadata.use_axis_indexes);
        assert!(!metadata.has_quantization);
    }
}
