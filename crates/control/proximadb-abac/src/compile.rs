// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! The compile bridge (FA-2 / TF-2 §1.4): turn `row_predicate_refs` (`ObjectId`s)
//! into an executable `FilterExpression` that `admits_with_security` evaluates.
//!
//! ## What this is
//!
//! `PolicyBinding.predicate_ref` is an `Option<ObjectId>` pointing at a predicate
//! object — a stored `FilterExpression` that encodes a row-level rule (e.g.
//! `dept == "eng"`). `EffectivePolicy.predicate_refs` is the list of applicable
//! ones. This module resolves those refs against a [`PredicateObjectStore`] and
//! ANDs them into one expression, the "security" half of
//! [`admits_with_security`](../../../../../src/security/rls/filter_lattice.rs).
//!
//! ## Fail-closed
//!
//! A missing ref is a **deny**: the policy references a predicate object the store
//! cannot find, so the safe answer is "admit nothing." The compiled expression is
//! an explicit contradiction (`IsNull(f) ∧ IsNotNull(f)`), never `None`, so the
//! caller cannot accidentally treat a broken policy as "no restriction."
//!
//! ## Why `FilterExpression`, not `SecurityPredicate`
//!
//! The abac crate (control layer) cannot import the root crate's `SecurityPredicate`
//! or `SecurityFilter` types. The predicate object's canonical form is therefore a
//! `FilterExpression` (foundation layer) — the same type `admits_with_security`
//! takes. The lattice lowering (`SecurityPredicate → SecurityFilter → FilterExpression`,
//! in the root crate's `filter_lattice.rs`) happens at *registration* time, not at
//! *compile* time; the store holds the already-lowered `FilterExpression`.

use std::collections::BTreeMap;

use proximadb_catalog::fc_metamodel::ObjectId;
use proximadb_filter_expression::{ComparisonOperator, FilterExpression};

/// Reserved field name for the synthetic unsatisfiable expression. Never read
/// from a record — the expression is a contradiction over *presence*.
const UNSATISFIABLE_FIELD: &str = "__proximadb_abac_unsatisfiable__";

/// An expression that admits no row. `IsNull(f) ∧ IsNotNull(f)` is a genuine
/// contradiction under the evaluator's own presence semantics (an absent field
/// IS NULL; so exactly one of the two holds for every record).
fn unsatisfiable_filter() -> FilterExpression {
    FilterExpression::And(vec![
        FilterExpression::Comparison {
            field: UNSATISFIABLE_FIELD.to_string(),
            operator: ComparisonOperator::IsNull,
            value: serde_json::Value::Null,
        },
        FilterExpression::Comparison {
            field: UNSATISFIABLE_FIELD.to_string(),
            operator: ComparisonOperator::IsNotNull,
            value: serde_json::Value::Null,
        },
    ])
}

/// Resolves `ObjectId` predicate-refs to their stored `FilterExpression`.
///
/// **Structurally tenant-scoped**: an implementation is built for one tenant and
/// only sees that tenant's predicate objects. A cross-tenant ref fails to resolve
/// (returns `None`), and [`compile_security_filter`] turns that into a deny.
pub trait PredicateObjectStore {
    /// Look up the predicate object registered under `id`. Returns `None` when
    /// unknown, revoked, or in another tenant's scope — [`compile_security_filter`]
    /// treats `None` as fail-closed.
    fn get(&self, id: ObjectId) -> Option<&FilterExpression>;
}

/// An in-memory [`PredicateObjectStore`] — the reference implementation.
pub struct InMemoryPredicateObjectStore {
    objects: BTreeMap<ObjectId, FilterExpression>,
}

impl InMemoryPredicateObjectStore {
    /// An empty store (every ref fails to resolve → deny).
    pub fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
        }
    }

    /// Register or replace a predicate object.
    pub fn register(&mut self, id: ObjectId, expr: FilterExpression) {
        self.objects.insert(id, expr);
    }

    /// Remove a predicate object. Subsequent resolves of `id` fail-closed.
    pub fn revoke(&mut self, id: ObjectId) {
        self.objects.remove(&id);
    }
}

impl Default for InMemoryPredicateObjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PredicateObjectStore for InMemoryPredicateObjectStore {
    fn get(&self, id: ObjectId) -> Option<&FilterExpression> {
        self.objects.get(&id)
    }
}

/// Compile `refs` into a single security `FilterExpression` by resolving each
/// ref against `store` and ANDing the results.
///
/// Returns `None` when `refs` is empty — no applicable predicates means no
/// row restriction *at the policy level* (the subject is still admitted/denied
/// by the `ReadDecision`; this only governs which rows are visible).
///
/// **Fail-closed**: if ANY ref is missing from the store, returns
/// `Some(unsatisfiable_filter())` — a broken policy reference denies
/// everything rather than silently admitting.
pub fn compile_security_filter(
    refs: &[ObjectId],
    store: &dyn PredicateObjectStore,
) -> Option<FilterExpression> {
    if refs.is_empty() {
        return None;
    }

    let mut resolved: Vec<FilterExpression> = Vec::with_capacity(refs.len());
    for id in refs {
        match store.get(*id) {
            Some(expr) => resolved.push(expr.clone()),
            None => {
                // A missing predicate ref is a deny — the policy references
                // something the store cannot find, and the safe answer is
                // "admit nothing," not "no restriction."
                return Some(unsatisfiable_filter());
            }
        }
    }

    Some(match resolved.len() {
        1 => resolved.into_iter().next().expect("len == 1"),
        _ => FilterExpression::And(resolved),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_filter_expression::ComparisonOperator;

    fn eq_dept(value: &str) -> FilterExpression {
        FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String(value.to_string()),
        }
    }

    fn gt_clearance(level: i64) -> FilterExpression {
        FilterExpression::Comparison {
            field: "clearance".to_string(),
            operator: ComparisonOperator::GreaterThanOrEqual,
            value: serde_json::Value::Number(level.into()),
        }
    }

    #[test]
    fn empty_refs_produce_no_restriction() {
        let store = InMemoryPredicateObjectStore::new();
        assert_eq!(compile_security_filter(&[], &store), None);
    }

    #[test]
    fn a_single_ref_compiles_to_its_expression() {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(42, eq_dept("eng"));
        let compiled = compile_security_filter(&[42], &store).expect("resolved");
        assert_eq!(compiled, eq_dept("eng"));
    }

    #[test]
    fn multiple_refs_are_anded() {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(1, eq_dept("eng"));
        store.register(2, gt_clearance(3));
        let compiled = compile_security_filter(&[1, 2], &store).expect("resolved");
        match compiled {
            FilterExpression::And(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], eq_dept("eng"));
                assert_eq!(parts[1], gt_clearance(3));
            }
            _ => panic!("expected And of two predicates"),
        }
    }

    #[test]
    fn a_missing_ref_is_fail_closed() {
        let store = InMemoryPredicateObjectStore::new();
        let compiled = compile_security_filter(&[999], &store).expect("fail-closed yields an expr");
        // Must be unsatisfiable, not None (None would mean "no restriction").
        assert!(
            !matches!(compiled, FilterExpression::Comparison { .. }),
            "a missing ref must not silently pass through as a single comparison"
        );
    }

    #[test]
    fn one_missing_ref_among_many_denies_all() {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(1, eq_dept("eng"));
        // ref 2 is missing
        let compiled = compile_security_filter(&[1, 2], &store).expect("an expr");
        // Must be the unsatisfiable contradiction, not the partial AND.
        match &compiled {
            FilterExpression::And(parts) => {
                assert!(parts.len() == 2, "the unsatisfiable contradiction");
                assert!(parts.iter().any(|p| matches!(
                    p,
                    FilterExpression::Comparison {
                        operator: ComparisonOperator::IsNull,
                        ..
                    }
                )));
                assert!(parts.iter().any(|p| matches!(
                    p,
                    FilterExpression::Comparison {
                        operator: ComparisonOperator::IsNotNull,
                        ..
                    }
                )));
            }
            _ => panic!("expected the unsatisfiable And"),
        }
    }

    #[test]
    fn revocation_takes_effect() {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(1, eq_dept("eng"));
        assert!(compile_security_filter(&[1], &store).is_some());

        store.revoke(1);
        let compiled = compile_security_filter(&[1], &store).expect("an expr");
        match compiled {
            FilterExpression::And(parts) => assert_eq!(parts.len(), 2), // unsatisfiable
            _ => panic!("revoked ref should deny"),
        }
    }

    #[test]
    fn the_unsatisfiable_expression_genuinely_denies() {
        // Structural proof: it is IsNull(f) ∧ IsNotNull(f), which cannot both hold.
        let expr = unsatisfiable_filter();
        match expr {
            FilterExpression::And(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(parts.iter().any(|p| matches!(
                    p,
                    FilterExpression::Comparison {
                        operator: ComparisonOperator::IsNull,
                        ..
                    }
                )));
                assert!(parts.iter().any(|p| matches!(
                    p,
                    FilterExpression::Comparison {
                        operator: ComparisonOperator::IsNotNull,
                        ..
                    }
                )));
            }
            _ => panic!("unsatisfiable must be an And"),
        }
    }
}
