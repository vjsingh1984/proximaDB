//! The single compiled predicate object shared by every AXIS filtered-ANN path.
//!
//! F7 / TD-HYBRID-1 (D2). Today the Inline HNSW walk, its post-search residual
//! re-filter, and the exact PreFilter branch each hand-roll the *same* predicate:
//! an `id_filters` membership gate ANDed with a metadata [`FilterExpression`]
//! evaluated by [`json_comparison::evaluate_filter`]. They already converge on
//! one builder and one evaluator, so keeping three copies of the gluing logic is
//! a divergence risk, not a feature. [`CompiledGlobalFilter`] is the extraction:
//! one object, one [`CompiledGlobalFilter::matches`], consumed by all three.
//!
//! Deliberately **scoped to the id + metadata predicate only**. The similarity
//! threshold and expiry checks are PreFilter-specific (they need the record's
//! vector and `valid_to`, not just its filter metadata) and must stay on that
//! path — folding them in here would manufacture the very divergence this type
//! removes.
//!
//! [`json_comparison::evaluate_filter`]: crate::json_comparison::evaluate_filter
//! [`FilterExpression`]: proximadb_filter_expression::FilterExpression

use std::collections::HashMap;

use proximadb_filter_expression::FilterExpression;
use serde_json::Value;

use crate::json_comparison::evaluate_filter;

/// The compiled id + metadata predicate for one filtered query.
///
/// Construct once per query from the resolved id filters and the (optional)
/// metadata expression, then evaluate every candidate through
/// [`matches`](Self::matches). The semantics mirror the pre-extraction closure
/// in `AxisManager::query_hnsw_with_predicate` exactly:
///
/// * no id filters and no expression → matches everything ([`is_empty`]);
/// * a non-empty id filter that does not contain the candidate → no match;
/// * an expression present but the candidate has no filter metadata → no match
///   (a metadata predicate cannot be satisfied by an absent metadata map);
/// * otherwise → whatever [`evaluate_filter`] says.
///
/// [`is_empty`]: Self::is_empty
#[derive(Debug, Clone, Default)]
pub struct CompiledGlobalFilter {
    id_filters: Vec<String>,
    metadata_expression: Option<FilterExpression>,
}

impl CompiledGlobalFilter {
    /// Build from the resolved id filters and optional metadata expression.
    pub fn new(id_filters: Vec<String>, metadata_expression: Option<FilterExpression>) -> Self {
        Self {
            id_filters,
            metadata_expression,
        }
    }

    /// True when there is nothing to filter — no id gate and no metadata
    /// expression. Callers can skip metadata materialization entirely in this
    /// case, exactly as the pre-extraction paths did.
    pub fn is_empty(&self) -> bool {
        self.id_filters.is_empty() && self.metadata_expression.is_none()
    }

    /// True when this filter carries a metadata expression (so callers know
    /// whether they must materialize per-candidate metadata at all).
    pub fn has_metadata_predicate(&self) -> bool {
        self.metadata_expression.is_some()
    }

    /// The id-membership half of the predicate, in isolation. An empty id-filter
    /// list admits everything.
    pub fn admits_id(&self, id: &str) -> bool {
        self.id_filters.is_empty() || self.id_filters.iter().any(|candidate| candidate == id)
    }

    /// Evaluate the full predicate for one candidate.
    ///
    /// `metadata` is `None` when the caller has no filter metadata for this id
    /// (e.g. it was evicted between snapshotting the candidate set and
    /// evaluating it). A candidate with no metadata cannot satisfy a metadata
    /// expression, so it fails closed — matching the original closure.
    pub fn matches(&self, id: &str, metadata: Option<&HashMap<String, Value>>) -> bool {
        if !self.admits_id(id) {
            return false;
        }

        let Some(expr) = &self.metadata_expression else {
            return true;
        };
        let Some(metadata) = metadata else {
            return false;
        };
        evaluate_filter(expr, metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_filter_expression::{ComparisonOperator, FilterExpression};
    use serde_json::json;

    fn meta(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn eq_expr(field: &str, value: Value) -> FilterExpression {
        FilterExpression::And(vec![FilterExpression::Comparison {
            field: field.to_string(),
            operator: ComparisonOperator::Equals,
            value,
        }])
    }

    #[test]
    fn empty_filter_admits_everything() {
        let f = CompiledGlobalFilter::default();
        assert!(f.is_empty());
        assert!(f.matches("anything", None));
        assert!(f.matches("anything", Some(&meta(&[("k", json!("v"))]))));
    }

    #[test]
    fn id_filter_only_gates_on_membership() {
        let f = CompiledGlobalFilter::new(vec!["a".into(), "b".into()], None);
        assert!(!f.is_empty());
        assert!(f.matches("a", None));
        assert!(f.matches("b", None));
        assert!(!f.matches("c", None));
    }

    #[test]
    fn expression_only_evaluates_metadata() {
        let f = CompiledGlobalFilter::new(vec![], Some(eq_expr("tier", json!("gold"))));
        assert!(f.has_metadata_predicate());
        assert!(f.matches("x", Some(&meta(&[("tier", json!("gold"))]))));
        assert!(!f.matches("x", Some(&meta(&[("tier", json!("silver"))]))));
    }

    #[test]
    fn expression_with_absent_metadata_fails_closed() {
        // Mirrors the original closure: Some(expr) + None(metadata) => false.
        let f = CompiledGlobalFilter::new(vec![], Some(eq_expr("tier", json!("gold"))));
        assert!(!f.matches("x", None));
        // A present-but-missing-field metadata map defers to evaluate_filter,
        // which treats an absent field as not-equal (SQL null-on-absence).
        assert!(!f.matches("x", Some(&meta(&[("other", json!(1))]))));
    }

    #[test]
    fn id_and_expression_are_anded() {
        let f = CompiledGlobalFilter::new(vec!["a".into()], Some(eq_expr("tier", json!("gold"))));
        let gold = meta(&[("tier", json!("gold"))]);
        let silver = meta(&[("tier", json!("silver"))]);
        assert!(f.matches("a", Some(&gold))); // id ok + expr ok
        assert!(!f.matches("a", Some(&silver))); // id ok + expr fail
        assert!(!f.matches("b", Some(&gold))); // id fail short-circuits
    }

    #[test]
    fn admits_id_matches_the_membership_half() {
        let f = CompiledGlobalFilter::new(vec!["a".into()], None);
        assert!(f.admits_id("a"));
        assert!(!f.admits_id("z"));
        let open = CompiledGlobalFilter::default();
        assert!(open.admits_id("anything"));
    }
}
