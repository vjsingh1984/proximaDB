//! Prompt Builder for Natural Language Translation
//!
//! Builds secure, effective prompts for LLM-based SQL translation
//! with security constraints and context awareness.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::{Result, anyhow};

/// Prompt builder for LLM queries
#[derive(Debug, Clone)]
pub struct PromptBuilder {
    templates: HashMap<PromptTemplate, String>,
}

/// Available prompt templates
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PromptTemplate {
    SecureTranslation,
    BusinessIntelligence,
    DataAnalysis,
    TrendAnalysis,
    ExplanationGeneration,
}

impl PromptBuilder {
    pub fn new() -> Self {
        let mut templates = HashMap::new();

        // Secure SQL translation template
        templates.insert(
            PromptTemplate::SecureTranslation,
            include_str!("../../prompts/secure_sql_translation.txt").to_string()
        );

        // Fallback templates if files don't exist
        if templates.get(&PromptTemplate::SecureTranslation).unwrap().is_empty() {
            templates.insert(
                PromptTemplate::SecureTranslation,
                Self::default_secure_translation_template()
            );
        }

        templates.insert(
            PromptTemplate::BusinessIntelligence,
            Self::default_business_intelligence_template()
        );

        templates.insert(
            PromptTemplate::DataAnalysis,
            Self::default_data_analysis_template()
        );

        templates.insert(
            PromptTemplate::TrendAnalysis,
            Self::default_trend_analysis_template()
        );

        templates.insert(
            PromptTemplate::ExplanationGeneration,
            Self::default_explanation_template()
        );

        Self { templates }
    }

    /// Build prompt from template with variable substitution
    pub fn build_prompt(
        &self,
        template: PromptTemplate,
        variables: &[(&str, &str)],
    ) -> Result<String> {
        let template_text = self.templates.get(&template)
            .ok_or_else(|| anyhow!("Template {:?} not found", template))?;

        let mut prompt = template_text.clone();

        // Substitute variables
        for (key, value) in variables {
            let placeholder = format!("{{{}}}", key);
            prompt = prompt.replace(&placeholder, value);
        }

        // Validate that all placeholders were replaced
        if prompt.contains('{') && prompt.contains('}') {
            let remaining_placeholders = self.extract_placeholders(&prompt);
            return Err(anyhow!("Unresolved placeholders in prompt: {:?}", remaining_placeholders));
        }

        Ok(prompt)
    }

    /// Extract remaining placeholders from prompt
    fn extract_placeholders(&self, prompt: &str) -> Vec<String> {
        let mut placeholders = Vec::new();
        let mut in_placeholder = false;
        let mut current_placeholder = String::new();

        for ch in prompt.chars() {
            match ch {
                '{' if !in_placeholder => {
                    in_placeholder = true;
                    current_placeholder.clear();
                }
                '}' if in_placeholder => {
                    in_placeholder = false;
                    if !current_placeholder.is_empty() {
                        placeholders.push(current_placeholder.clone());
                    }
                }
                _ if in_placeholder => {
                    current_placeholder.push(ch);
                }
                _ => {}
            }
        }

        placeholders
    }

    /// Default secure SQL translation template
    fn default_secure_translation_template() -> String {
        r#"You are a secure SQL translator for ProximaDB. Follow these security rules:

SECURITY CONSTRAINTS:
- User can only access these tables: {user_accessible_tables}
- No DROP, DELETE, UPDATE, INSERT, ALTER, CREATE, or TRUNCATE statements allowed
- No access to system tables or metadata tables
- Queries must include proper WHERE clauses for tenant isolation
- Maximum result limit: 10,000 rows

DATABASE SCHEMA (accessible tables only):
{schema_context}

USER CONTEXT:
- User ID: {user_id}
- Tenant ID: {tenant_id}
- Accessible tables: {user_accessible_tables}

TRANSLATION TASK:
Convert this natural language query to safe SQL:
"{natural_language_query}"

REQUIREMENTS:
1. Generate SELECT statements only
2. Include tenant_id = '{tenant_id}' filters where applicable
3. Use proper JOINs for related tables
4. Limit results to prevent resource exhaustion (add LIMIT clause if missing)
5. Use only the tables and columns provided in the schema
6. Include helpful comments explaining the query logic

RESPONSE FORMAT:
Provide only the SQL query, optionally wrapped in ```sql code blocks.

SQL:"#.to_string()
    }

    /// Default business intelligence template
    fn default_business_intelligence_template() -> String {
        r#"You are a business intelligence analyst for ProximaDB. Generate insights based on the following data:

DATA CONTEXT:
{data_context}

BUSINESS METRICS:
{business_metrics}

ANALYSIS REQUEST:
{analysis_request}

INSTRUCTIONS:
1. Provide actionable business insights
2. Include specific numbers and trends
3. Suggest concrete recommendations
4. Focus on business impact and outcomes
5. Use clear, executive-friendly language

RESPONSE FORMAT:
Provide insights in a structured format with:
- Key findings
- Trends and patterns
- Recommendations
- Next steps

BUSINESS INSIGHTS:"#.to_string()
    }

    /// Default data analysis template
    fn default_data_analysis_template() -> String {
        r#"You are a data analyst for ProximaDB. Analyze the following dataset:

DATASET CONTEXT:
{dataset_context}

ANALYSIS TYPE:
{analysis_type}

DATA SAMPLE:
{data_sample}

ANALYSIS GOALS:
{analysis_goals}

INSTRUCTIONS:
1. Identify patterns and trends in the data
2. Highlight significant findings
3. Suggest data quality improvements
4. Recommend additional analysis
5. Focus on statistical significance

RESPONSE FORMAT:
Provide analysis with:
- Summary statistics
- Key patterns
- Anomalies or outliers
- Recommendations

DATA ANALYSIS:"#.to_string()
    }

    /// Default trend analysis template
    fn default_trend_analysis_template() -> String {
        r#"You are a trend analyst for ProximaDB. Analyze trends in the following data:

TIME SERIES DATA:
{time_series_data}

TREND PERIOD:
{trend_period}

BASELINE METRICS:
{baseline_metrics}

ANALYSIS REQUEST:
{analysis_request}

INSTRUCTIONS:
1. Identify upward, downward, and cyclical trends
2. Calculate trend strength and significance
3. Predict future trend direction
4. Identify trend drivers and influencers
5. Suggest trend-based strategies

RESPONSE FORMAT:
Provide trend analysis with:
- Trend direction and strength
- Seasonal patterns
- Predictions and forecasts
- Strategic recommendations

TREND ANALYSIS:"#.to_string()
    }

    /// Default explanation template
    fn default_explanation_template() -> String {
        r#"You are an expert at explaining technical concepts in simple terms.

TECHNICAL CONTENT:
{technical_content}

CONTEXT:
{context}

AUDIENCE:
{audience}

EXPLANATION REQUEST:
{explanation_request}

INSTRUCTIONS:
1. Use clear, non-technical language
2. Provide concrete examples
3. Focus on practical implications
4. Avoid jargon and technical terms
5. Make it actionable and understandable

RESPONSE FORMAT:
Provide a clear explanation that:
- Explains what happened
- Why it's important
- What it means for the user
- What they should do next

EXPLANATION:"#.to_string()
    }

    /// Get available templates
    pub fn get_available_templates(&self) -> Vec<PromptTemplate> {
        self.templates.keys().cloned().collect()
    }

    /// Validate template variables
    pub fn validate_template_variables(&self, template: &PromptTemplate, variables: &[(&str, &str)]) -> Result<Vec<String>> {
        let template_text = self.templates.get(template)
            .ok_or_else(|| anyhow!("Template {:?} not found", template))?;

        let required_placeholders = self.extract_placeholders(template_text);
        let provided_variables: HashMap<String, String> = variables.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let missing_variables: Vec<String> = required_placeholders.iter()
            .filter(|placeholder| !provided_variables.contains_key(*placeholder))
            .cloned()
            .collect();

        Ok(missing_variables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_builder_creation() {
        let builder = PromptBuilder::new();
        assert!(!builder.templates.is_empty());
        assert!(builder.templates.contains_key(&PromptTemplate::SecureTranslation));
    }

    #[test]
    fn test_prompt_building_with_variables() {
        let builder = PromptBuilder::new();

        let variables = vec![
            ("user_accessible_tables", "collections, vectors"),
            ("schema_context", "TABLE collections (id VARCHAR, name VARCHAR)"),
            ("natural_language_query", "Show me all collections"),
            ("tenant_id", "tenant_1"),
            ("user_id", "user_123"),
        ];

        let result = builder.build_prompt(PromptTemplate::SecureTranslation, &variables);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.contains("collections, vectors"));
        assert!(prompt.contains("Show me all collections"));
        assert!(prompt.contains("tenant_1"));
        assert!(!prompt.contains("{user_accessible_tables}")); // Should be replaced
    }

    #[test]
    fn test_placeholder_extraction() {
        let builder = PromptBuilder::new();
        let text = "Hello {name}, your {item} is ready. Contact {support}.";

        let placeholders = builder.extract_placeholders(text);
        assert_eq!(placeholders.len(), 3);
        assert!(placeholders.contains(&"name".to_string()));
        assert!(placeholders.contains(&"item".to_string()));
        assert!(placeholders.contains(&"support".to_string()));
    }

    #[test]
    fn test_template_variable_validation() {
        let builder = PromptBuilder::new();

        let complete_variables = vec![
            ("user_accessible_tables", "collections"),
            ("schema_context", "schema"),
            ("natural_language_query", "query"),
            ("tenant_id", "tenant_1"),
            ("user_id", "user_1"),
        ];

        let missing = builder.validate_template_variables(
            &PromptTemplate::SecureTranslation,
            &complete_variables
        ).unwrap();

        assert!(missing.is_empty());

        // Test with missing variables
        let incomplete_variables = vec![
            ("user_accessible_tables", "collections"),
            ("schema_context", "schema"),
        ];

        let missing = builder.validate_template_variables(
            &PromptTemplate::SecureTranslation,
            &incomplete_variables
        ).unwrap();

        assert!(!missing.is_empty());
    }
}