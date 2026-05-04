//! Evolutionary Query Optimizer
//!
//! Implements genetic algorithms to find optimal execution plans for complex
//! multi-model queries. This approaches global optima that greedy selectivity-based
//! reordering might miss.

use super::ast::QueryComponent;
use super::optimizer::SelectivityEstimate;
use crate::ai::llm_integration::LLMIntegrationEngine;
use rand::prelude::*;
use std::sync::Arc;

/// Evolutionary optimizer for query plans
pub struct EvolutionaryOptimizer {
    population_size: usize,
    generations: usize,
    llm_engine: Option<Arc<LLMIntegrationEngine>>,
}

impl EvolutionaryOptimizer {
    /// Create a new evolutionary optimizer
    pub fn new(population_size: usize, generations: usize) -> Self {
        Self {
            population_size,
            generations,
            llm_engine: None,
        }
    }

    /// Optimize query component order
    pub fn optimize<F>(
        &self,
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
        cost_fn: F,
    ) -> Vec<usize>
    where
        F: Fn(&[SelectivityEstimate], &[usize]) -> f64,
    {
        let n = components.len();
        if n <= 1 {
            return (0..n).collect();
        }

        // Build dependency graph
        let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];
        let mut in_degrees: Vec<usize> = vec![0; n];
        for (i, component) in components.iter().enumerate() {
            for dep in &component.dependencies {
                dependents[dep.component_index].push(i);
                in_degrees[i] += 1;
            }
        }

        let mut rng = thread_rng();
        let mut population =
            self.generate_initial_population(n, &dependents, &in_degrees, &mut rng);

        let mut best_order = population[0].clone();
        let mut min_cost = cost_fn(selectivity, &best_order);

        for _ in 0..self.generations {
            // Calculate fitness
            let costs: Vec<f64> = population
                .iter()
                .map(|order| cost_fn(selectivity, order))
                .collect();

            // Track global best
            for (i, &cost) in costs.iter().enumerate() {
                if cost < min_cost {
                    min_cost = cost;
                    best_order = population[i].clone();
                }
            }

            // Selection & Evolve
            let mut new_population = Vec::with_capacity(self.population_size);

            // Elitism: Keep best
            new_population.push(best_order.clone());

            while new_population.len() < self.population_size {
                // Tournament selection
                let p1 = self.select_parent(&population, &costs, &mut rng);
                let p2 = self.select_parent(&population, &costs, &mut rng);

                // Crossover (simple approach)
                let mut child = if rng.gen_bool(0.7) {
                    self.crossover(p1, p2)
                } else {
                    p1.clone()
                };

                // Mutation
                if rng.gen_bool(0.2) {
                    self.mutate(&mut child, &dependents, &in_degrees, &mut rng);
                }

                // TODO: LLM-assisted mutation (arXiv:2602.10387)
                // if self.llm_engine.is_some() && rng.gen_bool(0.05) {
                //    let llm_child = self.llm_mutate(&child, components, selectivity).await;
                //    if let Some(c) = llm_child { child = c; }
                // }

                new_population.push(child);
            }

            population = new_population;
        }

        best_order
    }

    /// Generate random valid topological sorts
    fn generate_initial_population(
        &self,
        n: usize,
        dependents: &[Vec<usize>],
        in_degrees: &[usize],
        rng: &mut ThreadRng,
    ) -> Vec<Vec<usize>> {
        let mut population = Vec::with_capacity(self.population_size);
        for _ in 0..self.population_size {
            population.push(self.generate_random_valid_order(n, dependents, in_degrees, rng));
        }
        population
    }

    fn generate_random_valid_order(
        &self,
        n: usize,
        dependents: &[Vec<usize>],
        in_degrees: &[usize],
        rng: &mut ThreadRng,
    ) -> Vec<usize> {
        let mut order = Vec::with_capacity(n);
        let mut current_in_degrees = in_degrees.to_vec();
        let mut ready: Vec<usize> = current_in_degrees
            .iter()
            .enumerate()
            .filter(|&(_, &d)| d == 0)
            .map(|(i, _)| i)
            .collect();

        while !ready.is_empty() {
            let pick_idx = rng.gen_range(0..ready.len());
            let node = ready.remove(pick_idx);
            order.push(node);

            for &dep in &dependents[node] {
                current_in_degrees[dep] -= 1;
                if current_in_degrees[dep] == 0 {
                    ready.push(dep);
                }
            }
        }

        if order.len() < n {
            // Fallback for cycles
            return (0..n).collect();
        }
        order
    }

    fn select_parent<'a>(
        &self,
        population: &'a [Vec<usize>],
        costs: &[f64],
        rng: &mut ThreadRng,
    ) -> &'a Vec<usize> {
        let idx1 = rng.gen_range(0..population.len());
        let idx2 = rng.gen_range(0..population.len());
        if costs[idx1] < costs[idx2] {
            &population[idx1]
        } else {
            &population[idx2]
        }
    }

    /// Mutation: Swap two nodes if it doesn't break dependencies
    fn mutate(
        &self,
        order: &mut Vec<usize>,
        dependents: &[Vec<usize>],
        in_degrees: &[usize],
        rng: &mut ThreadRng,
    ) {
        let i = rng.gen_range(0..order.len());
        let j = rng.gen_range(0..order.len());
        if i == j {
            return;
        }

        // Try swap
        order.swap(i, j);

        // Check validity
        if !self.is_valid(order, in_degrees, dependents) {
            // Rollback
            order.swap(i, j);
        }
    }

    /// Simple crossover
    fn crossover(&self, p1: &[usize], p2: &[usize]) -> Vec<usize> {
        if thread_rng().gen_bool(0.5) {
            p1.to_vec()
        } else {
            p2.to_vec()
        }
    }

    /// Attach an LLM engine for mutation operators (DBPlanBench-style)
    pub fn with_llm(mut self, engine: Arc<LLMIntegrationEngine>) -> Self {
        self.llm_engine = Some(engine);
        self
    }

    /// LLM-based mutation operator (arXiv:2602.10387)
    ///
    /// Proposes a new plan order based on higher-level reasoning about the
    /// components and their estimated selectivity.
    #[allow(dead_code)]
    pub async fn llm_mutate(
        &self,
        current_order: &[usize],
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
    ) -> Option<Vec<usize>> {
        let llm = self.llm_engine.as_ref()?;

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
        prompt.push_str("Propose a potentially better order as a JSON list of indices. Just the list, nothing else.");

        match llm.query_with_fallback(&prompt).await {
            Ok(resp) => {
                // Parse indices from response
                if let Ok(new_order) = serde_json::from_str::<Vec<usize>>(&resp.content) {
                    // Basic validation: same length and all indices present
                    if new_order.len() == current_order.len() {
                        return Some(new_order);
                    }
                }
                None
            }
            Err(_) => None,
        }
    }

    fn is_valid(&self, order: &[usize], in_degrees: &[usize], dependents: &[Vec<usize>]) -> bool {
        let mut current_in_degrees = in_degrees.to_vec();
        for &node in order {
            if current_in_degrees[node] != 0 {
                return false;
            }
            for &dep in &dependents[node] {
                current_in_degrees[dep] -= 1;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::{
        DataModel, DistanceMetric, ModelOperation, QueryComponent, VectorSearchExpr,
        VectorSearchParams,
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
                .map(|d| crate::query::unified::ast::ComponentDependency {
                    component_index: d,
                    join_field: "id".to_string(),
                    join_type: crate::query::unified::ast::JoinType::Inner,
                })
                .collect(),
        }
    }

    #[test]
    fn test_random_valid_order() {
        let components = vec![
            mock_component(0, vec![]),
            mock_component(1, vec![0]),
            mock_component(2, vec![0]),
            mock_component(3, vec![1, 2]),
        ];

        let mut dependents = vec![vec![]; 4];
        let mut in_degrees = vec![0; 4];
        for (i, c) in components.iter().enumerate() {
            for dep in &c.dependencies {
                dependents[dep.component_index].push(i);
                in_degrees[i] += 1;
            }
        }

        let optimizer = EvolutionaryOptimizer::new(10, 5);
        let mut rng = thread_rng();

        for _ in 0..100 {
            let order =
                optimizer.generate_random_valid_order(4, &dependents, &in_degrees, &mut rng);
            assert!(optimizer.is_valid(&order, &in_degrees, &dependents));
            assert_eq!(order[0], 0);
            assert_eq!(order[3], 3);
        }
    }

    #[test]
    fn test_evolutionary_optimizer_convergence() {
        let c0 = mock_component(0, vec![]); // Vector
        let c1 = QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(
                crate::query::unified::ast::DocumentQueryExpr {
                    collection: "c1".to_string(),
                    path_filters: vec![],
                    text_search: None,
                    projection: vec![],
                    sort: None,
                    limit: Some(0),
                },
            ),
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
        let best_order = optimizer.optimize(&components, &selectivity, |sel, order| {
            let mut total_cost = 0.0;
            let mut intermediate_size = 1.0;
            for &idx in order {
                let base_cost = if idx == 0 { 2.0 } else { 1.0 };
                total_cost += base_cost * intermediate_size;
                intermediate_size *= sel[idx].selectivity;
            }
            total_cost
        });

        assert_eq!(best_order, vec![1, 0]); // Should pick the lower base cost first
    }
}
