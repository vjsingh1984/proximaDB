//! REST API Handler for Progressive Quantization-Aware Search

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::core::search::{
    progressive_orchestrator::ProgressiveSearchOrchestrator,
    progressive_quantization::{ProgressiveSearchConfig, SearchScenario},
    SearchParams, FilterExpression,
};
use crate::compute::distance_computation::DistanceMetric;

/// Request for progressive search
#[derive(Debug, Deserialize)]
pub struct ProgressiveSearchRequest {
    /// Query vector
    pub vector: Vec<f32>,
    
    /// Number of results to return
    pub k: usize,
    
    /// Optional filter expression
    pub filter: Option<FilterExpression>,
    
    /// Distance metric override
    pub distance_metric: Option<String>,
    
    /// Search scenario (high_recall, balanced, high_speed, low_memory)
    pub scenario: Option<String>,
    
    /// Enable adaptive recall tuning
    pub adaptive_recall: Option<bool>,
    
    /// Custom recall rates
    pub custom_recalls: Option<CustomRecalls>,
    
    /// Include vectors in response
    pub include_vectors: Option<bool>,
    
    /// Include metadata in response
    pub include_metadata: Option<bool>,
    
    /// Return stage metrics
    pub include_metrics: Option<bool>,
}

/// Custom recall rates for fine control
#[derive(Debug, Deserialize)]
pub struct CustomRecalls {
    pub binary_recall: Option<f32>,
    pub int8_recall: Option<f32>,
    pub pq_recall: Option<f32>,
}

/// Response for progressive search
#[derive(Debug, Serialize)]
pub struct ProgressiveSearchResponse {
    /// Search results
    pub results: Vec<SearchResultDto>,
    
    /// Total search time in milliseconds
    pub search_time_ms: f64,
    
    /// Stage metrics if requested
    pub metrics: Option<StageMetrics>,
    
    /// Effective configuration used
    pub config_used: Option<ConfigUsed>,
}

/// Search result DTO
#[derive(Debug, Serialize)]
pub struct SearchResultDto {
    pub id: String,
    pub score: f32,
    pub similarity: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Stage metrics for analysis
#[derive(Debug, Serialize)]
pub struct StageMetrics {
    pub binary_stage: StageInfo,
    pub int8_stage: StageInfo,
    pub pq_stage: StageInfo,
    pub fp32_stage: StageInfo,
    pub total_candidates_evaluated: usize,
    pub speedup_vs_brute_force: f64,
}

#[derive(Debug, Serialize)]
pub struct StageInfo {
    pub candidates: usize,
    pub time_ms: f64,
    pub recall_rate: Option<f32>,
}

/// Configuration actually used
#[derive(Debug, Serialize)]
pub struct ConfigUsed {
    pub scenario: String,
    pub binary_recall: f32,
    pub int8_recall: f32,
    pub pq_recall: f32,
    pub max_expansion_factor: f32,
}

/// Handler for progressive search endpoint
pub async fn progressive_search_handler(
    Path(collection_id): Path<String>,
    Query(params): Query<ProgressiveSearchRequest>,
    State(orchestrator): State<Arc<ProgressiveSearchOrchestrator>>,
) -> Result<Json<ProgressiveSearchResponse>, StatusCode> {
    let start_time = std::time::Instant::now();
    
    info!(
        "Progressive search request for collection {} with k={}",
        collection_id, params.k
    );
    
    // Configure progressive search
    let mut config = if let Some(scenario_str) = params.scenario.as_ref() {
        match scenario_str.as_str() {
            "high_recall" => ProgressiveSearchConfig::for_scenario(SearchScenario::HighRecall),
            "high_speed" => ProgressiveSearchConfig::for_scenario(SearchScenario::HighSpeed),
            "low_memory" => ProgressiveSearchConfig::for_scenario(SearchScenario::LowMemory),
            _ => ProgressiveSearchConfig::default(),
        }
    } else {
        ProgressiveSearchConfig::default()
    };
    
    // Apply custom recall rates if provided
    if let Some(custom) = params.custom_recalls {
        if let Some(binary) = custom.binary_recall {
            config.binary_recall = binary.clamp(0.5, 1.0);
        }
        if let Some(int8) = custom.int8_recall {
            config.int8_recall = int8.clamp(0.5, 1.0);
        }
        if let Some(pq) = custom.pq_recall {
            config.pq_recall = pq.clamp(0.5, 1.0);
        }
    }
    
    // Enable adaptive recall if requested
    if let Some(adaptive) = params.adaptive_recall {
        config.adaptive_recall = adaptive;
    }
    
    // Create search parameters
    let search_params = SearchParams {
        top_k: Some(params.k),
        filters: Default::default(),
        accuracy_threshold: None,
        include_expired: Some(false),
        timeout_ms: Some(30000), // 30 second timeout
        enable_two_stage: Some(true),
        enable_clustering_hint: Some(true),
        enable_metadata_filtering_hint: params.filter.is_some().into(),
        quantization_hint: None,
        custom_hints: Default::default(),
        optimization_hint: params.scenario,
    };
    
    // Execute progressive search
    match orchestrator.search(
        &collection_id,
        &params.vector,
        params.k,
        &search_params,
        params.filter.as_ref(),
    ).await {
        Ok(results) => {
            let search_time_ms = start_time.elapsed().as_secs_f64() * 1000.0;
            
            // Convert results to DTOs
            let result_dtos: Vec<SearchResultDto> = results
                .into_iter()
                .map(|r| SearchResultDto {
                    id: r.id,
                    score: r.score,
                    similarity: r.similarity,
                    vector: if params.include_vectors {
                        r.vector
                    } else {
                        None
                    },
                    metadata: if params.include_metadata {
                        r.metadata.iter().map(|m| serde_json::to_value(m).unwrap_or_default())
                    } else {
                        None
                    },
                })
                .collect();
            
            // Prepare metrics if requested
            let metrics = if params.include_metrics {
                Some(StageMetrics {
                    binary_stage: StageInfo {
                        candidates: config.compute_stage_sizes(params.k).binary_candidates,
                        time_ms: 0.0, // Would be tracked by orchestrator
                        recall_rate: Some(config.binary_recall),
                    },
                    int8_stage: StageInfo {
                        candidates: config.compute_stage_sizes(params.k).int8_candidates,
                        time_ms: 0.0,
                        recall_rate: Some(config.int8_recall),
                    },
                    pq_stage: StageInfo {
                        candidates: config.compute_stage_sizes(params.k).pq_candidates,
                        time_ms: 0.0,
                        recall_rate: Some(config.pq_recall),
                    },
                    fp32_stage: StageInfo {
                        candidates: params.k,
                        time_ms: 0.0,
                        recall_rate: Some(1.0),
                    },
                    total_candidates_evaluated: config.compute_stage_sizes(params.k).total_computations,
                    speedup_vs_brute_force: 0.0, // Would be calculated based on collection size
                })
            } else {
                None
            };
            
            // Prepare config used
            let config_used = Some(ConfigUsed {
                scenario: params.scenario.unwrap_or_else(|| "balanced".to_string()),
                binary_recall: config.binary_recall,
                int8_recall: config.int8_recall,
                pq_recall: config.pq_recall,
                max_expansion_factor: config.max_expansion_factor,
            });
            
            info!(
                "Progressive search completed in {:.2}ms with {} results",
                search_time_ms,
                result_dtos.len()
            );
            
            Ok(Json(ProgressiveSearchResponse {
                results: result_dtos,
                search_time_ms,
                metrics,
                config_used,
            }))
        }
        Err(e) => {
            error!("Progressive search failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Handler for explaining progressive search plan
pub async fn explain_progressive_search_handler(
    Path(collection_id): Path<String>,
    Query(params): Query<ExplainRequest>,
    State(orchestrator): State<Arc<ProgressiveSearchOrchestrator>>,
) -> Result<Json<ExplainResponse>, StatusCode> {
    let config = if let Some(scenario_str) = params.scenario.as_ref() {
        match scenario_str.as_str() {
            "high_recall" => ProgressiveSearchConfig::for_scenario(SearchScenario::HighRecall),
            "high_speed" => ProgressiveSearchConfig::for_scenario(SearchScenario::HighSpeed),
            "low_memory" => ProgressiveSearchConfig::for_scenario(SearchScenario::LowMemory),
            _ => ProgressiveSearchConfig::default(),
        }
    } else {
        ProgressiveSearchConfig::default()
    };
    
    let k = params.k;
    let stage_sizes = config.compute_stage_sizes(k);
    
    Ok(Json(ExplainResponse {
        collection_id,
        k,
        scenario: params.scenario.unwrap_or_else(|| "balanced".to_string()),
        stages: vec![
            ExplainStage {
                name: "Binary".to_string(),
                candidates: stage_sizes.binary_candidates,
                recall_rate: config.binary_recall,
                expansion_factor: stage_sizes.binary_candidates as f32 / k as f32,
                description: "Ultra-fast binary quantization for initial filtering".to_string(),
            },
            ExplainStage {
                name: "INT8".to_string(),
                candidates: stage_sizes.int8_candidates,
                recall_rate: config.int8_recall,
                expansion_factor: stage_sizes.int8_candidates as f32 / k as f32,
                description: "8-bit integer quantization for improved accuracy".to_string(),
            },
            ExplainStage {
                name: "PQ".to_string(),
                candidates: stage_sizes.pq_candidates,
                recall_rate: config.pq_recall,
                expansion_factor: stage_sizes.pq_candidates as f32 / k as f32,
                description: "Product quantization for near-lossless compression".to_string(),
            },
            ExplainStage {
                name: "FP32".to_string(),
                candidates: k,
                recall_rate: 1.0,
                expansion_factor: 1.0,
                description: "Full precision for final ranking".to_string(),
            },
        ],
        total_computations: stage_sizes.total_computations,
        effective_expansion: stage_sizes.effective_expansion,
        estimated_speedup: (1_000_000.0 / stage_sizes.total_computations as f32).round(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct ExplainRequest {
    pub k: Option<usize>,
    pub scenario: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExplainResponse {
    pub collection_id: String,
    pub k: usize,
    pub scenario: String,
    pub stages: Vec<ExplainStage>,
    pub total_computations: usize,
    pub effective_expansion: f32,
    pub estimated_speedup: f32,
}

#[derive(Debug, Serialize)]
pub struct ExplainStage {
    pub name: String,
    pub candidates: usize,
    pub recall_rate: f32,
    pub expansion_factor: f32,
    pub description: String,
}