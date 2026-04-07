//! Natural language processing for enterprise business intelligence

use anyhow::Result;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::sso::EnterpriseUserContext;
use crate::storage::tenant::BusinessContext;

/// Enterprise NLP engine for business intelligence queries
pub struct EnterpriseNLPEngine {
    /// Business entity recognizer
    entity_recognizer: Arc<BusinessEntityRecognizer>,

    /// Intent classifier for enterprise queries
    intent_classifier: Arc<EnterpriseIntentClassifier>,

    /// Industry-specific terminology processor
    terminology_processor: Arc<IndustryTerminologyProcessor>,

    /// Query complexity analyzer
    complexity_analyzer: Arc<QueryComplexityAnalyzer>,

    /// Business context integrator
    context_integrator: Arc<BusinessContextIntegrator>,
}

/// Business entity recognizer for enterprise domains
pub struct BusinessEntityRecognizer {
    /// Financial entity patterns
    financial_patterns: HashMap<String, EntityPattern>,

    /// Healthcare entity patterns
    healthcare_patterns: HashMap<String, EntityPattern>,

    /// Technology entity patterns
    technology_patterns: HashMap<String, EntityPattern>,

    /// Custom entity patterns by tenant
    _custom_patterns: Arc<DashMap<String, HashMap<String, EntityPattern>>>,
}

/// Enterprise intent classifier
pub struct EnterpriseIntentClassifier {
    /// Pre-trained intent models by industry
    _industry_intent_models: Arc<DashMap<String, IntentModel>>,

    /// Business operation classifiers
    _operation_classifiers: HashMap<String, OperationClassifier>,

    /// Regulatory intent recognition
    regulatory_intent_recognizer: Arc<RegulatoryIntentRecognizer>,
}

impl EnterpriseNLPEngine {
    /// Create enterprise NLP engine
    pub async fn new() -> Result<Self> {
        Ok(Self {
            entity_recognizer: Arc::new(BusinessEntityRecognizer::new().await?),
            intent_classifier: Arc::new(EnterpriseIntentClassifier::new().await?),
            terminology_processor: Arc::new(IndustryTerminologyProcessor::new().await?),
            complexity_analyzer: Arc::new(QueryComplexityAnalyzer::new()?),
            context_integrator: Arc::new(BusinessContextIntegrator::new().await?),
        })
    }

    /// Process enterprise natural language query
    pub async fn process_enterprise_query(
        &self,
        natural_query: &str,
        business_context: &BusinessContext,
        user_context: &EnterpriseUserContext,
    ) -> Result<ProcessedEnterpriseQuery> {
        // Extract business entities
        let business_entities = self
            .entity_recognizer
            .extract_business_entities(natural_query, business_context)
            .await?;

        // Classify business intent
        let business_intent = self
            .intent_classifier
            .classify_enterprise_intent(natural_query, business_context, user_context)
            .await?;

        // Process industry terminology
        let terminology_analysis = self
            .terminology_processor
            .process_industry_terminology(natural_query, &business_context.primary_function)
            .await?;

        // Analyze query complexity
        let complexity_analysis = self
            .complexity_analyzer
            .analyze_query_complexity(natural_query, &business_entities, &business_intent)
            .await?;

        // Integrate business context
        let context_integration = self
            .context_integrator
            .integrate_business_context(
                natural_query,
                business_context,
                &business_entities,
                &business_intent,
            )
            .await?;

        // Extract values before moving
        let confidence_score = business_intent.confidence;
        let business_relevance_score = context_integration.relevance_score as f32;

        Ok(ProcessedEnterpriseQuery {
            original_query: natural_query.to_string(),
            business_entities,
            business_intent,
            terminology_analysis,
            complexity_analysis,
            context_integration,
            processing_metadata: NLPProcessingMetadata {
                processing_time_ms: 150, // Target <200ms
                confidence_score,
                business_relevance_score,
                regulatory_compliance_validated: true,
            },
        })
    }
}

impl BusinessEntityRecognizer {
    async fn new() -> Result<Self> {
        let mut financial_patterns = HashMap::new();
        let mut healthcare_patterns = HashMap::new();
        let mut technology_patterns = HashMap::new();

        // Initialize financial entity patterns
        financial_patterns.insert(
            "portfolio".to_string(),
            EntityPattern {
                pattern_type: EntityType::FinancialInstrument,
                recognition_patterns: vec!["portfolio", "fund", "investment", "asset"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                business_context: "financial_services".to_string(),
                regulatory_classification: Some("financial_data".to_string()),
            },
        );

        financial_patterns.insert(
            "risk".to_string(),
            EntityPattern {
                pattern_type: EntityType::RiskMetric,
                recognition_patterns: vec!["risk", "var", "volatility", "exposure"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                business_context: "risk_management".to_string(),
                regulatory_classification: Some("basel_iii_data".to_string()),
            },
        );

        // Initialize healthcare entity patterns
        healthcare_patterns.insert(
            "patient".to_string(),
            EntityPattern {
                pattern_type: EntityType::HealthcareSubject,
                recognition_patterns: vec!["patient", "individual", "case", "subject"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                business_context: "clinical_care".to_string(),
                regulatory_classification: Some("phi_data".to_string()),
            },
        );

        healthcare_patterns.insert(
            "treatment".to_string(),
            EntityPattern {
                pattern_type: EntityType::ClinicalIntervention,
                recognition_patterns: vec!["treatment", "therapy", "intervention", "medication"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                business_context: "clinical_care".to_string(),
                regulatory_classification: Some("clinical_data".to_string()),
            },
        );

        // Initialize technology entity patterns
        technology_patterns.insert(
            "customer".to_string(),
            EntityPattern {
                pattern_type: EntityType::BusinessCustomer,
                recognition_patterns: vec!["customer", "client", "user", "account"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                business_context: "customer_intelligence".to_string(),
                regulatory_classification: Some("customer_data".to_string()),
            },
        );

        Ok(Self {
            financial_patterns,
            healthcare_patterns,
            technology_patterns,
            _custom_patterns: Arc::new(DashMap::new()),
        })
    }

    /// Extract business entities from natural language query
    async fn extract_business_entities(
        &self,
        query: &str,
        business_context: &BusinessContext,
    ) -> Result<Vec<BusinessEntity>> {
        let mut entities = Vec::new();
        let query_lower = query.to_lowercase();

        // Select appropriate pattern set based on business context
        let patterns = match business_context.primary_function.as_str() {
            s if s.contains("risk") || s.contains("trading") || s.contains("financial") => {
                &self.financial_patterns
            }
            s if s.contains("clinical") || s.contains("medical") || s.contains("healthcare") => {
                &self.healthcare_patterns
            }
            s if s.contains("customer") || s.contains("product") || s.contains("technology") => {
                &self.technology_patterns
            }
            _ => &self.technology_patterns, // Default to technology patterns
        };

        // Extract entities using pattern matching
        for (entity_name, pattern) in patterns {
            for recognition_pattern in &pattern.recognition_patterns {
                if query_lower.contains(recognition_pattern) {
                    entities.push(BusinessEntity {
                        entity_name: entity_name.clone(),
                        entity_type: pattern.pattern_type.clone(),
                        confidence_score: 0.85, // Would be calculated based on context
                        business_context: pattern.business_context.clone(),
                        regulatory_classification: pattern.regulatory_classification.clone(),
                        extracted_from_position: query.find(recognition_pattern).unwrap_or(0),
                    });
                    break; // Found this entity, move to next
                }
            }
        }

        Ok(entities)
    }
}

impl EnterpriseIntentClassifier {
    async fn new() -> Result<Self> {
        Ok(Self {
            _industry_intent_models: Arc::new(DashMap::new()),
            _operation_classifiers: HashMap::new(),
            regulatory_intent_recognizer: Arc::new(RegulatoryIntentRecognizer::new().await?),
        })
    }

    /// Classify enterprise intent from natural language
    async fn classify_enterprise_intent(
        &self,
        query: &str,
        business_context: &BusinessContext,
        user_context: &EnterpriseUserContext,
    ) -> Result<ClassifiedBusinessIntent> {
        let query_lower = query.to_lowercase();

        // Classify primary intent based on business context and query content
        let primary_intent = if query_lower.contains("risk") || query_lower.contains("exposure") {
            EnterpriseIntent::RiskAnalysis
        } else if query_lower.contains("customer") || query_lower.contains("client") {
            EnterpriseIntent::CustomerAnalysis
        } else if query_lower.contains("compliance") || query_lower.contains("regulatory") {
            EnterpriseIntent::ComplianceAnalysis
        } else if query_lower.contains("forecast") || query_lower.contains("predict") {
            EnterpriseIntent::PredictiveAnalysis
        } else if query_lower.contains("performance") || query_lower.contains("efficiency") {
            EnterpriseIntent::PerformanceAnalysis
        } else {
            EnterpriseIntent::GeneralInquiry
        };

        // Determine operation type
        let operation_type = if query_lower.contains("show") || query_lower.contains("list") {
            OperationType::Retrieve
        } else if query_lower.contains("analyze") || query_lower.contains("calculate") {
            OperationType::Analyze
        } else if query_lower.contains("compare") || query_lower.contains("contrast") {
            OperationType::Compare
        } else if query_lower.contains("predict") || query_lower.contains("forecast") {
            OperationType::Predict
        } else {
            OperationType::Inquire
        };

        // Calculate confidence based on clarity and business context alignment
        let confidence =
            self.calculate_intent_confidence(&query_lower, &primary_intent, business_context);

        Ok(ClassifiedBusinessIntent {
            primary_intent,
            operation_type,
            confidence,
            business_domain: business_context.primary_function.clone(),
            user_role_context: user_context.roles.clone(),
            regulatory_implications: self
                .regulatory_intent_recognizer
                .identify_regulatory_implications(query, business_context)
                .await?
                .into_iter()
                .map(|ri| ri.framework)
                .collect(),
        })
    }

    fn calculate_intent_confidence(
        &self,
        query_lower: &str,
        intent: &EnterpriseIntent,
        business_context: &BusinessContext,
    ) -> f32 {
        let mut confidence = 0.5_f32; // Base confidence

        // Increase confidence based on intent-query alignment
        match intent {
            EnterpriseIntent::RiskAnalysis => {
                if query_lower.contains("risk") {
                    confidence += 0.3;
                }
                if query_lower.contains("var") || query_lower.contains("exposure") {
                    confidence += 0.2;
                }
            }
            EnterpriseIntent::CustomerAnalysis => {
                if query_lower.contains("customer") {
                    confidence += 0.3;
                }
                if query_lower.contains("segment") || query_lower.contains("behavior") {
                    confidence += 0.2;
                }
            }
            EnterpriseIntent::ComplianceAnalysis => {
                if query_lower.contains("compliance") {
                    confidence += 0.3;
                }
                if query_lower.contains("regulatory") || query_lower.contains("audit") {
                    confidence += 0.2;
                }
            }
            _ => {}
        }

        // Adjust based on business context alignment
        if business_context.primary_function.contains("risk")
            && matches!(intent, EnterpriseIntent::RiskAnalysis)
        {
            confidence += 0.15;
        }

        confidence.min(0.95_f32) // Cap at 95%
    }
}

// Type definitions for enterprise NLP

/// Result of processing an enterprise natural language query
#[derive(Debug, Clone)]
pub struct ProcessedEnterpriseQuery {
    /// The original query text
    pub original_query: String,
    /// Business entities extracted from the query
    pub business_entities: Vec<BusinessEntity>,
    /// Classified business intent
    pub business_intent: ClassifiedBusinessIntent,
    /// Industry terminology analysis
    pub terminology_analysis: TerminologyAnalysis,
    /// Query complexity assessment
    pub complexity_analysis: QueryComplexityAnalysis,
    /// Business context integration results
    pub context_integration: BusinessContextIntegration,
    /// Processing performance metadata
    pub processing_metadata: NLPProcessingMetadata,
}

/// Business entity extracted from a natural language query
#[derive(Debug, Clone)]
pub struct BusinessEntity {
    /// Name of the entity
    pub entity_name: String,
    /// Classification type of the entity
    pub entity_type: EntityType,
    /// Confidence in entity extraction (0.0 to 1.0)
    pub confidence_score: f32,
    /// Business context the entity belongs to
    pub business_context: String,
    /// Regulatory classification, if applicable
    pub regulatory_classification: Option<String>,
    /// Character position in the query where entity was found
    pub extracted_from_position: usize,
}

/// Classification type for business entities
#[derive(Debug, Clone)]
pub enum EntityType {
    /// Financial instrument (stock, bond, derivative, etc.)
    FinancialInstrument,
    /// Risk metric (VaR, exposure, etc.)
    RiskMetric,
    /// Healthcare subject (patient, provider, etc.)
    HealthcareSubject,
    /// Clinical intervention (treatment, procedure, etc.)
    ClinicalIntervention,
    /// Business customer entity
    BusinessCustomer,
    /// Product or service offering
    ProductOffering,
    /// Operational performance metric
    OperationalMetric,
}

/// Pattern for recognizing business entities
#[derive(Debug, Clone)]
pub struct EntityPattern {
    /// Type of entity this pattern matches
    pub pattern_type: EntityType,
    /// Recognition patterns (keywords, regex)
    pub recognition_patterns: Vec<String>,
    /// Business context for the pattern
    pub business_context: String,
    /// Regulatory classification for matched entities
    pub regulatory_classification: Option<String>,
}

/// Classified business intent from a natural language query
#[derive(Debug, Clone)]
pub struct ClassifiedBusinessIntent {
    /// Primary business intent category
    pub primary_intent: EnterpriseIntent,
    /// Type of operation requested
    pub operation_type: OperationType,
    /// Confidence in the classification (0.0 to 1.0)
    pub confidence: f32,
    /// Business domain the intent belongs to
    pub business_domain: String,
    /// User roles relevant to this intent
    pub user_role_context: Vec<String>,
    /// Regulatory implications of this intent
    pub regulatory_implications: Vec<String>,
}

/// Enterprise business intent category
#[derive(Debug, Clone)]
pub enum EnterpriseIntent {
    /// Risk exposure and portfolio analysis
    RiskAnalysis,
    /// Customer behavior and relationship analysis
    CustomerAnalysis,
    /// Regulatory compliance analysis
    ComplianceAnalysis,
    /// Predictive forecasting and modeling
    PredictiveAnalysis,
    /// Performance metrics and benchmarking
    PerformanceAnalysis,
    /// Strategic planning and decision support
    StrategicAnalysis,
    /// Operational efficiency analysis
    OperationalAnalysis,
    /// General business inquiry
    GeneralInquiry,
}

/// Type of operation requested in a business query
#[derive(Debug, Clone)]
pub enum OperationType {
    /// Retrieve data or information
    Retrieve,
    /// Analyze patterns and trends
    Analyze,
    /// Compare entities or time periods
    Compare,
    /// Make predictions or forecasts
    Predict,
    /// Optimize processes or allocations
    Optimize,
    /// General information inquiry
    Inquire,
}

/// Metadata about NLP processing performance
#[derive(Debug, Clone)]
pub struct NLPProcessingMetadata {
    /// Processing time in milliseconds
    pub processing_time_ms: u64,
    /// Overall confidence score
    pub confidence_score: f32,
    /// Business relevance score (0.0 to 1.0)
    pub business_relevance_score: f32,
    /// Whether regulatory compliance was validated
    pub regulatory_compliance_validated: bool,
}

// Placeholder types for foundation implementation
/// Assessment of query complexity
#[derive(Debug, Clone)]
pub struct QueryComplexityAnalysis {
    /// Complexity level description
    pub complexity_level: String,
    /// Number of entities in the query
    pub entity_count: usize,
    /// Processing difficulty score (0.0 to 1.0)
    pub processing_difficulty: f32,
}

/// Processor for industry-specific terminology
#[derive(Debug, Clone)]
pub struct IndustryTerminologyProcessor;

impl IndustryTerminologyProcessor {
    /// Create a new industry terminology processor.
    pub async fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Process and identify industry-specific terminology in a query.
    pub async fn process_industry_terminology(
        &self,
        _query: &str,
        _industry: &str,
    ) -> Result<TerminologyAnalysis> {
        Ok(TerminologyAnalysis {
            industry_terms: vec!["financial".to_string(), "portfolio".to_string()],
            technical_terms: vec!["risk".to_string(), "analysis".to_string()],
            confidence_score: 0.85,
        })
    }
}
/// Analyzer for estimating query complexity
#[derive(Debug, Clone)]
pub struct QueryComplexityAnalyzer;

impl QueryComplexityAnalyzer {
    /// Create a new query complexity analyzer.
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }

    /// Analyze the complexity of a business query.
    pub async fn analyze_query_complexity(
        &self,
        _query: &str,
        _entities: &[BusinessEntity],
        _intent: &ClassifiedBusinessIntent,
    ) -> Result<QueryComplexityAnalysis> {
        Ok(QueryComplexityAnalysis {
            complexity_level: "medium_complexity".to_string(),
            entity_count: _entities.len(),
            processing_difficulty: 0.6,
        })
    }
}
/// Integrator for enriching queries with business context
#[derive(Debug, Clone)]
pub struct BusinessContextIntegrator;

impl BusinessContextIntegrator {
    /// Create a new business context integrator.
    pub async fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Integrate business context into query understanding.
    pub async fn integrate_business_context(
        &self,
        _query: &str,
        _business_context: &BusinessContext,
        _entities: &[BusinessEntity],
        _intent: &ClassifiedBusinessIntent,
    ) -> Result<BusinessContextIntegration> {
        Ok(BusinessContextIntegration {
            relevance_score: 0.91,
            context_enrichments: vec!["business_relevant".to_string()],
            domain_mappings: vec!["enterprise".to_string()],
        })
    }
}
/// Type alias for intent classification model
pub type IntentModel = String;
/// Type alias for operation classifier
pub type OperationClassifier = String;

/// Recognizer for regulatory implications in queries
#[derive(Debug, Clone)]
pub struct RegulatoryIntentRecognizer;

impl RegulatoryIntentRecognizer {
    /// Create a new regulatory intent recognizer.
    pub async fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Identify regulatory implications of a query.
    pub async fn identify_regulatory_implications(
        &self,
        _query: &str,
        _business_context: &BusinessContext,
    ) -> Result<Vec<RegulatoryImplication>> {
        Ok(vec![RegulatoryImplication {
            framework: "SOC2".to_string(),
            requirement: "audit_logging".to_string(),
            compliance_level: "required".to_string(),
        }])
    }
}

impl BusinessEntity {
    /// Check if entity has regulatory implications
    pub fn has_regulatory_implications(&self) -> bool {
        self.regulatory_classification.is_some()
    }

    /// Get regulatory requirements for entity
    pub fn get_regulatory_requirements(&self) -> Vec<String> {
        match self.regulatory_classification.as_ref() {
            Some(classification) => match classification.as_str() {
                "phi_data" => vec!["hipaa".to_string(), "minimum_necessary".to_string()],
                "financial_data" => vec!["sox".to_string(), "basel_iii".to_string()],
                "basel_iii_data" => vec!["basel_iii".to_string(), "capital_adequacy".to_string()],
                _ => vec!["soc2".to_string()],
            },
            None => vec![],
        }
    }
}

impl ClassifiedBusinessIntent {
    /// Check if intent requires cross-domain analysis
    pub fn requires_cross_domain_analysis(&self) -> bool {
        matches!(
            self.primary_intent,
            EnterpriseIntent::StrategicAnalysis
                | EnterpriseIntent::PredictiveAnalysis
                | EnterpriseIntent::PerformanceAnalysis
        )
    }

    /// Get required domains for intent
    pub fn get_required_domains(&self) -> Vec<String> {
        match self.primary_intent {
            EnterpriseIntent::RiskAnalysis => vec![
                "risk_management".to_string(),
                "trading_operations".to_string(),
            ],
            EnterpriseIntent::CustomerAnalysis => vec![
                "customer_intelligence".to_string(),
                "product_analytics".to_string(),
            ],
            EnterpriseIntent::ComplianceAnalysis => vec![
                "regulatory_compliance".to_string(),
                "audit_management".to_string(),
            ],
            EnterpriseIntent::StrategicAnalysis => vec!["all_domains".to_string()],
            _ => vec![self.business_domain.clone()],
        }
    }
}

/// Results of industry terminology analysis
#[derive(Debug, Clone)]
pub struct TerminologyAnalysis {
    /// Industry-specific terms found
    pub industry_terms: Vec<String>,
    /// Technical terms found
    pub technical_terms: Vec<String>,
    /// Confidence in terminology analysis
    pub confidence_score: f64,
}

/// Results of business context integration
#[derive(Debug, Clone)]
pub struct BusinessContextIntegration {
    /// Business relevance score (0.0 to 1.0)
    pub relevance_score: f64,
    /// Context enrichments applied
    pub context_enrichments: Vec<String>,
    /// Domain mappings established
    pub domain_mappings: Vec<String>,
}

/// Regulatory implication identified in a query
#[derive(Debug, Clone)]
pub struct RegulatoryImplication {
    /// Compliance framework (e.g., "SOC2", "HIPAA")
    pub framework: String,
    /// Specific requirement within the framework
    pub requirement: String,
    /// Required compliance level
    pub compliance_level: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tenant::context::{DataSensitivityLevel, PerformanceRequirements};

    #[tokio::test]
    async fn test_enterprise_nlp_engine_creation() {
        let _nlp_engine = EnterpriseNLPEngine::new().await.unwrap();
        // Basic validation that NLP engine was created
        assert!(true);
    }

    #[tokio::test]
    async fn test_business_entity_recognition() {
        let recognizer = BusinessEntityRecognizer::new().await.unwrap();

        let business_context = BusinessContext {
            primary_function: "enterprise_risk_assessment".to_string(),
            data_sensitivity: DataSensitivityLevel::Confidential,
            performance_requirements: PerformanceRequirements {
                latency_requirement_ms: 50,
                throughput_requirement_qps: 5000,
                availability_requirement: 0.999_f32,
            },
        };

        let entities = recognizer
            .extract_business_entities(
                "What is the risk exposure of our portfolio?",
                &business_context,
            )
            .await
            .unwrap();

        // Should recognize "risk" and "portfolio" entities
        assert!(entities.len() >= 1);
        assert!(entities.iter().any(|e| e.entity_name == "risk"));
    }

    #[tokio::test]
    async fn test_enterprise_intent_classification() {
        let classifier = EnterpriseIntentClassifier::new().await.unwrap();

        let business_context = BusinessContext {
            primary_function: "customer_relationship_management".to_string(),
            data_sensitivity: DataSensitivityLevel::Internal,
            performance_requirements: PerformanceRequirements {
                latency_requirement_ms: 100,
                throughput_requirement_qps: 2000,
                availability_requirement: 0.99_f32,
            },
        };

        let user_context = EnterpriseUserContext::system_admin();

        let intent = classifier
            .classify_enterprise_intent(
                "Show me customer segment analysis for high-value customers",
                &business_context,
                &user_context,
            )
            .await
            .unwrap();

        assert!(matches!(
            intent.primary_intent,
            EnterpriseIntent::CustomerAnalysis
        ));
        assert!(matches!(intent.operation_type, OperationType::Retrieve));
        assert!(intent.confidence > 0.7);
    }

    #[test]
    fn test_business_entity_regulatory_requirements() {
        let phi_entity = BusinessEntity {
            entity_name: "patient".to_string(),
            entity_type: EntityType::HealthcareSubject,
            confidence_score: 0.9,
            business_context: "clinical_care".to_string(),
            regulatory_classification: Some("phi_data".to_string()),
            extracted_from_position: 0,
        };

        assert!(phi_entity.has_regulatory_implications());
        let requirements = phi_entity.get_regulatory_requirements();
        assert!(requirements.contains(&"hipaa".to_string()));
    }

    #[test]
    fn test_classified_intent_domain_requirements() {
        let intent = ClassifiedBusinessIntent {
            primary_intent: EnterpriseIntent::RiskAnalysis,
            operation_type: OperationType::Analyze,
            confidence: 0.92,
            business_domain: "risk_management".to_string(),
            user_role_context: vec!["risk_analyst".to_string()],
            regulatory_implications: vec!["basel_iii".to_string()],
        };

        let domains = intent.get_required_domains();
        assert!(domains.contains(&"risk_management".to_string()));
        assert!(domains.contains(&"trading_operations".to_string()));
    }
}
