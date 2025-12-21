//! Search Strategy Registry
//!
//! Central registry for managing and selecting search strategies.
//! Enables runtime strategy selection and custom strategy registration.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::{
    AdaptiveSearchStrategy, ApproximateSearchStrategy, CandidateProvider, ExactSearchStrategy,
    ScoredCandidate, SearchContext, SearchCostEstimate, SearchStrategy,
};

/// Registry for search strategies
///
/// Provides centralized management of search strategies with:
/// - Built-in strategies (exact, approximate, adaptive)
/// - Custom strategy registration
/// - Automatic strategy selection
/// - Cost-based optimization
///
/// # Thread Safety
///
/// The registry uses interior mutability (RwLock) for thread-safe access.
///
/// # Usage
///
/// ```rust,ignore
/// let registry = SearchStrategyRegistry::new();
///
/// // Get a specific strategy
/// let exact = registry.get("exact").unwrap();
///
/// // Auto-select best strategy
/// let best = registry.select_best(&ctx, num_candidates);
///
/// // Register custom strategy
/// registry.register("my_strategy", Arc::new(MyStrategy::new()));
/// ```
pub struct SearchStrategyRegistry {
    strategies: RwLock<HashMap<String, Arc<dyn SearchStrategy>>>,
    /// Default strategy name (used when no specific strategy requested)
    default_strategy: RwLock<String>,
}

impl Default for SearchStrategyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchStrategyRegistry {
    /// Create a new registry with built-in strategies
    pub fn new() -> Self {
        let mut strategies = HashMap::new();

        // Register built-in strategies
        strategies.insert(
            "exact".to_string(),
            Arc::new(ExactSearchStrategy::new()) as Arc<dyn SearchStrategy>,
        );
        strategies.insert(
            "approximate".to_string(),
            Arc::new(ApproximateSearchStrategy::new()) as Arc<dyn SearchStrategy>,
        );
        strategies.insert(
            "adaptive".to_string(),
            Arc::new(AdaptiveSearchStrategy::new()) as Arc<dyn SearchStrategy>,
        );

        Self {
            strategies: RwLock::new(strategies),
            default_strategy: RwLock::new("adaptive".to_string()),
        }
    }

    /// Create an empty registry (no built-in strategies)
    pub fn empty() -> Self {
        Self {
            strategies: RwLock::new(HashMap::new()),
            default_strategy: RwLock::new("exact".to_string()),
        }
    }

    /// Register a custom strategy
    pub fn register(&self, name: &str, strategy: Arc<dyn SearchStrategy>) -> Result<()> {
        let mut strategies = self.strategies.write().map_err(|e| anyhow!("Lock error: {}", e))?;

        if strategies.contains_key(name) {
            tracing::warn!("Overwriting existing strategy: {}", name);
        }

        strategies.insert(name.to_string(), strategy);
        tracing::info!("Registered search strategy: {}", name);

        Ok(())
    }

    /// Unregister a strategy
    pub fn unregister(&self, name: &str) -> Result<Option<Arc<dyn SearchStrategy>>> {
        let mut strategies = self.strategies.write().map_err(|e| anyhow!("Lock error: {}", e))?;
        Ok(strategies.remove(name))
    }

    /// Get a specific strategy by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn SearchStrategy>> {
        let strategies = self.strategies.read().ok()?;
        strategies.get(name).cloned()
    }

    /// Get the default strategy
    pub fn get_default(&self) -> Option<Arc<dyn SearchStrategy>> {
        let default_name = self.default_strategy.read().ok()?;
        self.get(&default_name)
    }

    /// Set the default strategy
    pub fn set_default(&self, name: &str) -> Result<()> {
        // Verify strategy exists
        if self.get(name).is_none() {
            return Err(anyhow!("Strategy '{}' not found", name));
        }

        let mut default = self.default_strategy.write().map_err(|e| anyhow!("Lock error: {}", e))?;
        *default = name.to_string();

        Ok(())
    }

    /// List all registered strategy names
    pub fn list(&self) -> Vec<String> {
        self.strategies
            .read()
            .map(|s| s.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Find the first strategy that applies to the given context
    pub fn find_applicable(&self, ctx: &dyn SearchContext) -> Option<Arc<dyn SearchStrategy>> {
        let strategies = self.strategies.read().ok()?;

        // First, try to find a strategy that explicitly applies
        for strategy in strategies.values() {
            if strategy.applies_to(ctx) {
                return Some(strategy.clone());
            }
        }

        // Fall back to default
        drop(strategies);
        self.get_default()
    }

    /// Select the best strategy based on cost estimates
    ///
    /// Evaluates all applicable strategies and returns the one with:
    /// 1. Lowest estimated time (within recall requirements)
    /// 2. Highest priority (for tie-breaking)
    pub fn select_best(
        &self,
        ctx: &dyn SearchContext,
        num_candidates: usize,
    ) -> Option<Arc<dyn SearchStrategy>> {
        let strategies = self.strategies.read().ok()?;

        let mut best: Option<(Arc<dyn SearchStrategy>, SearchCostEstimate)> = None;

        for strategy in strategies.values() {
            // Skip strategies that don't apply
            if !strategy.applies_to(ctx) {
                continue;
            }

            let cost = strategy.estimate_cost(ctx, num_candidates);

            // Check if this strategy meets accuracy requirements
            if cost.expected_recall < ctx.accuracy_threshold() {
                continue;
            }

            match &best {
                None => best = Some((strategy.clone(), cost)),
                Some((_, best_cost)) => {
                    // Prefer lower time, then higher priority
                    let is_better = cost.estimated_time_ms < best_cost.estimated_time_ms
                        || (cost.estimated_time_ms == best_cost.estimated_time_ms
                            && cost.priority > best_cost.priority);

                    if is_better {
                        best = Some((strategy.clone(), cost));
                    }
                }
            }
        }

        best.map(|(s, _)| s).or_else(|| {
            drop(strategies);
            self.get_default()
        })
    }

    /// Execute search using the best available strategy
    pub async fn execute_search(
        &self,
        ctx: &dyn SearchContext,
        candidates: &dyn CandidateProvider,
    ) -> Result<Vec<ScoredCandidate>> {
        let strategy = self
            .select_best(ctx, candidates.total_candidates())
            .or_else(|| self.get_default())
            .ok_or_else(|| anyhow!("No search strategy available"))?;

        tracing::debug!(
            "SearchStrategyRegistry: executing '{}' strategy",
            strategy.name()
        );

        strategy.execute(ctx, candidates).await
    }

    /// Get strategy with specific configuration
    pub fn get_exact(&self) -> Option<Arc<dyn SearchStrategy>> {
        self.get("exact")
    }

    /// Get approximate strategy
    pub fn get_approximate(&self) -> Option<Arc<dyn SearchStrategy>> {
        self.get("approximate")
    }

    /// Get adaptive strategy
    pub fn get_adaptive(&self) -> Option<Arc<dyn SearchStrategy>> {
        self.get("adaptive")
    }
}

// Global registry instance for convenience
lazy_static::lazy_static! {
    /// Global search strategy registry
    pub static ref GLOBAL_REGISTRY: SearchStrategyRegistry = SearchStrategyRegistry::new();
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::context::SearchContextImpl;
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::SearchMode;

    #[test]
    fn test_registry_creation() {
        let registry = SearchStrategyRegistry::new();

        // Built-in strategies should be registered
        assert!(registry.get("exact").is_some());
        assert!(registry.get("approximate").is_some());
        assert!(registry.get("adaptive").is_some());
    }

    #[test]
    fn test_list_strategies() {
        let registry = SearchStrategyRegistry::new();
        let names = registry.list();

        assert!(names.contains(&"exact".to_string()));
        assert!(names.contains(&"approximate".to_string()));
        assert!(names.contains(&"adaptive".to_string()));
    }

    #[test]
    fn test_custom_strategy_registration() {
        let registry = SearchStrategyRegistry::new();

        // Register a custom strategy (using exact as placeholder)
        registry
            .register("custom", Arc::new(ExactSearchStrategy::new()))
            .unwrap();

        assert!(registry.get("custom").is_some());
    }

    #[test]
    fn test_find_applicable() {
        let registry = SearchStrategyRegistry::new();

        // Exact mode should find exact strategy
        let exact_ctx = SearchContextImpl::new(
            vec![1.0, 0.0],
            10,
            DistanceMetric::Cosine,
        );

        let strategy = registry.find_applicable(&exact_ctx);
        assert!(strategy.is_some());
        assert_eq!(strategy.unwrap().name(), "exact");

        // Approximate mode should find approximate strategy
        let approx_ctx = SearchContextImpl::new(
            vec![1.0, 0.0],
            10,
            DistanceMetric::Cosine,
        ).with_search_mode(SearchMode::approximate());

        let strategy = registry.find_applicable(&approx_ctx);
        assert!(strategy.is_some());
        assert_eq!(strategy.unwrap().name(), "approximate");
    }

    #[test]
    fn test_set_default() {
        let registry = SearchStrategyRegistry::new();

        // Default is adaptive
        assert_eq!(
            registry.get_default().unwrap().name(),
            "adaptive"
        );

        // Change to exact
        registry.set_default("exact").unwrap();
        assert_eq!(
            registry.get_default().unwrap().name(),
            "exact"
        );

        // Invalid strategy should fail
        assert!(registry.set_default("nonexistent").is_err());
    }

    #[test]
    fn test_empty_registry() {
        let registry = SearchStrategyRegistry::empty();

        assert!(registry.get("exact").is_none());
        assert!(registry.list().is_empty());
    }
}
