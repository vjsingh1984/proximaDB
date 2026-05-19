//! Filter Contracts for Normalized Hybrid Search (Issue #38, SB-08)
//!
//! This module provides the foundation for efficient hybrid search by defining
//! normalized filter contracts that work across all storage engines (HNSW, IVF, brute-force).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    FilterContract                            │
//! │  - Normalized filter expression                              │
//! │  - Cross-engine compatibility                                │
//! │  - SIMD-friendly evaluation                                 │
//! └──────────────────────┬────────────────────────────────────────┘
//!                        │
//!                        ▼
//!     ┌─────────────────────────────────────────┐
//!     │         CandidateSet                     │
//!     │  - Incremental candidate generation     │
//!     │  - Multi-stage filtering                │
//!     │  - Efficient ranking                    │
//!     └─────────────────────────────────────────┘
//!                        │
//!                        ▼
//!     ┌─────────────────────────────────────────┐
//!     │      Storage Engine Execution            │
//!     │  HNSW │ IVF │ Brute-force               │
//!     └─────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Normalization**: Consistent filter representation across engines
//! - **Pushdown**: Enable filter evaluation at the storage layer
//! - **Incremental**: Stream-friendly candidate generation
//! - **Zero-Copy**: Minimize data movement during filtering
//!
//! ## Design Principles
//!
//! 1. **Storage Agnostic**: Same contract works for HNSW, IVF, and brute-force
//! 2. **Incremental**: Support streaming candidate generation
//! 3. **SIMD-Friendly**: Enable vectorized filter evaluation
//! 4. **Composable**: Support complex filter expressions (AND, OR, NOT)
//! 5. **Efficient**: Minimize overhead over unfiltered search

use anyhow::Result;
use arrow::array::BooleanArray;
use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;
use tracing::debug;

use crate::core::search::{ComparisonOperator, FilterExpression};

/// Configurable fallback selectivity heuristics for normalized filters.
///
/// These values are used only when the filter contract has no catalog/statistics
/// context. Defaults preserve the historical planner behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterSelectivityPolicy {
    /// Equality predicates such as `field = value`.
    pub equals: f64,
    /// Inequality predicates such as `field != value`.
    pub not_equals: f64,
    /// Open-ended range predicates such as `<`, `<=`, `>`, `>=`.
    pub range: f64,
    /// Membership predicates such as `field IN (...)`.
    pub in_list: f64,
    /// Closed range predicates such as `BETWEEN`.
    pub between: f64,
    /// Fallback for unsupported or unknown predicates.
    pub unknown: f64,
}

impl Default for FilterSelectivityPolicy {
    fn default() -> Self {
        Self {
            equals: 0.1,
            not_equals: 0.9,
            range: 0.5,
            in_list: 0.2,
            between: 0.3,
            unknown: 0.5,
        }
    }
}

impl FilterSelectivityPolicy {
    /// Validate policy values as finite probabilities.
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("equals", self.equals),
            ("not_equals", self.not_equals),
            ("range", self.range),
            ("in_list", self.in_list),
            ("between", self.between),
            ("unknown", self.unknown),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                anyhow::bail!(
                    "filter selectivity policy {name} must be finite and between 0.0 and 1.0, got {value}"
                );
            }
        }
        Ok(())
    }
}

/// Normalized filter contract for hybrid search
///
/// Provides a consistent interface for filter evaluation across
/// all storage engines and enables efficient pushdown optimizations.
pub trait FilterContract: Send + Sync + Debug {
    /// Get the estimated selectivity of this filter (0.0 to 1.0)
    ///
    /// Selectivity is the fraction of rows expected to pass the filter.
    /// This is used for optimization (e.g., choosing between graph-first
    /// vs vector-first search strategies).
    fn estimated_selectivity(&self) -> f64;

    /// Check if this filter can be pushed down to a storage engine
    ///
    /// Some filters can be evaluated more efficiently at the storage layer
    /// (e.g., during HNSW traversal or IVF inverted list filtering).
    fn can_pushdown(&self, engine: StorageEngineType) -> bool;

    /// Evaluate this filter against a single metadata record
    ///
    /// Used for row-level filtering in candidate sets.
    fn evaluate_row(&self, metadata: &serde_json::Value) -> Result<bool>;

    /// Evaluate this filter against a batch of metadata records
    ///
    /// Returns a boolean array where true indicates the row passes the filter.
    /// This is optimized for SIMD/vectorized evaluation.
    fn evaluate_batch(&self, metadata_batch: &[serde_json::Value]) -> Result<BooleanArray>;

    /// Get the fields required by this filter
    ///
    /// Used for column pruning and projection optimization.
    fn required_fields(&self) -> Vec<String>;

    /// Check if this filter is compatible with another filter
    ///
    /// Used for query optimization and filter combination.
    fn is_compatible_with(&self, other: &dyn FilterContract) -> bool;

    /// Clone the filter contract
    fn clone_box(&self) -> Box<dyn FilterContract>;

    /// Get a string representation of this filter
    fn as_string(&self) -> String;

    /// Return the underlying `FilterExpression` if available.
    ///
    /// Used by HNSW predicate-aware search (Phase C) to convert this filter
    /// into the `Fn(&str) -> bool` callback accepted by `search_with_predicate`.
    /// Default implementation returns `None` (no pushdown possible).
    fn as_filter_expression(&self) -> Option<&FilterExpression> {
        None
    }
}

impl Clone for Box<dyn FilterContract> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Storage engine types for filter pushdown
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum StorageEngineType {
    /// HNSW (Hierarchical Navigable Small World) index
    HNSW,
    /// IVF (Inverted File) index
    IVF,
    /// Brute-force flat search
    BruteForce,
    /// DiskANN (Vamana graph + SSD layout)
    DiskANN,
    /// Annoy (Approximate Nearest Neighbors Oh Yeah)
    Annoy,
    /// LSH (Locality Sensitive Hashing)
    LSH,
    /// SST (Sorted String Table) storage engine
    SST,
    /// VIPER storage engine
    VIPER,
    /// HELIX storage engine
    HELIX,
    /// NOVA storage engine
    NOVA,
}

/// Candidate set for incremental hybrid search
///
/// Represents a set of candidate vector IDs that can be incrementally
/// refined through multiple filtering stages.
pub trait CandidateSet: Send + Sync + Debug {
    /// Add a candidate ID to this set
    fn add_candidate(&mut self, id: String) -> Result<()>;

    /// Add multiple candidate IDs at once
    fn add_candidates(&mut self, ids: Vec<String>) -> Result<()> {
        for id in ids {
            self.add_candidate(id)?;
        }
        Ok(())
    }

    /// Check if a candidate ID is in this set
    fn contains(&self, id: &str) -> bool;

    /// Get the number of candidates in this set
    fn len(&self) -> usize;

    /// Check if this set is empty
    fn is_empty(&self) -> bool;

    /// Clear all candidates from this set
    fn clear(&mut self);

    /// Get all candidate IDs as a vector
    fn to_vec(&self) -> Vec<String>;

    /// Apply a filter contract to this candidate set
    ///
    /// Returns a new candidate set with only the candidates that pass the filter.
    fn filter(
        &self,
        contract: &dyn FilterContract,
        metadata_lookup: &dyn MetadataLookup,
    ) -> Result<Box<dyn CandidateSet>>;

    /// Take the top K candidates from this set
    ///
    /// Candidates are ranked by the provided scores.
    fn top_k(&self, k: usize, scores: &[f32]) -> Result<Box<dyn CandidateSet>>;

    /// Union this candidate set with another
    fn union(&self, other: &dyn CandidateSet) -> Result<Box<dyn CandidateSet>>;

    /// Intersect this candidate set with another
    fn intersect(&self, other: &dyn CandidateSet) -> Result<Box<dyn CandidateSet>>;

    /// Clone the candidate set
    fn clone_box(&self) -> Box<dyn CandidateSet>;
}

impl Clone for Box<dyn CandidateSet> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Metadata lookup for candidate filtering
///
/// Provides access to metadata records for filter evaluation.
pub trait MetadataLookup: Send + Sync + Debug {
    /// Get metadata for a single candidate ID
    fn get_metadata(&self, id: &str) -> Result<Option<serde_json::Value>>;

    /// Get metadata for multiple candidate IDs in batch
    fn get_metadata_batch(&self, ids: &[String]) -> Result<Vec<Option<serde_json::Value>>>;

    /// Check if this lookup source can efficiently support batch operations
    fn supports_batch_lookup(&self) -> bool;
}

/// Normalized filter implementation based on FilterExpression
#[derive(Debug, Clone)]
pub struct NormalizedFilter {
    /// The underlying filter expression
    pub expression: FilterExpression,

    /// Estimated selectivity (cached)
    selectivity: Option<f64>,

    /// Fields required by this filter
    required_fields: Vec<String>,
}

impl NormalizedFilter {
    /// Create a new normalized filter from a FilterExpression
    pub fn new(expression: FilterExpression) -> Self {
        Self::new_with_policy(expression, &FilterSelectivityPolicy::default())
    }

    /// Create a normalized filter with explicit selectivity policy.
    pub fn try_new_with_policy(
        expression: FilterExpression,
        policy: FilterSelectivityPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        Ok(Self::new_with_policy(expression, &policy))
    }

    fn new_with_policy(expression: FilterExpression, policy: &FilterSelectivityPolicy) -> Self {
        let required_fields = Self::extract_fields(&expression);
        let selectivity = Self::estimate_selectivity(&expression, policy);

        Self {
            expression,
            selectivity,
            required_fields,
        }
    }

    /// Extract field names from a filter expression
    fn extract_fields(expression: &FilterExpression) -> Vec<String> {
        match expression {
            FilterExpression::Comparison { field, .. } => vec![field.clone()],
            FilterExpression::And(exprs) | FilterExpression::Or(exprs) => {
                let mut fields = Vec::new();
                for expr in exprs {
                    fields.extend(Self::extract_fields(expr));
                }
                fields.sort();
                fields.dedup();
                fields
            }
            FilterExpression::Not(expr) => Self::extract_fields(expr),
        }
    }

    /// Estimate filter selectivity
    ///
    /// Returns a value between 0.0 (very selective) and 1.0 (not selective).
    fn estimate_selectivity(
        expression: &FilterExpression,
        policy: &FilterSelectivityPolicy,
    ) -> Option<f64> {
        match expression {
            FilterExpression::Comparison { operator, .. } => match operator {
                ComparisonOperator::Equals => Some(policy.equals),
                ComparisonOperator::NotEquals => Some(policy.not_equals),
                ComparisonOperator::GreaterThan
                | ComparisonOperator::GreaterThanOrEqual
                | ComparisonOperator::LessThan
                | ComparisonOperator::LessThanOrEqual => Some(policy.range),
                ComparisonOperator::In => Some(policy.in_list),
                ComparisonOperator::Between => Some(policy.between),
                _ => Some(policy.unknown),
            },
            FilterExpression::And(exprs) => {
                // AND filters are more selective (multiply selectivities)
                let selectivities: Vec<_> = exprs
                    .iter()
                    .filter_map(|expr| Self::estimate_selectivity(expr, policy))
                    .collect();

                if selectivities.is_empty() {
                    None
                } else {
                    Some(selectivities.iter().product::<f64>())
                }
            }
            FilterExpression::Or(exprs) => {
                // OR filters are less selective (combine with inclusion-exclusion)
                let selectivities: Vec<_> = exprs
                    .iter()
                    .filter_map(|expr| Self::estimate_selectivity(expr, policy))
                    .collect();

                if selectivities.is_empty() {
                    None
                } else {
                    // Simplified OR selectivity (ignores overlap)
                    let sum: f64 = selectivities.iter().sum();
                    Some(sum.min(1.0))
                }
            }
            FilterExpression::Not(expr) => {
                // NOT inverts selectivity
                Self::estimate_selectivity(expr, policy).map(|s| 1.0 - s)
            }
        }
    }
}

impl FilterContract for NormalizedFilter {
    fn estimated_selectivity(&self) -> f64 {
        self.selectivity.unwrap_or(0.5) // Default to 50% selectivity
    }

    fn can_pushdown(&self, engine: StorageEngineType) -> bool {
        // For now, all filters can potentially be pushed down
        // In production, you would check engine-specific capabilities
        match engine {
            StorageEngineType::HNSW | StorageEngineType::IVF => {
                // HNSW and IVF support filter pushdown for certain filter types
                matches!(self.expression, FilterExpression::Comparison { .. })
            }
            _ => true, // Other engines support all filters
        }
    }

    fn evaluate_row(&self, metadata: &serde_json::Value) -> Result<bool> {
        let metadata_map: std::collections::HashMap<String, serde_json::Value> =
            if let Some(obj) = metadata.as_object() {
                obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            } else {
                return Ok(false);
            };
        Ok(crate::core::search::json_comparison::evaluate_filter(
            &self.expression,
            &metadata_map,
        ))
    }

    fn evaluate_batch(&self, metadata_batch: &[serde_json::Value]) -> Result<BooleanArray> {
        let mut results = Vec::with_capacity(metadata_batch.len());

        for metadata in metadata_batch {
            let passes = self.evaluate_row(metadata)?;
            results.push(passes);
        }

        Ok(BooleanArray::from(results))
    }

    fn required_fields(&self) -> Vec<String> {
        self.required_fields.clone()
    }

    fn is_compatible_with(&self, other: &dyn FilterContract) -> bool {
        // Check if the required fields overlap
        let my_fields_vec = self.required_fields();
        let other_fields_vec = other.required_fields();
        let my_fields: HashSet<_> = my_fields_vec.iter().collect();
        let other_fields: HashSet<_> = other_fields_vec.iter().collect();

        // Filters are compatible if they don't have conflicting requirements
        my_fields.is_subset(&other_fields) || other_fields.is_subset(&my_fields)
    }

    fn clone_box(&self) -> Box<dyn FilterContract> {
        Box::new(self.clone())
    }

    fn as_string(&self) -> String {
        format!("{:?}", self.expression)
    }

    fn as_filter_expression(&self) -> Option<&FilterExpression> {
        Some(&self.expression)
    }
}

/// Simple in-memory candidate set implementation
#[derive(Debug, Clone)]
pub struct MemoryCandidateSet {
    /// Candidate IDs in this set
    candidates: Vec<String>,

    /// Optional scores for ranking
    scores: Vec<f32>,

    /// Use a HashSet for fast lookups
    lookup: HashSet<String>,
}

impl MemoryCandidateSet {
    /// Create a new empty candidate set
    pub fn new() -> Self {
        Self {
            candidates: Vec::new(),
            scores: Vec::new(),
            lookup: HashSet::new(),
        }
    }

    /// Create a candidate set from initial IDs
    pub fn from_ids(ids: Vec<String>) -> Self {
        let lookup: HashSet<_> = ids.iter().cloned().collect();
        let scores = vec![0.0; ids.len()]; // Default scores

        Self {
            candidates: ids,
            scores,
            lookup,
        }
    }
}

impl Default for MemoryCandidateSet {
    fn default() -> Self {
        Self::new()
    }
}

impl CandidateSet for MemoryCandidateSet {
    fn add_candidate(&mut self, id: String) -> Result<()> {
        if !self.lookup.contains(&id) {
            self.candidates.push(id.clone());
            self.scores.push(0.0);
            self.lookup.insert(id);
        }
        Ok(())
    }

    fn contains(&self, id: &str) -> bool {
        self.lookup.contains(id)
    }

    fn len(&self) -> usize {
        self.candidates.len()
    }

    fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn clear(&mut self) {
        self.candidates.clear();
        self.scores.clear();
        self.lookup.clear();
    }

    fn to_vec(&self) -> Vec<String> {
        self.candidates.clone()
    }

    fn filter(
        &self,
        contract: &dyn FilterContract,
        metadata_lookup: &dyn MetadataLookup,
    ) -> Result<Box<dyn CandidateSet>> {
        let mut filtered = MemoryCandidateSet::new();

        // Get metadata in batch if supported
        let metadata_batch = if metadata_lookup.supports_batch_lookup() {
            metadata_lookup.get_metadata_batch(&self.candidates)?
        } else {
            // Fallback to individual lookups
            self.candidates
                .iter()
                .map(|id| metadata_lookup.get_metadata(id).unwrap_or(None))
                .collect()
        };

        // Convert Vec<Option<Value>> to Vec<Value> for batch evaluation
        let metadata_values: Vec<serde_json::Value> =
            metadata_batch.into_iter().flatten().collect();

        // Evaluate filter for each candidate
        let filter_results = contract.evaluate_batch(&metadata_values)?;

        for (idx, passes) in filter_results.iter().enumerate() {
            if passes.unwrap_or(false) {
                filtered.add_candidate(self.candidates[idx].clone())?;
            }
        }

        debug!(
            "Filter reduced candidates from {} to {}",
            self.len(),
            filtered.len()
        );

        Ok(Box::new(filtered))
    }

    fn top_k(&self, k: usize, scores: &[f32]) -> Result<Box<dyn CandidateSet>> {
        if scores.len() != self.candidates.len() {
            return Err(anyhow::anyhow!(
                "Score count {} does not match candidate count {}",
                scores.len(),
                self.candidates.len()
            ));
        }

        // Create indexed candidates for sorting
        let mut indexed: Vec<_> = scores
            .iter()
            .zip(self.candidates.iter())
            .enumerate()
            .collect();

        // Sort by score (descending) and take top K
        indexed.sort_by(|a, b| {
            b.1.0
                .partial_cmp(a.1.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let top_candidates: Vec<String> = indexed
            .iter()
            .take(k)
            .map(|(_, (_, id))| (*id).clone())
            .collect();

        Ok(Box::new(MemoryCandidateSet::from_ids(top_candidates)))
    }

    fn union(&self, other: &dyn CandidateSet) -> Result<Box<dyn CandidateSet>> {
        let mut union_set = MemoryCandidateSet::new();

        // Add all candidates from self
        for id in &self.candidates {
            union_set.add_candidate(id.clone())?;
        }

        // Add all candidates from other
        for id in other.to_vec() {
            union_set.add_candidate(id)?;
        }

        Ok(Box::new(union_set))
    }

    fn intersect(&self, other: &dyn CandidateSet) -> Result<Box<dyn CandidateSet>> {
        let mut intersect_set = MemoryCandidateSet::new();

        // Only add candidates that are in both sets
        for id in &self.candidates {
            if other.contains(id) {
                intersect_set.add_candidate(id.clone())?;
            }
        }

        Ok(Box::new(intersect_set))
    }

    fn clone_box(&self) -> Box<dyn CandidateSet> {
        Box::new(self.clone())
    }
}

/// Convenience function to create a normalized filter
pub fn normalize_filter(expression: FilterExpression) -> Box<dyn FilterContract> {
    Box::new(NormalizedFilter::new(expression))
}

/// Convenience function to create a normalized filter with explicit heuristics.
pub fn normalize_filter_with_policy(
    expression: FilterExpression,
    policy: FilterSelectivityPolicy,
) -> Result<Box<dyn FilterContract>> {
    Ok(Box::new(NormalizedFilter::try_new_with_policy(
        expression, policy,
    )?))
}

/// Convenience function to create an empty candidate set
pub fn create_candidate_set() -> Box<dyn CandidateSet> {
    Box::new(MemoryCandidateSet::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::ComparisonOperator;

    #[test]
    fn test_normalized_filter_creation() {
        let expression = FilterExpression::Comparison {
            field: "price".to_string(),
            operator: ComparisonOperator::LessThan,
            value: serde_json::json!(1000),
        };

        let filter = NormalizedFilter::new(expression);

        assert_eq!(filter.required_fields(), vec!["price"]);
        assert_eq!(filter.estimated_selectivity(), 0.5);
    }

    #[test]
    fn test_normalized_filter_selectivity() {
        // Equality filters are more selective
        let eq_filter = NormalizedFilter::new(FilterExpression::Comparison {
            field: "status".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("active"),
        });
        assert_eq!(eq_filter.estimated_selectivity(), 0.1);

        // Range filters are less selective
        let range_filter = NormalizedFilter::new(FilterExpression::Comparison {
            field: "price".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: serde_json::json!(100),
        });
        assert_eq!(range_filter.estimated_selectivity(), 0.5);
    }

    #[test]
    fn test_filter_selectivity_policy_validation() {
        let mut policy = FilterSelectivityPolicy::default();
        assert!(policy.validate().is_ok());

        policy.equals = 1.2;
        assert!(policy.validate().is_err());
    }

    #[test]
    fn test_normalized_filter_uses_configured_selectivity_policy() {
        let policy = FilterSelectivityPolicy {
            equals: 0.25,
            range: 0.75,
            ..Default::default()
        };
        let eq_filter = NormalizedFilter::try_new_with_policy(
            FilterExpression::Comparison {
                field: "status".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("active"),
            },
            policy.clone(),
        )
        .unwrap();
        assert_eq!(eq_filter.estimated_selectivity(), 0.25);

        let range_filter = normalize_filter_with_policy(
            FilterExpression::Comparison {
                field: "price".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: serde_json::json!(100),
            },
            policy,
        )
        .unwrap();
        assert_eq!(range_filter.estimated_selectivity(), 0.75);
    }

    #[test]
    fn test_normalized_filter_required_fields() {
        let expression = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "price".to_string(),
                operator: ComparisonOperator::LessThan,
                value: serde_json::json!(1000),
            },
            FilterExpression::Comparison {
                field: "category".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("electronics"),
            },
        ]);

        let filter = NormalizedFilter::new(expression);
        let mut fields = filter.required_fields();
        fields.sort();

        assert_eq!(fields, vec!["category", "price"]);
    }

    #[test]
    fn test_memory_candidate_set() {
        let mut candidates = MemoryCandidateSet::new();

        assert!(candidates.is_empty());
        assert_eq!(candidates.len(), 0);

        candidates.add_candidate("id1".to_string()).unwrap();
        candidates.add_candidate("id2".to_string()).unwrap();
        candidates.add_candidate("id3".to_string()).unwrap();

        assert_eq!(candidates.len(), 3);
        assert!(candidates.contains("id1"));
        assert!(!candidates.contains("id4"));

        candidates.clear();
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_memory_candidate_set_from_ids() {
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let candidates = MemoryCandidateSet::from_ids(ids.clone());

        assert_eq!(candidates.len(), 3);
        assert!(candidates.contains("a"));
        assert!(candidates.contains("b"));
        assert!(candidates.contains("c"));
    }

    #[test]
    fn test_memory_candidate_set_union() {
        let set1 = MemoryCandidateSet::from_ids(vec!["a".to_string(), "b".to_string()]);
        let set2 = MemoryCandidateSet::from_ids(vec!["b".to_string(), "c".to_string()]);

        let union = set1.union(&set2).unwrap();
        assert_eq!(union.len(), 3); // a, b, c
    }

    #[test]
    fn test_memory_candidate_set_intersect() {
        let set1 =
            MemoryCandidateSet::from_ids(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        let set2 =
            MemoryCandidateSet::from_ids(vec!["b".to_string(), "c".to_string(), "d".to_string()]);

        let intersection = set1.intersect(&set2).unwrap();
        assert_eq!(intersection.len(), 2); // b, c
    }

    #[test]
    fn test_memory_candidate_set_top_k() {
        let ids = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let scores = vec![0.1, 0.9, 0.5, 0.3];

        let candidates = MemoryCandidateSet::from_ids(ids);
        let top_k = candidates.top_k(2, &scores).unwrap();

        assert_eq!(top_k.len(), 2);
        let top_ids = top_k.to_vec();
        assert_eq!(top_ids[0], "b"); // Highest score (0.9)
        assert_eq!(top_ids[1], "c"); // Second highest (0.5)
    }

    #[test]
    fn test_normalize_filter_convenience() {
        let expression = FilterExpression::Comparison {
            field: "status".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("active"),
        };

        let filter = normalize_filter(expression);
        assert_eq!(filter.estimated_selectivity(), 0.1);
    }

    #[test]
    fn test_create_candidate_set_convenience() {
        let candidates = create_candidate_set();
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_normalized_filter_exposes_expression() {
        let expr = FilterExpression::Comparison {
            field: "id".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("abc"),
        };
        let filter = NormalizedFilter::new(expr.clone());
        let retrieved = filter.as_filter_expression();
        assert!(
            retrieved.is_some(),
            "NormalizedFilter must expose its expression"
        );
        assert_eq!(*retrieved.unwrap(), expr);
    }

    #[test]
    fn test_default_filter_contract_returns_none_expression() {
        // Ensure the default impl returns None for impls that don't override it.
        struct Stub;
        impl std::fmt::Debug for Stub {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "Stub")
            }
        }
        impl FilterContract for Stub {
            fn estimated_selectivity(&self) -> f64 {
                0.5
            }
            fn can_pushdown(&self, _: StorageEngineType) -> bool {
                false
            }
            fn evaluate_row(&self, _: &serde_json::Value) -> Result<bool> {
                Ok(true)
            }
            fn evaluate_batch(
                &self,
                batch: &[serde_json::Value],
            ) -> Result<arrow_array::BooleanArray> {
                Ok(arrow_array::BooleanArray::from(vec![true; batch.len()]))
            }
            fn required_fields(&self) -> Vec<String> {
                vec![]
            }
            fn is_compatible_with(&self, _: &dyn FilterContract) -> bool {
                true
            }
            fn clone_box(&self) -> Box<dyn FilterContract> {
                Box::new(Stub)
            }
            fn as_string(&self) -> String {
                "stub".to_string()
            }
        }
        let stub = Stub;
        assert!(stub.as_filter_expression().is_none());
    }
}
