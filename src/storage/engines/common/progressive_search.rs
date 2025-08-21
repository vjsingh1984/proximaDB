//! Common Progressive Search Implementation
//!
//! This module provides shared progressive search logic that can be used by all storage engines
//! (SST, VIPER, NOVA, SWIFT, etc.) to implement multi-stage quantization-aware search.
//!
//! ## FLEXIBLE QUANTIZATION ARCHITECTURE (2025-08-21):
//! 
//! **Two supported paths based on use case:**
//! 
//! 1. **HIGH PERFORMANCE PATH** (Write-once, Read-many):
//!    - Collection config has quantization enabled
//!    - Write Path: FP32 → [Binary + INT8 + PQ8] → Store ALL quantized versions
//!    - Read Path: Query → Search pre-stored quantized → Fast response
//!    - Use case: Static datasets with frequent searches
//! 
//! 2. **STORAGE OPTIMIZED PATH** (Continuous writes, Infrequent reads):
//!    - Collection config has quantization disabled
//!    - Write Path: FP32 → Store only FP32 (save storage)
//!    - Read Path: Query → Runtime quantization → Slower but acceptable
//!    - Use case: Streaming data where storage cost matters more than latency
//! 
//! The unified query optimizer and search hints determine which path to use.

use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{debug, info, trace};

use crate::core::search::SearchResult;
use crate::proto::proximadb::VectorRecord;
use crate::storage::traits::{SearchContext, QuantizationType, QuantizationLevel};
use crate::compute::quantization::unified::{QuantizedVector, UnifiedQuantizationEngine};
use crate::compute::distance_computation::core::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;

/// Progressive search executor that can be used by any storage engine
pub struct ProgressiveSearchExecutor {
    /// Quantization engine for vector operations
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    
    /// Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
}

/// Candidate tracking during progressive search
#[derive(Debug, Clone)]
pub struct SearchCandidate {
    /// Vector ID
    pub id: String,
    
    /// Full precision vector (loaded on demand)
    pub vector: Option<Vec<f32>>,
    
    /// Quantized representations at different levels
    pub quantized_vectors: Vec<QuantizedRepresentation>,
    
    /// Current score/distance
    pub score: f32,
    
    /// Stage where this candidate was added
    pub stage: SearchStage,
    
    /// Metadata (optional)
    pub metadata: Option<Vec<u8>>,
}

/// Quantized representation at a specific level
#[derive(Debug, Clone)]
pub struct QuantizedRepresentation {
    /// Level identifier
    pub level_id: String,
    
    /// Quantized data
    pub data: Vec<u8>,
    
    /// Quantization type
    pub quant_type: QuantizationType,
}

/// Search stage for tracking
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SearchStage {
    BinaryFilter,
    Int8Ranking,
    PqRanking,
    FullPrecision,
}

impl ProgressiveSearchExecutor {
    /// Create a new progressive search executor
    pub fn new(
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        distance_compute: Arc<UnifiedDistanceCompute>,
    ) -> Self {
        Self {
            quantization_engine,
            distance_compute,
        }
    }
    
    /// Execute progressive search with the given context and candidates
    pub async fn execute_progressive_search(
        &self,
        ctx: &SearchContext,
        initial_candidates: Vec<VectorRecord>,
        query_vector: &[f32],
    ) -> Result<Vec<SearchResult>> {
        // Check if progressive search is enabled
        if !ctx.is_progressive_search_enabled() {
            debug!("Progressive search not enabled, falling back to full precision search");
            return self.full_precision_search(ctx, initial_candidates, query_vector).await;
        }
        
        // Get progressive levels
        let levels = ctx.get_progressive_levels()
            .ok_or_else(|| anyhow::anyhow!("No progressive levels configured"))?;
        
        if levels.is_empty() {
            debug!("No progressive levels defined, using full precision");
            return self.full_precision_search(ctx, initial_candidates, query_vector).await;
        }
        
        info!("🔄 Starting progressive search with {} levels for {} candidates", 
            levels.len(), initial_candidates.len());
        
        // Convert to search candidates
        let mut candidates = self.prepare_candidates(ctx, initial_candidates, levels)?;
        
        // Execute progressive stages
        for (stage_idx, level) in levels.iter().enumerate() {
            let stage = self.get_search_stage(&level.quantization_type);
            
            debug!("📊 Stage {}: {} ({:?}) - {} candidates", 
                stage_idx, level.level_id, level.quantization_type, candidates.len());
            
            // Apply progressive filter/ranking
            candidates = self.apply_progressive_stage(
                ctx,
                candidates,
                query_vector,
                level,
                stage,
            ).await?;
            
            // Check if we have enough candidates
            if candidates.len() <= ctx.top_k() {
                debug!("Early termination: candidates ({}) <= top_k ({})", 
                    candidates.len(), ctx.top_k());
                break;
            }
        }
        
        // Final reranking with full precision if needed
        if candidates.iter().any(|c| c.stage != SearchStage::FullPrecision) {
            candidates = self.final_rerank(ctx, candidates, query_vector).await?;
        }
        
        // Convert to search results
        self.convert_to_results(candidates, ctx.top_k())
    }
    
    /// Prepare candidates using PRE-STORED quantized representations (no re-quantization!)
    fn prepare_candidates(
        &self,
        ctx: &SearchContext,
        records: Vec<VectorRecord>,
        levels: &[QuantizationLevel],
    ) -> Result<Vec<SearchCandidate>> {
        let mut candidates = Vec::with_capacity(records.len());
        
        for record in records {
            let quantized_vectors = if let Some(quant_data) = &record.quantized_vector {
                // FAST PATH: Use pre-stored quantized vectors (write-time quantization)
                self.parse_quantized_data(quant_data, levels)?
            } else {
                // Check if runtime quantization should be allowed based on:
                // 1. Collection configuration
                // 2. Search hints
                // 3. Unified query optimizer recommendations
                
                let should_runtime_quantize = self.should_allow_runtime_quantization(ctx)?;
                
                if should_runtime_quantize {
                    // SLOW PATH: Runtime quantization for storage-optimized collections
                    debug!("⚠️ Vector {} using runtime quantization (storage-optimized path)", 
                           record.id.as_ref().unwrap_or(&"unknown".to_string()));
                    self.quantization_engine.quantize_vector(&record.vector, levels)?
                } else {
                    // ERROR: Runtime quantization not allowed for this collection/query
                    debug!("❌ Vector {} missing pre-quantized data (collection expects pre-quantization)", 
                           record.id.as_ref().unwrap_or(&"unknown".to_string()));
                    return Err(anyhow::anyhow!(
                        "Missing pre-quantized data for vector {}. Collection config expects pre-quantization.",
                        record.id.as_ref().unwrap_or(&"unknown".to_string())
                    ));
                }
            };
            
            candidates.push(SearchCandidate {
                id: record.id.unwrap_or_default(),
                vector: Some(record.vector.clone()),
                quantized_vectors,
                score: f32::MAX,
                stage: SearchStage::BinaryFilter,
                metadata: None,
            });
        }
        
        Ok(candidates)
    }
    
    /// Apply a progressive search stage
    async fn apply_progressive_stage(
        &self,
        ctx: &SearchContext,
        mut candidates: Vec<SearchCandidate>,
        query_vector: &[f32],
        level: &QuantizationLevel,
        stage: SearchStage,
    ) -> Result<Vec<SearchCandidate>> {
        // Get selectivity for this stage
        let selectivity = match stage {
            SearchStage::BinaryFilter => ctx.binary_filter_selectivity(),
            SearchStage::Int8Ranking => ctx.metadata.quantization_config
                .as_ref()
                .map(|qc| qc.int8_ranking_selectivity)
                ,
            SearchStage::PqRanking => ctx.metadata.quantization_config
                .as_ref()
                .map(|qc| qc.pq_ranking_selectivity)
                ,
            SearchStage::FullPrecision => 1.0,
        };
        
        // Calculate how many candidates to keep
        let keep_count = ((candidates.len() as f32) * selectivity).ceil() as usize;
        let keep_count = keep_count.max(ctx.top_k()).min(candidates.len());
        
        trace!("Stage {:?}: keeping {} of {} candidates (selectivity: {})",
            stage, keep_count, candidates.len(), selectivity);
        
        // Score candidates based on quantization level
        match level.quantization_type {
            QuantizationType::Binary => {
                self.score_binary(&mut candidates, query_vector, level)?;
            },
            QuantizationType::Scalar => {
                self.score_scalar(&mut candidates, query_vector, level)?;
            },
            QuantizationType::Product => {
                self.score_product(&mut candidates, query_vector, level)?;
            },
            QuantizationType::None => {
                self.score_full_precision(&mut candidates, query_vector, ctx.distance_metric())?;
            },
            _ => {
                debug!("Unsupported quantization type: {:?}", level.quantization_type);
            }
        }
        
        // Sort by score (ascending for distance, descending for similarity)
        candidates.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        
        // Keep top candidates
        candidates.truncate(keep_count);
        
        // Update stage for kept candidates
        for candidate in &mut candidates {
            candidate.stage = stage;
        }
        
        Ok(candidates)
    }
    
    /// Score candidates using binary quantization (delegates to unified quantization)
    fn score_binary(
        &self,
        candidates: &mut [SearchCandidate],
        query_vector: &[f32],
        level: &QuantizationLevel,
    ) -> Result<()> {
        // Delegate all quantization to UnifiedQuantizationEngine
        let query_quantized = self.quantization_engine.quantize_to_level(
            query_vector,
            &QuantizationType::Binary
        )?;
        
        for candidate in candidates {
            if let Some(binary_repr) = candidate.quantized_vectors
                .iter()
                .find(|qv| qv.level_id == level.level_id) 
            {
                // Delegate distance calculation to unified quantization engine
                // which internally uses SIMD-optimized distance computation
                let quantized_vec = QuantizedVector {
                    data: binary_repr.data.clone(),
                    quantization_level: level.quantization_level.clone(),
                    metadata: Default::default(),
                };
                let distance = self.quantization_engine.calculate_distance(
                    &query_quantized,
                    &quantized_vec,
                    &self.distance_compute.default_metric()
                ).await?;
                candidate.score = distance.raw_value;
            }
        }
        
        Ok(())
    }
    
    /// Score candidates using scalar quantization (INT8) - delegates to unified quantization
    fn score_scalar(
        &self,
        candidates: &mut [SearchCandidate],
        query_vector: &[f32],
        level: &QuantizationLevel,
    ) -> Result<()> {
        // Delegate all quantization and distance calculation to unified modules
        for candidate in candidates {
            if let Some(int8_repr) = candidate.quantized_vectors
                .iter()
                .find(|qv| qv.level_id == level.level_id)
            {
                // Delegate to unified quantization engine which uses SIMD distance computation
                let distance = self.quantization_engine.calculate_int8_distance_optimized(
                    query_vector,
                    &int8_repr.data,
                    &self.distance_compute.default_metric()
                )?;
                candidate.score = distance;
            }
        }
        
        Ok(())
    }
    
    /// Score candidates using product quantization - delegates to unified quantization
    fn score_product(
        &self,
        candidates: &mut [SearchCandidate],
        query_vector: &[f32],
        level: &QuantizationLevel,
    ) -> Result<()> {
        let num_subvectors = level.num_subvectors as usize;
        
        // Delegate PQ distance calculation to unified quantization engine
        for candidate in candidates {
            if let Some(pq_repr) = candidate.quantized_vectors
                .iter()
                .find(|qv| qv.level_id == level.level_id)
            {
                // Unified quantization engine handles PQ lookup tables and SIMD optimization
                let distance = self.quantization_engine.calculate_pq_distance_optimized(
                    query_vector,
                    &pq_repr.data,
                    num_subvectors,
                    level.bits,
                    &self.distance_compute.default_metric()
                )?;
                candidate.score = distance;
            }
        }
        
        Ok(())
    }
    
    /// Score candidates using full precision
    fn score_full_precision(
        &self,
        candidates: &mut [SearchCandidate],
        query_vector: &[f32],
        distance_metric: DistanceMetric,
    ) -> Result<()> {
        for candidate in candidates {
            if let Some(ref vector) = candidate.vector {
                let result = self.distance_compute.calculate_distance(
                    query_vector,
                    vector,
                    &distance_metric,
                );
                let distance = result.rank_value;
                candidate.score = distance;
            }
        }
        
        Ok(())
    }
    
    /// Final reranking with full precision
    async fn final_rerank(
        &self,
        ctx: &SearchContext,
        mut candidates: Vec<SearchCandidate>,
        query_vector: &[f32],
    ) -> Result<Vec<SearchCandidate>> {
        debug!("🎯 Final reranking {} candidates with full precision", candidates.len());
        
        self.score_full_precision(&mut candidates, query_vector, ctx.distance_metric())?;
        
        // Sort and truncate to top_k
        candidates.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        candidates.truncate(ctx.top_k());
        
        // Mark as full precision
        for candidate in &mut candidates {
            candidate.stage = SearchStage::FullPrecision;
        }
        
        Ok(candidates)
    }
    
    /// Fallback to full precision search
    async fn full_precision_search(
        &self,
        ctx: &SearchContext,
        records: Vec<VectorRecord>,
        query_vector: &[f32],
    ) -> Result<Vec<SearchResult>> {
        let mut results = Vec::with_capacity(records.len());
        
        for record in records {
            let result = self.distance_compute.calculate_distance(
                query_vector,
                &record.vector,
                &ctx.distance_metric(),
            );
            let distance = result.rank_value;
            
            results.push(SearchResult {
                id: record.id.unwrap_or_default(),
                score: distance,
                vector: Some(record.vector),
                metadata: record.metadata.into_iter()
                    .map(|item| (item.key, serde_json::Value::String(item.string_value.unwrap_or_default())))
                    .collect(),
                ..Default::default()
            });
        }
        
        // Sort and truncate
        results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        results.truncate(ctx.top_k());
        
        Ok(results)
    }
    
    /// Convert candidates to search results
    fn convert_to_results(
        &self,
        candidates: Vec<SearchCandidate>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        let mut results = Vec::with_capacity(top_k.min(candidates.len()));
        
        for candidate in candidates.into_iter().take(top_k) {
            results.push(SearchResult {
                id: candidate.id,
                score: candidate.score,
                vector: candidate.vector,
                metadata: Default::default(),
                ..Default::default()
            });
        }
        
        Ok(results)
    }
    
    /// Determine if runtime quantization should be allowed based on multiple factors
    fn should_allow_runtime_quantization(&self, ctx: &SearchContext) -> Result<bool> {
        // 1. Check collection configuration
        let collection_config = ctx.get_collection_config();
        let quantization_enabled = collection_config
            .and_then(|c| c.quantization_config.as_ref())
            .map(|qc| qc.enabled)
            .unwrap_or(false);
        
        if quantization_enabled {
            // Collection expects pre-quantized data for performance
            // Only allow runtime quantization if explicitly requested
            
            // 2. Check search hints
            if let Some(hints) = ctx.get_search_hints() {
                if hints.allow_runtime_quantization {
                    debug!("Runtime quantization allowed by search hints despite collection config");
                    return Ok(true);
                }
            }
            
            // 3. Check unified query optimizer recommendation
            if let Some(optimizer_rec) = ctx.get_optimizer_recommendation() {
                if optimizer_rec.suggests_runtime_quantization {
                    debug!("Runtime quantization recommended by query optimizer");
                    return Ok(true);
                }
            }
            
            // Collection has quantization enabled but no override - expect pre-quantized
            Ok(false)
        } else {
            // Collection doesn't have quantization enabled
            // This is the storage-optimized path - runtime quantization expected
            debug!("Collection configured for storage optimization - runtime quantization allowed");
            Ok(true)
        }
    }
    
    /// Helper: Parse pre-computed quantized data
    fn parse_quantized_data(
        &self,
        data: &[u8],
        levels: &[QuantizationLevel],
    ) -> Result<Vec<QuantizedRepresentation>> {
        // TODO: Implement parsing of serialized quantized data
        // For now, return empty vec
        Ok(Vec::new())
    }
    
    /// Helper: Quantize vector on-the-fly (for storage-optimized path)
    fn quantize_vector(
        &self,
        vector: &[f32],
        levels: &[QuantizationLevel],
    ) -> Result<Vec<QuantizedRepresentation>> {
        // This is used for the storage-optimized path where we trade latency for storage savings
        trace!("Runtime quantization for storage-optimized path");
        let mut representations = Vec::new();
        
        for level in levels {
            let data = match level.quantization_type {
                QuantizationType::Binary => {
                    self.quantization_engine.quantize_to_binary(vector)?
                },
                QuantizationType::Scalar => {
                    self.quantization_engine.quantize_to_int8(vector)?
                },
                QuantizationType::Product => {
                    self.quantization_engine.quantize_to_pq(
                        vector,
                        level.num_subvectors as usize,
                        level.bits,
                    )?
                },
                _ => Vec::new(),
            };
            
            representations.push(QuantizedRepresentation {
                level_id: level.level_id.clone(),
                data,
                quant_type: level.quantization_type.clone(),
            });
        }
        
        Ok(representations)
    }
    
    /// Helper: Get search stage from quantization type
    fn get_search_stage(&self, quant_type: &QuantizationType) -> SearchStage {
        match quant_type {
            QuantizationType::Binary => SearchStage::BinaryFilter,
            QuantizationType::Scalar => SearchStage::Int8Ranking,
            QuantizationType::Product => SearchStage::PqRanking,
            QuantizationType::None => SearchStage::FullPrecision,
            _ => SearchStage::FullPrecision,
        }
    }
    
    // NOTE: All distance computations are delegated to UnifiedQuantizationEngine
    // which internally uses UnifiedDistanceCompute with SIMD optimizations.
    // No manual distance calculations should be done here to maintain
    // proper separation of concerns and leverage hardware-optimized implementations.
    
}

/// Builder pattern for configuring progressive search
pub struct ProgressiveSearchBuilder {
    quantization_engine: Option<Arc<UnifiedQuantizationEngine>>,
    distance_compute: Option<Arc<UnifiedDistanceCompute>>,
}

impl ProgressiveSearchBuilder {
    pub fn new() -> Self {
        Self {
            quantization_engine: None,
            distance_compute: None,
        }
    }
    
    pub fn with_quantization_engine(mut self, engine: Arc<UnifiedQuantizationEngine>) -> Self {
        self.quantization_engine = Some(engine);
        self
    }
    
    pub fn with_distance_compute(mut self, compute: Arc<UnifiedDistanceCompute>) -> Self {
        self.distance_compute = Some(compute);
        self
    }
    
    pub fn build(self) -> Result<ProgressiveSearchExecutor> {
        Ok(ProgressiveSearchExecutor::new(
            self.quantization_engine
                .ok_or_else(|| anyhow::anyhow!("Quantization engine required"))?,
            self.distance_compute
                .ok_or_else(|| anyhow::anyhow!("Distance compute required"))?,
        ))
    }
}