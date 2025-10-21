//! AI-powered intelligence for Release 2 enterprise platform

pub mod analytics;
pub mod business_intelligence;
pub mod executive_dashboard;
pub mod insights;
pub mod llm;
pub mod llm_integration;
pub mod natural_language;
pub mod natural_language_api;
pub mod nlp;

pub use analytics::{
    ConversationalAnalyticsEngine, GovernanceAnalyticsEngine, PredictiveAnalyticsEngine,
};
pub use business_intelligence::engine::ExecutiveDashboard;
pub use business_intelligence::{BusinessInsight, BusinessIntelligenceEngine};
pub use executive_dashboard::{
    AIExecutiveDashboard, AIExecutiveDashboardResponse, ExecutiveDashboardRequest,
};
pub use insights::{AutomatedInsightEngine, BusinessInsightsGenerator, InsightType};
pub use llm::{AIIntelligenceFoundation, BusinessIntent, LLMIntegrationEngine};
pub use llm_integration::{
    LLMConfig, LLMError, LLMIntegrationEngine as ModernLLMEngine, LLMProvider, LLMRequest,
    LLMResponse,
};
pub use natural_language::{NLQueryTranslator, SchemaContext, TranslationResult};
pub use natural_language_api::{
    ConversationalBusinessAnswer, NaturalLanguageBusinessIntelligenceAPI,
};
pub use nlp::EnterpriseNLPEngine;

use anyhow::Result;
use std::sync::Arc;

/// AI-powered enterprise intelligence coordinator
pub struct AIEnterpiseIntelligenceCoordinator {
    /// AI intelligence foundation
    ai_foundation: Arc<AIIntelligenceFoundation>,

    /// Natural language processing
    nlp_engine: Arc<EnterpriseNLPEngine>,

    /// Automated insights generation
    insights_engine: Arc<AutomatedInsightEngine>,

    /// Predictive analytics
    predictive_analytics: Arc<PredictiveAnalyticsEngine>,

    /// Conversational analytics
    conversational_analytics: Arc<ConversationalAnalyticsEngine>,
}

impl AIEnterpiseIntelligenceCoordinator {
    /// Create AI enterprise intelligence coordinator
    pub async fn new() -> Result<Self> {
        Ok(Self {
            ai_foundation: Arc::new(AIIntelligenceFoundation::new().await?),
            nlp_engine: Arc::new(EnterpriseNLPEngine::new().await?),
            insights_engine: Arc::new(AutomatedInsightEngine::new().await?),
            predictive_analytics: Arc::new(PredictiveAnalyticsEngine::new(
                analytics::PredictiveAnalyticsConfig::default(),
            )),
            conversational_analytics: Arc::new(ConversationalAnalyticsEngine::new(
                analytics::ConversationalAnalyticsConfig::default(),
            )),
        })
    }

    /// Execute AI-powered enterprise intelligence query
    pub async fn execute_ai_enterprise_intelligence(
        &self,
        tenant_id: &str,
        ai_query: AIEnterpriseQuery,
        user_context: &crate::auth::sso::EnterpriseUserContext,
    ) -> Result<AIEnterpriseIntelligenceResult> {
        match ai_query {
            AIEnterpriseQuery::NaturalLanguage {
                query,
                business_context,
            } => {
                let result = self
                    .ai_foundation
                    .process_natural_language_business_query(
                        tenant_id,
                        &query,
                        &business_context,
                        user_context,
                    )
                    .await?;
                Ok(AIEnterpriseIntelligenceResult::NaturalLanguageAnswer(
                    result.to_string(),
                ))
            }
            AIEnterpriseQuery::AutomatedInsights {
                domains,
                insight_type,
            } => {
                let result = self
                    .insights_engine
                    .generate_automated_business_insights(
                        tenant_id,
                        &domains,
                        &insight_type,
                        user_context,
                    )
                    .await?;
                Ok(AIEnterpriseIntelligenceResult::AutomatedInsights(format!(
                    "{:?}",
                    result
                )))
            }
            AIEnterpriseQuery::PredictiveAnalytics {
                business_scenario,
                prediction_horizon,
            } => {
                let tenant_user_context: crate::storage::tenant::UserContext =
                    user_context.clone().into();
                let result = self
                    .predictive_analytics
                    .execute_business_prediction(
                        tenant_id,
                        &business_scenario,
                        &prediction_horizon,
                        &tenant_user_context,
                    )
                    .await?;
                Ok(AIEnterpriseIntelligenceResult::PredictiveAnalysis(format!(
                    "{:?}",
                    result
                )))
            }
            AIEnterpriseQuery::ConversationalSession {
                session_type,
                context,
            } => {
                let tenant_user_context: crate::storage::tenant::UserContext =
                    user_context.clone().into();
                let result = self
                    .conversational_analytics
                    .start_conversational_session(
                        tenant_id,
                        &session_type,
                        &context,
                        &tenant_user_context,
                    )
                    .await?;
                Ok(AIEnterpriseIntelligenceResult::ConversationalSession(
                    format!("{:?}", result),
                ))
            }
        }
    }
}

/// AI enterprise query types
#[derive(Debug, Clone)]
pub enum AIEnterpriseQuery {
    NaturalLanguage {
        query: String,
        business_context: crate::storage::tenant::BusinessContext,
    },
    AutomatedInsights {
        domains: Vec<String>,
        insight_type: InsightType,
    },
    PredictiveAnalytics {
        business_scenario: BusinessScenario,
        prediction_horizon: PredictionHorizon,
    },
    ConversationalSession {
        session_type: ConversationalSessionType,
        context: ConversationalContext,
    },
}

/// AI enterprise intelligence result
#[derive(Debug, Clone)]
pub enum AIEnterpriseIntelligenceResult {
    NaturalLanguageAnswer(AIIntelligentBusinessAnswer),
    AutomatedInsights(AutomatedBusinessInsights),
    PredictiveAnalysis(PredictiveBusinessAnalysis),
    ConversationalSession(ConversationalAnalyticsSession),
}

// Type definitions for AI intelligence
pub type AIIntelligentBusinessAnswer = String;
pub type AutomatedBusinessInsights = String;
pub type PredictiveBusinessAnalysis = String;
pub type ConversationalAnalyticsSession = String;
pub type BusinessScenario = String;
pub type PredictionHorizon = String;
pub type ConversationalSessionType = String;
pub type ConversationalContext = String;

/// Global AI intelligence manager
static AI_INTELLIGENCE_COORDINATOR: std::sync::OnceLock<AIEnterpiseIntelligenceCoordinator> =
    std::sync::OnceLock::new();

/// Initialize global AI intelligence coordinator
pub async fn initialize_ai_intelligence() -> Result<&'static AIEnterpiseIntelligenceCoordinator> {
    let coordinator = AIEnterpiseIntelligenceCoordinator::new().await?;
    Ok(AI_INTELLIGENCE_COORDINATOR.get_or_init(|| coordinator))
}

/// Get global AI intelligence coordinator
pub fn get_ai_intelligence() -> Option<&'static AIEnterpiseIntelligenceCoordinator> {
    AI_INTELLIGENCE_COORDINATOR.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ai_intelligence_coordinator_creation() {
        let coordinator = AIEnterpiseIntelligenceCoordinator::new().await.unwrap();
        // Basic validation that AI coordinator was created
        assert!(true);
    }

    #[test]
    fn test_ai_enterprise_query_types() {
        let nl_query = AIEnterpriseQuery::NaturalLanguage {
            query: "What is our risk exposure in emerging markets?".to_string(),
            business_context: crate::storage::tenant::BusinessContext::default(),
        };

        match nl_query {
            AIEnterpriseQuery::NaturalLanguage { query, .. } => {
                assert!(query.contains("risk exposure"));
            }
            _ => panic!("Should be natural language query"),
        }
    }
}
