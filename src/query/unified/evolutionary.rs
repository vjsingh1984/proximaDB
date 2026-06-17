//! Root compatibility adapter for the extracted evolutionary optimizer.

use std::sync::Arc;

use async_trait::async_trait;

use super::ast::QueryComponent;
use super::optimizer::SelectivityEstimate;
use crate::ai::llm_integration::LLMIntegrationEngine;

struct LlmMutationAdvisor {
    engine: Arc<LLMIntegrationEngine>,
}

pub(crate) fn llm_mutation_advisor(
    engine: Arc<LLMIntegrationEngine>,
) -> Arc<dyn proximadb_query::evolutionary::EvolutionaryMutationAdvisor> {
    Arc::new(LlmMutationAdvisor { engine })
}

#[async_trait]
impl proximadb_query::evolutionary::EvolutionaryMutationAdvisor for LlmMutationAdvisor {
    async fn propose_order(
        &self,
        current_order: &[usize],
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
    ) -> Option<Vec<usize>> {
        let mut prompt = String::from(
            "Given these query components and their selectivity, propose an optimal execution order (list of indices).\n\n",
        );
        for (i, c) in components.iter().enumerate() {
            prompt.push_str(&format!(
                "Component {}: Model={:?}, Estimated Selectivity={:.2}\n",
                i, c.model, selectivity[i].selectivity
            ));
        }
        prompt.push_str(&format!("\nCurrent order: {:?}\n", current_order));
        prompt.push_str(
            "Propose a potentially better order as a JSON list of indices. Just the list, nothing else.",
        );

        match self.engine.query_with_fallback(&prompt).await {
            Ok(resp) => serde_json::from_str::<Vec<usize>>(&resp.content)
                .ok()
                .filter(|order| order.len() == current_order.len()),
            Err(_) => None,
        }
    }
}

/// Evolutionary optimizer for query plans.
pub struct EvolutionaryOptimizer {
    inner: proximadb_query::evolutionary::EvolutionaryOptimizer,
}

impl EvolutionaryOptimizer {
    /// Create a new evolutionary optimizer.
    pub fn new(population_size: usize, generations: usize) -> Self {
        Self {
            inner: proximadb_query::evolutionary::EvolutionaryOptimizer::new(
                population_size,
                generations,
            ),
        }
    }

    /// Attach an LLM engine for mutation operators.
    pub fn with_llm(mut self, engine: Arc<LLMIntegrationEngine>) -> Self {
        self.inner = self.inner.with_advisor(llm_mutation_advisor(engine));
        self
    }

    /// Optimize query component order.
    pub async fn optimize<F>(
        &self,
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
        cost_fn: F,
    ) -> Vec<usize>
    where
        F: Fn(&[SelectivityEstimate], &[usize]) -> f64,
    {
        self.inner.optimize(components, selectivity, cost_fn).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::{
        ComponentDependency, DataModel, DistanceMetric, DocumentQueryExpr, JoinType,
        ModelOperation, QueryComponent, VectorSearchExpr, VectorSearchParams,
    };

    fn mock_component(id: usize, deps: Vec<usize>) -> QueryComponent {
        QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: format!("c{}", id),
                query_vector: vec![],
                top_k: 10,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: deps
                .into_iter()
                .map(|d| ComponentDependency {
                    component_index: d,
                    join_field: "id".to_string(),
                    join_type: JoinType::Inner,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn test_evolutionary_optimizer_convergence() {
        let c0 = mock_component(0, vec![]);
        let c1 = QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                collection: "c1".to_string(),
                path_filters: vec![],
                text_search: None,
                projection: vec![],
                sort: None,
                limit: Some(0),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        let components = vec![c0, c1];
        let selectivity = vec![
            SelectivityEstimate {
                selectivity: 0.1,
                confidence: 1.0,
                estimated_rows: 100,
                method: crate::query::unified::optimizer::EstimationMethod::Statistics,
            },
            SelectivityEstimate {
                selectivity: 0.5,
                confidence: 1.0,
                estimated_rows: 500,
                method: crate::query::unified::optimizer::EstimationMethod::Statistics,
            },
        ];

        let optimizer = EvolutionaryOptimizer::new(20, 10);
        let best_order = optimizer
            .optimize(&components, &selectivity, |sel, order| {
                let mut total_cost = 0.0;
                let mut intermediate_size = 1.0;
                for &idx in order {
                    let base_cost = if idx == 0 { 2.0 } else { 1.0 };
                    total_cost += base_cost * intermediate_size;
                    intermediate_size *= sel[idx].selectivity;
                }
                total_cost
            })
            .await;

        assert_eq!(best_order, vec![1, 0]);
    }
}
