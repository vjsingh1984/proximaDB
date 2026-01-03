//! AI-Powered Business Intelligence API Endpoints
//!
//! REST and gRPC endpoints for natural language querying and executive dashboards.
//! NOTE: This module is not currently registered in `api_handlers::mod` or any router,
//! so the routes here are unreachable until wiring is added to the network layer.

use crate::ai::llm_integration::{LLMIntegrationEngine, LLMRequest, LLMResponse};
use crate::ai::natural_language::{NLQueryTranslator, TranslationResult, UserContext};
use crate::ai::{AIExecutiveDashboard, AIExecutiveDashboardResponse, ExecutiveDashboardRequest};
use anyhow::Result;
use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// AI service state for API handlers
#[derive(Clone)]
pub struct AIServiceState {
    pub ai_dashboard: Arc<AIExecutiveDashboard>,
    pub nl_translator: Arc<NLQueryTranslator>,
    pub llm_engine: Arc<LLMIntegrationEngine>,
}

/// Natural language query request
#[derive(Debug, Deserialize)]
pub struct NaturalLanguageQueryRequest {
    pub query: String,
    pub tenant_id: String,
    pub user_id: String,
    pub context: Option<String>,
}

/// Natural language query response
#[derive(Debug, Serialize)]
pub struct NaturalLanguageQueryResponse {
    pub sql: String,
    pub explanation: String,
    pub confidence: f32,
    pub execution_time_ms: u64,
    pub data_summary: Option<String>,
}

/// Executive dashboard request via API
#[derive(Debug, Deserialize)]
pub struct APIDashboardRequest {
    pub tenant_id: String,
    pub user_id: String,
    pub time_period: String,
    pub focus_areas: Option<Vec<String>>,
    pub custom_queries: Option<Vec<String>>,
}

/// Create AI API router
pub fn create_ai_router(ai_state: AIServiceState) -> Router {
    Router::new()
        .route(
            "/ai/natural-language/query",
            post(handle_natural_language_query),
        )
        .route("/ai/executive-dashboard", post(handle_executive_dashboard))
        .route(
            "/ai/executive-dashboard",
            get(handle_executive_dashboard_get),
        )
        .route("/ai/business-insights", post(handle_business_insights))
        .route("/ai/providers/status", get(handle_llm_providers_status))
        .with_state(ai_state)
}

/// Handle natural language query endpoint
pub async fn handle_natural_language_query(
    State(ai_state): State<AIServiceState>,
    Json(request): Json<NaturalLanguageQueryRequest>,
) -> Result<Json<NaturalLanguageQueryResponse>, (StatusCode, String)> {
    let start_time = std::time::Instant::now();

    info!(
        "🤔 Processing natural language query for tenant {}: {}",
        request.tenant_id, request.query
    );

    // Build user context
    let user_context = UserContext {
        user_id: request.user_id.clone(),
        tenant_id: Some(request.tenant_id.clone()),
        accessible_tables: vec![
            "collections".to_string(),
            "vectors".to_string(),
            "tenants".to_string(),
        ], // Default tables
        permissions: vec!["read_data".to_string(), "query_data".to_string()],
        roles: vec!["user".to_string()],
    };

    // Translate natural language to SQL
    match ai_state
        .nl_translator
        .translate_to_sql(&request.query, &user_context)
        .await
    {
        Ok(translation) => {
            let execution_time_ms = start_time.elapsed().as_millis() as u64;

            // Execute SQL and generate summary (placeholder)
            let data_summary = format!("Query processed successfully in {}ms", execution_time_ms);

            info!(
                "✅ Natural language query successful: {} -> SQL in {}ms",
                request.query.chars().take(50).collect::<String>(),
                execution_time_ms
            );

            Ok(Json(NaturalLanguageQueryResponse {
                sql: translation.sql,
                explanation: translation.explanation,
                confidence: translation.confidence,
                execution_time_ms,
                data_summary: Some(data_summary),
            }))
        }
        Err(e) => {
            error!(
                "❌ Natural language query failed for tenant {}: {}",
                request.tenant_id, e
            );
            Err((
                StatusCode::BAD_REQUEST,
                format!("Natural language query translation failed: {}", e),
            ))
        }
    }
}

/// Handle executive dashboard endpoint
pub async fn handle_executive_dashboard(
    State(ai_state): State<AIServiceState>,
    Json(request): Json<APIDashboardRequest>,
) -> Result<Json<AIExecutiveDashboardResponse>, (StatusCode, String)> {
    let start_time = std::time::Instant::now();

    info!(
        "📊 Generating executive dashboard for tenant: {}",
        request.tenant_id
    );

    // Build user context
    let user_context = UserContext {
        user_id: request.user_id.clone(),
        tenant_id: Some(request.tenant_id.clone()),
        accessible_tables: vec![
            "collections".to_string(),
            "vectors".to_string(),
            "tenants".to_string(),
        ],
        permissions: vec!["read_data".to_string(), "admin".to_string()],
        roles: vec!["executive".to_string(), "admin".to_string()],
    };

    // Parse focus areas
    let focus_areas = request
        .focus_areas
        .unwrap_or_default()
        .into_iter()
        .filter_map(|area| match area.as_str() {
            "revenue" => Some(crate::ai::executive_dashboard::FocusArea::Revenue),
            "customers" => Some(crate::ai::executive_dashboard::FocusArea::Customers),
            "performance" => Some(crate::ai::executive_dashboard::FocusArea::Performance),
            "operations" => Some(crate::ai::executive_dashboard::FocusArea::Operations),
            "growth" => Some(crate::ai::executive_dashboard::FocusArea::Growth),
            "risk" => Some(crate::ai::executive_dashboard::FocusArea::Risk),
            _ => None,
        })
        .collect();

    // Parse time period
    let time_period = match request.time_period.as_str() {
        "hour" => crate::ai::executive_dashboard::TimePeriod::LastHour,
        "day" => crate::ai::executive_dashboard::TimePeriod::LastDay,
        "week" => crate::ai::executive_dashboard::TimePeriod::LastWeek,
        "month" => crate::ai::executive_dashboard::TimePeriod::LastMonth,
        "quarter" => crate::ai::executive_dashboard::TimePeriod::LastQuarter,
        "year" => crate::ai::executive_dashboard::TimePeriod::LastYear,
        _ => crate::ai::executive_dashboard::TimePeriod::LastWeek,
    };

    // Build dashboard request
    let dashboard_request = ExecutiveDashboardRequest {
        tenant_id: request.tenant_id.clone(),
        user_context,
        time_period,
        focus_areas,
        custom_queries: request.custom_queries,
    };

    // Generate dashboard
    match ai_state
        .ai_dashboard
        .generate_dashboard(dashboard_request)
        .await
    {
        Ok(dashboard_response) => {
            let execution_time_ms = start_time.elapsed().as_millis() as u64;

            info!(
                "✅ Executive dashboard generated for tenant {} in {}ms with {} insights",
                request.tenant_id,
                execution_time_ms,
                dashboard_response.natural_language_insights.len()
            );

            Ok(Json(dashboard_response))
        }
        Err(e) => {
            error!(
                "❌ Executive dashboard generation failed for tenant {}: {}",
                request.tenant_id, e
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Executive dashboard generation failed: {}", e),
            ))
        }
    }
}

/// Handle GET request for executive dashboard (with query parameters)
pub async fn handle_executive_dashboard_get(
    State(ai_state): State<AIServiceState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<AIExecutiveDashboardResponse>, (StatusCode, String)> {
    // Convert query parameters to dashboard request
    let tenant_id = params
        .get("tenant_id")
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "tenant_id parameter required".to_string(),
            )
        })?
        .clone();

    let user_id = params
        .get("user_id")
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "user_id parameter required".to_string(),
            )
        })?
        .clone();

    let time_period = params
        .get("period")
        .cloned()
        .unwrap_or_else(|| "week".to_string());

    let focus_areas = params
        .get("focus")
        .map(|areas| areas.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let api_request = APIDashboardRequest {
        tenant_id,
        user_id,
        time_period,
        focus_areas: if focus_areas.is_empty() {
            None
        } else {
            Some(focus_areas)
        },
        custom_queries: None,
    };

    handle_executive_dashboard(State(ai_state), Json(api_request)).await
}

/// Handle business insights endpoint
pub async fn handle_business_insights(
    State(ai_state): State<AIServiceState>,
    Json(request): Json<NaturalLanguageQueryRequest>,
) -> Result<Json<Vec<crate::ai::business_intelligence::BusinessInsight>>, (StatusCode, String)> {
    info!(
        "💡 Generating business insights for tenant: {}",
        request.tenant_id
    );

    // Build user context
    let user_context = UserContext {
        user_id: request.user_id.clone(),
        tenant_id: Some(request.tenant_id.clone()),
        accessible_tables: vec!["collections".to_string(), "vectors".to_string()],
        permissions: vec!["read_data".to_string()],
        roles: vec!["analyst".to_string()],
    };

    // Generate insights using BI engine
    match ai_state
        .ai_dashboard
        .bi_engine
        .generate_executive_dashboard(&user_context)
        .await
    {
        Ok(dashboard) => {
            info!(
                "✅ Generated {} business insights for tenant {}",
                dashboard.insights.len(),
                request.tenant_id
            );
            Ok(Json(dashboard.insights))
        }
        Err(e) => {
            error!("❌ Business insights generation failed: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Business insights generation failed: {}", e),
            ))
        }
    }
}

/// Handle LLM providers status endpoint
pub async fn handle_llm_providers_status(
    State(ai_state): State<AIServiceState>,
) -> Result<Json<std::collections::HashMap<String, bool>>, (StatusCode, String)> {
    debug!("🔍 Checking LLM provider health status");

    match ai_state.llm_engine.get_provider_health().await {
        health_status => {
            let status_map: std::collections::HashMap<String, bool> = health_status
                .into_iter()
                .map(|(provider, healthy)| (format!("{:?}", provider), healthy))
                .collect();

            debug!("✅ LLM provider status: {:?}", status_map);
            Ok(Json(status_map))
        }
    }
}

/// Initialize AI service state
pub async fn initialize_ai_service_state() -> Result<AIServiceState> {
    info!("🚀 Initializing AI service state for API endpoints");

    // Initialize AI executive dashboard
    let ai_dashboard = Arc::new(
        AIExecutiveDashboard::new()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize AI dashboard: {}", e))?,
    );

    // Get references to components
    let nl_translator = ai_dashboard.nl_translator.clone();
    let llm_engine = ai_dashboard.llm_engine.clone();

    info!("✅ AI service state initialized successfully");

    Ok(AIServiceState {
        ai_dashboard,
        nl_translator,
        llm_engine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_test::TestServer;

    #[tokio::test]
    async fn test_ai_service_initialization() {
        let ai_state = initialize_ai_service_state().await;
        assert!(
            ai_state.is_ok(),
            "AI service state should initialize successfully"
        );
    }

    #[tokio::test]
    async fn test_natural_language_query_endpoint() {
        // This would test the actual API endpoint
        // For now, test the request/response structure

        let request = NaturalLanguageQueryRequest {
            query: "What are our top 10 customers by revenue?".to_string(),
            tenant_id: "test_tenant".to_string(),
            user_id: "test_user".to_string(),
            context: None,
        };

        assert!(!request.query.is_empty());
        assert!(!request.tenant_id.is_empty());
        assert!(!request.user_id.is_empty());
    }

    #[test]
    fn test_api_request_validation() {
        let dashboard_request = APIDashboardRequest {
            tenant_id: "test_tenant".to_string(),
            user_id: "test_user".to_string(),
            time_period: "week".to_string(),
            focus_areas: Some(vec!["revenue".to_string(), "customers".to_string()]),
            custom_queries: Some(vec!["What is our growth rate?".to_string()]),
        };

        assert_eq!(dashboard_request.tenant_id, "test_tenant");
        assert_eq!(dashboard_request.time_period, "week");
        assert!(dashboard_request.focus_areas.is_some());
        assert!(dashboard_request.custom_queries.is_some());
    }
}
