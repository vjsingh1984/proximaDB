/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Query Plan Validator
//!
//! Provides canonical validation for query plans before execution.
//! Ensures that plan node capabilities match storage engine capabilities.
//!
//! ## Architecture
//!
//! ```text
//! QueryFacadeAdapter
//!         ↓
//!   PlanValidator::validate_plan()
//!         ↓
//!   Check plan nodes against capability registry
//!         ↓
//!   Return Ok(()) or CapabilityCheckError
//!         ↓
//!   Execute query (if validation passed)
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximaDB::query::validator::PlanValidator;
//! use proximaDB::query::capability::global_capability_registry;
//!
//! let validator = PlanValidator::new(global_capability_registry());
//! validator.validate_plan(&plan, "SST")?;
//! ```

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, instrument, warn};

use crate::query::capability::{
    Capability, CapabilityCheckError, CapabilityRegistry, CapabilitySet,
};
use crate::query::federated::optimizer::PlanNode;

/// Backwards-compat alias for [`QueryValidationResult`].
pub type ValidationResult = QueryValidationResult;

/// Validation result with detailed information
#[derive(Debug, Clone)]
pub struct QueryValidationResult {
    /// Whether validation passed
    pub is_valid: bool,
    /// Missing capabilities (if any)
    pub missing_capabilities: Vec<String>,
    /// Available alternatives for missing capabilities
    pub available_alternatives: Vec<String>,
    /// Engine name that was validated against
    pub engine_name: String,
}

impl QueryValidationResult {
    /// Create a successful validation result
    pub fn success(engine_name: String) -> Self {
        Self {
            is_valid: true,
            missing_capabilities: vec![],
            available_alternatives: vec![],
            engine_name,
        }
    }

    /// Create a failed validation result
    pub fn failure(
        engine_name: String,
        missing_capabilities: Vec<String>,
        available_alternatives: Vec<String>,
    ) -> Self {
        Self {
            is_valid: false,
            missing_capabilities,
            available_alternatives,
            engine_name,
        }
    }
}

/// Query plan validator
///
/// Validates that plan nodes have the required capabilities
/// before execution, preventing runtime errors.
#[derive(Clone)]
pub struct PlanValidator {
    /// Capability registry to check against
    registry: Arc<CapabilityRegistry>,
    /// Enable detailed validation logging
    verbose_logging: bool,
}

impl PlanValidator {
    /// Create a new plan validator
    pub fn new(registry: Arc<CapabilityRegistry>) -> Self {
        Self {
            registry,
            verbose_logging: false,
        }
    }

    /// Create a new plan validator with verbose logging
    pub fn with_verbose_logging(mut self) -> Self {
        self.verbose_logging = true;
        self
    }

    /// Validate a plan node against the given storage engine
    ///
    /// This checks that all capabilities required by the plan node
    /// are supported by the specified storage engine.
    ///
    /// ## Arguments
    ///
    /// * `plan` - The plan node to validate
    /// * `engine_name` - The name of the storage engine (e.g., "SST", "VIPER")
    ///
    /// ## Returns
    ///
    /// * `Ok(QueryValidationResult)` - Validation result with details
    /// * `Err(CapabilityCheckError)` - If engine capabilities cannot be determined
    #[instrument(skip(self, plan), fields(engine = %engine_name, plan_id = %plan.id))]
    pub fn validate_plan(
        &self,
        plan: &PlanNode,
        engine_name: &str,
    ) -> Result<QueryValidationResult, CapabilityCheckError> {
        info!(
            plan_id = plan.id,
            node_type = ?plan.node_type,
            engine = %engine_name,
            "Validating plan node"
        );

        // Get engine capabilities
        let engine_caps = self.registry.get_capabilities(engine_name).ok_or_else(|| {
            CapabilityCheckError::UnsupportedCapability {
                capability: format!("engine:{}", engine_name),
                available_alternatives: self.registry.list_registered_engines(),
            }
        })?;

        // Infer required capabilities from plan
        let required_caps = plan.infer_capabilities();

        if self.verbose_logging {
            debug!(
                "Engine capabilities: {}",
                engine_caps
                    .iter()
                    .map(|c| format!("{:?}", c))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            debug!(
                "Required capabilities: {}",
                required_caps
                    .iter()
                    .map(|c| format!("{:?}", c))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        // Check if engine supports all required capabilities
        let missing: Vec<Capability> = required_caps
            .iter()
            .filter(|cap| !engine_caps.contains_capability(cap))
            .cloned()
            .collect();

        if missing.is_empty() {
            let result = QueryValidationResult::success(engine_name.to_string());
            info!(
                plan_id = plan.id,
                engine = %engine_name,
                "Plan validation passed"
            );
            Ok(result)
        } else {
            let missing_names: Vec<String> = missing.iter().map(|c| format!("{:?}", c)).collect();

            let available_alternatives: Vec<String> = self
                .registry
                .find_engines_with_capabilities(&required_caps)
                .into_iter()
                .filter(|name| name != engine_name)
                .collect();

            warn!(
                plan_id = plan.id,
                engine = %engine_name,
                missing_capabilities = %missing_names.join(", "),
                "Plan validation failed - missing capabilities"
            );

            Ok(QueryValidationResult::failure(
                engine_name.to_string(),
                missing_names,
                available_alternatives,
            ))
        }
    }

    /// Validate a plan node against all registered engines
    ///
    /// Returns a list of engines that can execute this plan.
    #[instrument(skip(self, plan), fields(plan_id = %plan.id))]
    pub fn validate_against_all_engines(
        &self,
        plan: &PlanNode,
    ) -> Result<Vec<String>, CapabilityCheckError> {
        let engines = self.registry.list_registered_engines();
        let mut compatible_engines = Vec::new();

        info!(
            plan_id = plan.id,
            total_engines = engines.len(),
            "Validating plan against all engines"
        );

        for engine_name in &engines {
            match self.validate_plan(plan, engine_name) {
                Ok(result) if result.is_valid => {
                    compatible_engines.push(engine_name.clone());
                    debug!(
                        plan_id = plan.id,
                        engine = %engine_name,
                        "Engine is compatible"
                    );
                }
                Ok(result) => {
                    debug!(
                        plan_id = plan.id,
                        engine = %engine_name,
                        missing = %result.missing_capabilities.join(", "),
                        "Engine is not compatible"
                    );
                }
                Err(e) => {
                    warn!(
                        plan_id = plan.id,
                        engine = %engine_name,
                        error = %e,
                        "Failed to validate against engine"
                    );
                }
            }
        }

        info!(
            plan_id = plan.id,
            compatible_count = compatible_engines.len(),
            "Found compatible engines"
        );

        Ok(compatible_engines)
    }

    /// Check if a plan is executable and return detailed error if not
    ///
    /// This is a convenience method that converts QueryValidationResult
    /// into a Result type for easier error handling.
    #[instrument(skip(self, plan), fields(engine = %engine_name))]
    pub fn ensure_executable(
        &self,
        plan: &PlanNode,
        engine_name: &str,
    ) -> Result<(), CapabilityCheckError> {
        let result = self.validate_plan(plan, engine_name)?;

        if result.is_valid {
            Ok(())
        } else {
            Err(CapabilityCheckError::UnsupportedCapability {
                capability: result.missing_capabilities.join(", "),
                available_alternatives: result.available_alternatives,
            })
        }
    }

    /// Get all capabilities required by a plan node
    ///
    /// This recursively collects capabilities from the entire plan tree.
    pub fn get_required_capabilities(&self, plan: &PlanNode) -> CapabilitySet {
        plan.infer_capabilities()
    }

    /// Find the best engine for executing a plan
    ///
    /// Returns the engine name that has the most capabilities
    /// required by the plan.
    #[instrument(skip(self, plan), fields(plan_id = %plan.id))]
    pub fn find_best_engine(
        &self,
        plan: &PlanNode,
    ) -> Result<Option<String>, CapabilityCheckError> {
        let required = self.get_required_capabilities(plan);
        let engines = self.registry.list_registered_engines();

        let mut best_engine: Option<String> = None;
        let mut best_score = 0usize;

        for engine_name in &engines {
            if let Some(engine_caps) = self.registry.get_capabilities(engine_name) {
                // Count how many required capabilities the engine has
                let score = required
                    .iter()
                    .filter(|cap| engine_caps.contains_capability(cap))
                    .count();

                if score > best_score && score == required.len() {
                    best_engine = Some(engine_name.clone());
                    best_score = score;
                }
            }
        }

        if let Some(ref engine) = best_engine {
            info!(
                plan_id = plan.id,
                engine = %engine,
                capability_match = best_score,
                total_required = required.len(),
                "Found best engine for plan"
            );
        } else {
            warn!(
                plan_id = plan.id,
                "No engine found with all required capabilities"
            );
        }

        Ok(best_engine)
    }
}

// ============================================================================
// UNIT TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::capability::{Capability, CapabilitySet};
    use crate::query::federated::optimizer::{PlanNode, PlanNodeType};
    use std::sync::Arc;

    fn create_test_registry() -> Arc<CapabilityRegistry> {
        let registry = Arc::new(CapabilityRegistry::new());

        // Register SST with vector capabilities
        let sst_caps = CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::VectorSearch,
            Capability::HNSWIndex,
        ]);
        registry.register_capabilities("SST", sst_caps);

        // Register VIPER with graph capabilities
        let viper_caps = CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::GraphQuery,
            Capability::GraphTraversal,
        ]);
        registry.register_capabilities("VIPER", viper_caps);

        registry
    }

    fn create_test_vector_scan_plan() -> PlanNode {
        PlanNode {
            id: 1,
            node_type: PlanNodeType::VectorSearch {
                collection: "test_collection".to_string(),
                top_k: 10,
                query_vector_source: crate::query::federated::optimizer::VectorSource::Literal(
                    vec![0.1, 0.2, 0.3],
                ),
            },
            estimated_cost: 1.0,
            estimated_rows: 10,
            output_columns: vec!["id".to_string(), "score".to_string()],
            required_capabilities: {
                let mut caps = CapabilitySet::new();
                caps.add(Capability::VectorSearch);
                caps.add(Capability::Scan);
                caps
            },
        }
    }

    fn create_test_graph_query_plan() -> PlanNode {
        PlanNode {
            id: 2,
            node_type: PlanNodeType::GraphTraversal {
                cypher: "MATCH (n) RETURN n".to_string(),
                start_nodes: None,
                source_alias: None,
            },
            estimated_cost: 2.0,
            estimated_rows: 100,
            output_columns: vec!["n".to_string()],
            required_capabilities: {
                let mut caps = CapabilitySet::new();
                caps.add(Capability::GraphQuery);
                caps.add(Capability::GraphTraversal);
                caps
            },
        }
    }

    #[test]
    fn test_validate_vector_search_on_sst() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_vector_scan_plan();

        let result = validator.validate_plan(&plan, "SST").unwrap();

        assert!(result.is_valid);
        assert_eq!(result.engine_name, "SST");
        assert!(result.missing_capabilities.is_empty());
    }

    #[test]
    fn test_validate_graph_query_on_viper() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_graph_query_plan();

        let result = validator.validate_plan(&plan, "VIPER").unwrap();

        assert!(result.is_valid);
        assert_eq!(result.engine_name, "VIPER");
        assert!(result.missing_capabilities.is_empty());
    }

    #[test]
    fn test_validate_vector_search_on_viper_fails() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_vector_scan_plan();

        let result = validator.validate_plan(&plan, "VIPER").unwrap();

        assert!(!result.is_valid);
        assert_eq!(result.engine_name, "VIPER");
        assert!(!result.missing_capabilities.is_empty());
        // VIPER doesn't have VectorSearch capability
        assert!(
            result
                .missing_capabilities
                .iter()
                .any(|c| c.contains("VectorSearch"))
        );
    }

    #[test]
    fn test_validate_graph_query_on_sst_fails() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_graph_query_plan();

        let result = validator.validate_plan(&plan, "SST").unwrap();

        assert!(!result.is_valid);
        assert_eq!(result.engine_name, "SST");
        assert!(!result.missing_capabilities.is_empty());
    }

    #[test]
    fn test_validate_against_all_engines() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_vector_scan_plan();

        let compatible = validator.validate_against_all_engines(&plan).unwrap();

        // Only SST should support vector search
        assert_eq!(compatible.len(), 1);
        assert!(compatible.contains(&"SST".to_string()));
    }

    #[test]
    fn test_ensure_executable_success() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_vector_scan_plan();

        let result = validator.ensure_executable(&plan, "SST");

        assert!(result.is_ok());
    }

    #[test]
    fn test_ensure_executable_failure() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_vector_scan_plan();

        let result = validator.ensure_executable(&plan, "VIPER");

        assert!(result.is_err());
    }

    #[test]
    fn test_find_best_engine() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_vector_scan_plan();

        let best_engine = validator.find_best_engine(&plan).unwrap();

        assert_eq!(best_engine, Some("SST".to_string()));
    }

    #[test]
    fn test_get_required_capabilities() {
        let registry = create_test_registry();
        let validator = PlanValidator::new(registry);
        let plan = create_test_vector_scan_plan();

        let caps = validator.get_required_capabilities(&plan);

        assert!(!caps.is_empty());
        assert!(caps.contains_capability(&Capability::VectorSearch));
    }

    #[test]
    fn test_validation_result_success() {
        let result = QueryValidationResult::success("SST".to_string());

        assert!(result.is_valid);
        assert!(result.missing_capabilities.is_empty());
        assert!(result.available_alternatives.is_empty());
        assert_eq!(result.engine_name, "SST");
    }

    #[test]
    fn test_validation_result_failure() {
        let result = QueryValidationResult::failure(
            "VIPER".to_string(),
            vec!["VectorSearch".to_string()],
            vec!["SST".to_string()],
        );

        assert!(!result.is_valid);
        assert_eq!(result.missing_capabilities.len(), 1);
        assert_eq!(result.missing_capabilities[0], "VectorSearch");
        assert_eq!(result.available_alternatives.len(), 1);
        assert_eq!(result.engine_name, "VIPER");
    }
}
