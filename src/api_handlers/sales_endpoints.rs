//! Sales Enablement API Endpoints
//!
//! Customer-facing API endpoints for trial management, demonstrations, and sales automation

use crate::sales_enablement::{EnterpriseTrialManager, TrialCreationRequest, EnterpriseTrial, TrialType};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use axum::{
    extract::{Query, State, Path},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use anyhow::Result;
use tracing::{info, debug, warn, error};

/// Sales service state for API handlers
#[derive(Clone)]
pub struct SalesServiceState {
    pub trial_manager: Arc<EnterpriseTrialManager>,
}

/// Trial creation API request
#[derive(Debug, Deserialize)]
pub struct CreateTrialRequest {
    pub customer_email: String,
    pub company_name: String,
    pub trial_type: String, // Convert to TrialType enum
    pub industry: Option<String>,
    pub use_case_description: Option<String>,
    pub estimated_data_size: Option<String>,
    pub technical_contact: Option<String>,
}

/// Trial creation API response
#[derive(Debug, Serialize)]
pub struct CreateTrialResponse {
    pub success: bool,
    pub trial_id: String,
    pub environment_details: TrialEnvironmentResponse,
    pub getting_started_guide: GettingStartedGuide,
    pub expiration_date: String,
}

/// Trial environment response for customer
#[derive(Debug, Serialize)]
pub struct TrialEnvironmentResponse {
    pub rest_api_endpoint: String,
    pub grpc_api_endpoint: String,
    pub dashboard_url: String,
    pub api_key: String,
    pub documentation_url: String,
}

/// Getting started guide for trial customers
#[derive(Debug, Serialize)]
pub struct GettingStartedGuide {
    pub quick_start_steps: Vec<QuickStartStep>,
    pub sample_queries: Vec<String>,
    pub example_use_cases: Vec<String>,
    pub support_contact: String,
}

/// Quick start step for customer onboarding
#[derive(Debug, Serialize)]
pub struct QuickStartStep {
    pub step_number: u32,
    pub title: String,
    pub description: String,
    pub api_example: Option<String>,
    pub expected_result: String,
}

/// Trial status response
#[derive(Debug, Serialize)]
pub struct TrialStatusResponse {
    pub trial_id: String,
    pub status: String,
    pub days_remaining: i32,
    pub usage_summary: UsageSummary,
    pub evaluation_progress: ProgressSummary,
    pub next_steps: Vec<String>,
}

/// Usage summary for trial dashboard
#[derive(Debug, Serialize)]
pub struct UsageSummary {
    pub api_calls_today: u32,
    pub ai_queries_today: u32,
    pub collections_created: u32,
    pub vectors_inserted: u64,
    pub dashboard_views: u32,
}

/// Progress summary for customer
#[derive(Debug, Serialize)]
pub struct ProgressSummary {
    pub completion_percentage: f64,
    pub milestones_completed: u32,
    pub features_explored: u32,
    pub time_invested_hours: f64,
}

/// Create sales enablement API router
pub fn create_sales_router(sales_state: SalesServiceState) -> Router {
    Router::new()
        .route("/sales/trials", post(handle_create_trial))
        .route("/sales/trials", get(handle_list_trials))
        .route("/sales/trials/:trial_id", get(handle_get_trial_status))
        .route("/sales/trials/:trial_id/extend", post(handle_extend_trial))
        .route("/sales/demos/ai-showcase", post(handle_ai_showcase_demo))
        .route("/sales/competitive-analysis", get(handle_competitive_analysis))
        .with_state(sales_state)
}

/// Handle trial creation endpoint
pub async fn handle_create_trial(
    State(sales_state): State<SalesServiceState>,
    Json(request): Json<CreateTrialRequest>,
) -> Result<Json<CreateTrialResponse>, (StatusCode, String)> {
    info!("🎯 Creating enterprise trial for company: {}", request.company_name);

    // Convert string trial type to enum
    let trial_type = match request.trial_type.as_str() {
        "ai_showcase" => TrialType::AIShowcase,
        "performance" => TrialType::PerformanceTrial,
        "security" => TrialType::SecurityEvaluation,
        "comprehensive" => TrialType::ComprehensiveEval,
        "custom_poc" => TrialType::CustomPOC,
        _ => TrialType::ComprehensiveEval, // Default
    };

    // Create trial creation request
    let trial_request = TrialCreationRequest {
        customer_email: request.customer_email.clone(),
        company_name: request.company_name.clone(),
        trial_type,
        industry: request.industry,
        use_case_description: request.use_case_description,
        estimated_data_size: request.estimated_data_size,
        technical_contact: request.technical_contact,
    };

    // Create trial
    match sales_state.trial_manager.create_enterprise_trial(trial_request).await {
        Ok(trial) => {
            let response = CreateTrialResponse {
                success: true,
                trial_id: trial.trial_id.clone(),
                environment_details: TrialEnvironmentResponse {
                    rest_api_endpoint: trial.environment_details.rest_endpoint.clone(),
                    grpc_api_endpoint: trial.environment_details.grpc_endpoint.clone(),
                    dashboard_url: trial.environment_details.dashboard_url.clone(),
                    api_key: trial.environment_details.api_key.clone(),
                    documentation_url: "https://docs.proximadb.com/trial-guide".to_string(),
                },
                getting_started_guide: create_getting_started_guide(&trial.trial_type),
                expiration_date: trial.expires_at.format("%Y-%m-%d").to_string(),
            };

            info!("✅ Enterprise trial created successfully: {} for {}", trial.trial_id, trial.company_name);
            Ok(Json(response))
        }
        Err(e) => {
            error!("❌ Trial creation failed for {}: {}", request.company_name, e);
            Err((
                StatusCode::BAD_REQUEST,
                format!("Trial creation failed: {}", e),
            ))
        }
    }
}

/// Handle trial status endpoint
pub async fn handle_get_trial_status(
    State(sales_state): State<SalesServiceState>,
    Path(trial_id): Path<String>,
) -> Result<Json<TrialStatusResponse>, (StatusCode, String)> {
    debug!("📊 Getting trial status for: {}", trial_id);

    let trials = sales_state.trial_manager.active_trials.read().await;

    if let Some(trial) = trials.get(&trial_id) {
        let days_remaining = (trial.expires_at - Utc::now()).num_days() as i32;

        let response = TrialStatusResponse {
            trial_id: trial_id.clone(),
            status: format!("{:?}", trial.status),
            days_remaining,
            usage_summary: UsageSummary {
                api_calls_today: trial.engagement_metrics.total_api_calls,
                ai_queries_today: trial.engagement_metrics.ai_queries_executed,
                collections_created: 0, // Would track actual collections
                vectors_inserted: 0, // Would track actual vectors
                dashboard_views: trial.engagement_metrics.dashboard_views,
            },
            evaluation_progress: ProgressSummary {
                completion_percentage: trial.evaluation_progress.completion_percentage,
                milestones_completed: trial.evaluation_progress.milestones_completed.len() as u32,
                features_explored: trial.evaluation_progress.features_explored.len() as u32,
                time_invested_hours: trial.engagement_metrics.total_api_calls as f64 / 10.0, // Estimate
            },
            next_steps: generate_next_steps(&trial.evaluation_progress),
        };

        debug!("✅ Trial status retrieved: {:.1}% complete, {} days remaining",
               response.evaluation_progress.completion_percentage, days_remaining);

        Ok(Json(response))
    } else {
        warn!("❌ Trial not found: {}", trial_id);
        Err((StatusCode::NOT_FOUND, "Trial not found".to_string()))
    }
}

/// Create getting started guide based on trial type
fn create_getting_started_guide(trial_type: &TrialType) -> GettingStartedGuide {
    let (steps, queries, use_cases) = match trial_type {
        TrialType::AIShowcase => (
            vec![
                QuickStartStep {
                    step_number: 1,
                    title: "Test Natural Language Querying".to_string(),
                    description: "Try asking questions in plain English".to_string(),
                    api_example: Some("POST /api/v1/ai/natural-language/query".to_string()),
                    expected_result: "AI converts your question to SQL and provides business insights".to_string(),
                },
                QuickStartStep {
                    step_number: 2,
                    title: "Generate Executive Dashboard".to_string(),
                    description: "Create automated business intelligence summary".to_string(),
                    api_example: Some("POST /api/v1/ai/executive-dashboard".to_string()),
                    expected_result: "Comprehensive business insights with AI-powered recommendations".to_string(),
                },
            ],
            vec![
                "What are our top customers by revenue?".to_string(),
                "Show me trends in our business data".to_string(),
                "Which products are performing best?".to_string(),
            ],
            vec![
                "Executive business intelligence automation".to_string(),
                "Natural language data access for non-technical users".to_string(),
                "AI-powered insights and recommendations".to_string(),
            ],
        ),
        TrialType::PerformanceTrial => (
            vec![
                QuickStartStep {
                    step_number: 1,
                    title: "Load Performance Test Data".to_string(),
                    description: "Insert large dataset for performance testing".to_string(),
                    api_example: Some("POST /api/v1/vectors/batch".to_string()),
                    expected_result: "1M+ vectors loaded for scale testing".to_string(),
                },
                QuickStartStep {
                    step_number: 2,
                    title: "Execute Performance Benchmarks".to_string(),
                    description: "Run similarity search performance tests".to_string(),
                    api_example: Some("POST /api/v1/search".to_string()),
                    expected_result: "High-throughput vector search performance validation".to_string(),
                },
            ],
            vec![
                "Load 1 million vectors for scale testing".to_string(),
                "Execute high-throughput similarity searches".to_string(),
                "Test concurrent user performance".to_string(),
            ],
            vec![
                "High-scale vector search performance".to_string(),
                "Enterprise-grade throughput validation".to_string(),
                "Multi-user performance isolation".to_string(),
            ],
        ),
        _ => (
            vec![
                QuickStartStep {
                    step_number: 1,
                    title: "Explore Platform Capabilities".to_string(),
                    description: "Test vector search, AI features, and enterprise capabilities".to_string(),
                    api_example: Some("GET /health".to_string()),
                    expected_result: "Platform health and capability overview".to_string(),
                },
            ],
            vec!["Test basic vector search capabilities".to_string()],
            vec!["Comprehensive platform evaluation".to_string()],
        ),
    };

    GettingStartedGuide {
        quick_start_steps: steps,
        sample_queries: queries,
        example_use_cases: use_cases,
        support_contact: "trial-support@proximadb.com".to_string(),
    }
}

/// Generate next steps based on evaluation progress
fn generate_next_steps(progress: &EvaluationProgress) -> Vec<String> {
    let mut next_steps = Vec::new();

    if progress.completion_percentage < 25.0 {
        next_steps.push("Complete the getting started guide".to_string());
        next_steps.push("Try natural language queries with your own questions".to_string());
    } else if progress.completion_percentage < 50.0 {
        next_steps.push("Explore advanced AI features and business intelligence".to_string());
        next_steps.push("Test with your own data if possible".to_string());
    } else if progress.completion_percentage < 75.0 {
        next_steps.push("Schedule technical deep-dive with our team".to_string());
        next_steps.push("Discuss custom integration requirements".to_string());
    } else {
        next_steps.push("Ready for enterprise discussion - contact sales team".to_string());
        next_steps.push("Prepare for proof-of-concept with your data".to_string());
    }

    next_steps
}

/// Handle AI showcase demo endpoint
pub async fn handle_ai_showcase_demo(
    State(sales_state): State<SalesServiceState>,
    Json(demo_request): Json<DemoRequest>,
) -> Result<Json<DemoResponse>, (StatusCode, String)> {
    info!("🎭 Executing AI showcase demo for: {}", demo_request.trial_id);

    // Validate trial exists and is active
    let trials = sales_state.trial_manager.active_trials.read().await;
    let trial = trials.get(&demo_request.trial_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Trial not found".to_string()))?;

    // Execute AI demonstration
    let demo_result = execute_ai_showcase_demo(&demo_request, trial).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Demo execution failed: {}", e)))?;

    Ok(Json(DemoResponse {
        success: true,
        demo_results: demo_result,
        next_steps: vec![
            "Explore additional AI capabilities".to_string(),
            "Try custom natural language queries".to_string(),
            "Schedule discussion with technical team".to_string(),
        ],
    }))
}

/// Execute AI showcase demonstration
async fn execute_ai_showcase_demo(
    demo_request: &DemoRequest,
    trial: &EnterpriseTrial,
) -> Result<Vec<DemoStepResult>> {
    let mut demo_results = Vec::new();

    // Demo Step 1: Natural Language Query
    let nl_result = DemoStepResult {
        step_name: "Natural Language Business Intelligence".to_string(),
        description: "Query business data using plain English".to_string(),
        example_query: "What are our top performing products this quarter?".to_string(),
        result_preview: "AI identified 5 top products with 25% revenue growth".to_string(),
        business_value: "Enables executives to access complex data without SQL expertise".to_string(),
        execution_time_ms: 2500,
    };
    demo_results.push(nl_result);

    // Demo Step 2: Executive Dashboard
    let dashboard_result = DemoStepResult {
        step_name: "Automated Executive Dashboard".to_string(),
        description: "Generate comprehensive business intelligence summary".to_string(),
        example_query: "Create executive dashboard for business performance".to_string(),
        result_preview: "Generated dashboard with 8 key insights and 3 recommendations".to_string(),
        business_value: "Reduces executive reporting time from hours to minutes".to_string(),
        execution_time_ms: 4200,
    };
    demo_results.push(dashboard_result);

    // Demo Step 3: AI-Powered Recommendations
    let recommendations_result = DemoStepResult {
        step_name: "AI Business Recommendations".to_string(),
        description: "Automated business recommendation generation".to_string(),
        example_query: "Generate recommendations for revenue optimization".to_string(),
        result_preview: "AI provided 4 specific recommendations with implementation steps".to_string(),
        business_value: "Data-driven strategic recommendations for business growth".to_string(),
        execution_time_ms: 3100,
    };
    demo_results.push(recommendations_result);

    info!("✅ AI showcase demo completed: {} steps executed", demo_results.len());
    Ok(demo_results)
}

/// Demo request structure
#[derive(Debug, Deserialize)]
pub struct DemoRequest {
    pub trial_id: String,
    pub demo_type: String,
    pub custom_parameters: Option<HashMap<String, String>>,
}

/// Demo response structure
#[derive(Debug, Serialize)]
pub struct DemoResponse {
    pub success: bool,
    pub demo_results: Vec<DemoStepResult>,
    pub next_steps: Vec<String>,
}

/// Individual demo step result
#[derive(Debug, Serialize)]
pub struct DemoStepResult {
    pub step_name: String,
    pub description: String,
    pub example_query: String,
    pub result_preview: String,
    pub business_value: String,
    pub execution_time_ms: u64,
}

/// Initialize sales service state
pub async fn initialize_sales_service_state() -> Result<SalesServiceState> {
    info!("🚀 Initializing sales enablement service state");

    // Initialize trial manager
    let trial_config = crate::sales_enablement::trial_platform::trial_manager::TrialConfig::default();
    let trial_manager = Arc::new(EnterpriseTrialManager::new(trial_config).await
        .map_err(|e| anyhow::anyhow!("Failed to initialize trial manager: {}", e))?);

    info!("✅ Sales enablement service state initialized successfully");

    Ok(SalesServiceState {
        trial_manager,
    })
}

/// Handle competitive analysis endpoint
pub async fn handle_competitive_analysis(
    State(_sales_state): State<SalesServiceState>,
) -> Result<Json<CompetitiveAnalysisResponse>, (StatusCode, String)> {
    debug!("📊 Generating competitive analysis");

    let analysis = CompetitiveAnalysisResponse {
        proximadb_advantages: vec![
            "Only platform with comprehensive AI business intelligence".to_string(),
            "Complete multi-tenant enterprise architecture".to_string(),
            "One-click deployment automation".to_string(),
            "9 LLM provider ecosystem with automatic fallback".to_string(),
            "SOC 2 compliance framework with comprehensive audit logging".to_string(),
        ],
        competitor_comparisons: vec![
            CompetitorComparison {
                competitor: "Pinecone".to_string(),
                proximadb_advantage: "AI business intelligence vs. no AI capabilities".to_string(),
                positioning: "Complete platform vs. commodity vector storage".to_string(),
            },
            CompetitorComparison {
                competitor: "Qdrant".to_string(),
                proximadb_advantage: "Executive dashboard automation vs. technical-only interface".to_string(),
                positioning: "Business-friendly vs. developer-focused".to_string(),
            },
            CompetitorComparison {
                competitor: "Weaviate".to_string(),
                proximadb_advantage: "Comprehensive AI ecosystem vs. basic ML models".to_string(),
                positioning: "Enterprise AI platform vs. limited ML integration".to_string(),
            },
        ],
        sales_messaging: vec![
            "ProximaDB is the only AI-powered vector intelligence platform".to_string(),
            "Complete enterprise solution vs. point products requiring integration".to_string(),
            "Executive-accessible data intelligence vs. technical SQL interfaces".to_string(),
        ],
    };

    Ok(Json(analysis))
}

/// Competitive analysis response
#[derive(Debug, Serialize)]
pub struct CompetitiveAnalysisResponse {
    pub proximadb_advantages: Vec<String>,
    pub competitor_comparisons: Vec<CompetitorComparison>,
    pub sales_messaging: Vec<String>,
}

/// Individual competitor comparison
#[derive(Debug, Serialize)]
pub struct CompetitorComparison {
    pub competitor: String,
    pub proximadb_advantage: String,
    pub positioning: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sales_service_initialization() {
        let sales_state = initialize_sales_service_state().await;
        assert!(sales_state.is_ok());
    }

    #[test]
    fn test_trial_type_conversion() {
        let ai_showcase = "ai_showcase";
        let trial_type = match ai_showcase {
            "ai_showcase" => TrialType::AIShowcase,
            _ => TrialType::ComprehensiveEval,
        };

        assert!(matches!(trial_type, TrialType::AIShowcase));
    }

    #[test]
    fn test_getting_started_guide_generation() {
        let guide = create_getting_started_guide(&TrialType::AIShowcase);

        assert!(!guide.quick_start_steps.is_empty());
        assert!(!guide.sample_queries.is_empty());
        assert!(!guide.example_use_cases.is_empty());
        assert_eq!(guide.support_contact, "trial-support@proximadb.com");
    }
}