// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! The total, deny-biased lattice a security predicate lowers into (FA-a / TF-2 S1–S2).
//!
//! # Why a lattice and not `Option<FilterExpression>`
//!
//! The bridge used to lower a [`SecurityPredicate`] to
//! `Result<Option<FilterExpression>>`, where `None` meant *no restriction*. That
//! third outcome is an inversion channel, because `None` is also what every
//! "nothing to say" path produces:
//!
//! * `AlwaysAllow` → `None`, so `Not(AlwaysAllow)` — semantically **deny-all** —
//!   saw an inner `None`, had nothing to negate, and returned `None`: the **full
//!   table**. Deny-all silently inverted to admit-all.
//! * an empty `And`/`Or` → `None` → full table.
//!
//! [`SecurityFilter`] makes the three outcomes distinct and total, so "no
//! restriction" is producible **only** by a predicate that genuinely permits
//! everything, and every deny-derived node lands on
//! [`SecurityFilter::Unsatisfiable`] instead of falling through to unfiltered.
//!
//! TF-2 S1 phrases the fix as "`build_filter` returns `Result<FilterExpression>`
//! — no `Ok(None)`". A bare `FilterExpression` would have to encode "no
//! restriction" as a tautology, which every row of every scan would then
//! evaluate for nothing. The lattice satisfies the *property* S1 is after (no
//! deny-derived node can reach the unfiltered outcome) without that cost, and
//! keeps the distinction the scan actually wants.
//!
//! # The NULL guard (S2)
//!
//! The shared evaluator negates with a bare boolean `!`
//! (`sql_value_filter::evaluate_filter_resolved`). A value comparison over an
//! absent field is `false`, so `Not(false)` **admits** the row: a
//! `Not(Eq(classification, "TS"))` policy leaks every record that has no
//! `classification` field at all.
//!
//! Rather than change the evaluator — it is live under metadata search, document
//! filtering and the pushdown paths, where SQL-3VL is a separate, user-visible
//! semantics decision — negation is lowered null-safe **here**:
//!
//! ```text
//! Not(P)  ⇒  And([ Not(P), IsNotNull(f₁), …, IsNotNull(fₙ) ])
//! ```
//!
//! over each field `P` compares by **value**. Null tests (`IsNull`/`IsNotNull`)
//! are deliberately excluded from the guard set: they are already
//! absence-aware, and guarding them would turn `Not(IsNotNull(f))` — correctly
//! `f IS NULL` — into a contradiction.

use std::collections::BTreeSet;

use crate::core::search::{ComparisonOperator, FilterExpression};

/// Reserved field name for the synthetic unsatisfiable expression. It is never
/// read from a record — the expression is a contradiction over *presence*, so it
/// is false whatever the field holds (or whether it exists).
const UNSATISFIABLE_FIELD: &str = "__proximadb_rls_unsatisfiable__";

/// An expression that admits no row, for any record.
///
/// `IsNull(f) AND IsNotNull(f)` is a genuine contradiction under the evaluator's
/// own semantics: the two operators are exact complements, decided by field
/// *presence* (an absent field IS NULL), so exactly one holds for every record
/// and the conjunction is always false.
///
/// This deliberately replaces the previous `field == "__rls_access_denied__"`
/// sentinel, which was a *value* comparison — one record carrying that literal
/// string would have satisfied a denial filter and leaked.
pub fn unsatisfiable_expression() -> FilterExpression {
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

/// What a security predicate lowers to. Total: every predicate maps to exactly
/// one of these, and there is no "nothing to say" case that means *unfiltered*.
#[derive(Debug, Clone, PartialEq)]
pub enum SecurityFilter {
    /// The subject may see every row. **Only** a predicate that genuinely
    /// permits everything (`AlwaysAllow`, a satisfied `RoleBased`) produces this.
    Unrestricted,
    /// The subject may see rows matching this expression.
    Restricted(FilterExpression),
    /// The subject may see nothing. Lowers to [`unsatisfiable_expression`] —
    /// never to "no filter".
    Unsatisfiable,
}

impl SecurityFilter {
    /// Build a restriction from a single comparison, **null-guarding negative
    /// operators**.
    ///
    /// `Not` is not the only negation in the language: `NotEquals` and `NotIn`
    /// are negations spelled as operators, and they leak the same way. The
    /// evaluator compares an absent-or-null field as *not equal* to any literal,
    /// so a `dept != "eng"` policy admits every record with no `dept` at all —
    /// TF-2 §1.4's "`NotEquals` deny lowers to admit". A positive operator needs
    /// no guard: a missing field simply fails the comparison and the row is
    /// denied, which is the direction we want.
    ///
    /// This is the constructor `build_filter` uses for every comparison, so the
    /// guard cannot be forgotten at one call site.
    pub fn comparison(
        field: String,
        operator: ComparisonOperator,
        value: serde_json::Value,
    ) -> SecurityFilter {
        let negative = matches!(
            operator,
            ComparisonOperator::NotEquals | ComparisonOperator::NotIn
        );
        let comparison = FilterExpression::Comparison {
            field: field.clone(),
            operator,
            value,
        };
        if negative {
            SecurityFilter::Restricted(FilterExpression::And(vec![
                comparison,
                FilterExpression::Comparison {
                    field,
                    operator: ComparisonOperator::IsNotNull,
                    value: serde_json::Value::Null,
                },
            ]))
        } else {
            SecurityFilter::Restricted(comparison)
        }
    }

    /// Conjunction, deny-biased.
    ///
    /// * any `Unsatisfiable` ⇒ `Unsatisfiable` (a deny anywhere in an AND wins);
    /// * `Unrestricted` members drop out (they restrict nothing);
    /// * **an empty input is `Unsatisfiable`**, not vacuously true — an
    ///   `And([])` in a policy is a malformed policy, and a malformed policy
    ///   denies (TF-2 S1). Note this is *not* the same as "every member was
    ///   `Unrestricted`", which correctly yields `Unrestricted`.
    pub fn all(parts: Vec<SecurityFilter>) -> SecurityFilter {
        if parts.is_empty() {
            return SecurityFilter::Unsatisfiable;
        }
        let mut exprs = Vec::new();
        for part in parts {
            match part {
                SecurityFilter::Unsatisfiable => return SecurityFilter::Unsatisfiable,
                SecurityFilter::Unrestricted => {}
                SecurityFilter::Restricted(e) => exprs.push(e),
            }
        }
        match exprs.len() {
            0 => SecurityFilter::Unrestricted, // every member permitted everything
            1 => SecurityFilter::Restricted(
                exprs.pop().unwrap_or_else(unsatisfiable_expression), // len checked; deny if not
            ),
            _ => SecurityFilter::Restricted(FilterExpression::And(exprs)),
        }
    }

    /// Disjunction.
    ///
    /// * any `Unrestricted` ⇒ `Unrestricted` (an OR with a permit-all branch
    ///   permits all);
    /// * `Unsatisfiable` members drop out;
    /// * an empty input, or one where every member was `Unsatisfiable`, is
    ///   `Unsatisfiable`.
    pub fn any(parts: Vec<SecurityFilter>) -> SecurityFilter {
        if parts.is_empty() {
            return SecurityFilter::Unsatisfiable;
        }
        let mut exprs = Vec::new();
        for part in parts {
            match part {
                SecurityFilter::Unrestricted => return SecurityFilter::Unrestricted,
                SecurityFilter::Unsatisfiable => {}
                SecurityFilter::Restricted(e) => exprs.push(e),
            }
        }
        match exprs.len() {
            0 => SecurityFilter::Unsatisfiable, // every branch denied
            1 => SecurityFilter::Restricted(
                exprs.pop().unwrap_or_else(unsatisfiable_expression), // len checked; deny if not
            ),
            _ => SecurityFilter::Restricted(FilterExpression::Or(exprs)),
        }
    }

    /// Negation — the operation the old `Option` shape got wrong.
    ///
    /// `Unrestricted` negates to `Unsatisfiable` (this is the `Not(AlwaysAllow)`
    /// inversion, closed), `Unsatisfiable` to `Unrestricted`, and a restriction
    /// to its **null-guarded** complement (S2).
    pub fn negate(self) -> SecurityFilter {
        match self {
            SecurityFilter::Unrestricted => SecurityFilter::Unsatisfiable,
            SecurityFilter::Unsatisfiable => SecurityFilter::Unrestricted,
            SecurityFilter::Restricted(e) => SecurityFilter::Restricted(null_guarded_not(e)),
        }
    }

    /// The expression to AND into a scan: `None` **only** for `Unrestricted`.
    /// `Unsatisfiable` yields a real, always-false expression, so a caller that
    /// forwards this straight to a scan cannot accidentally read the full table.
    pub fn into_expression(self) -> Option<FilterExpression> {
        match self {
            SecurityFilter::Unrestricted => None,
            SecurityFilter::Restricted(e) => Some(e),
            SecurityFilter::Unsatisfiable => Some(unsatisfiable_expression()),
        }
    }

    /// Whether this admits nothing.
    pub fn is_unsatisfiable(&self) -> bool {
        matches!(self, SecurityFilter::Unsatisfiable)
    }
}

/// `Not(e)` conjoined with an `IsNotNull` guard per value-compared field, so a
/// record missing one of those fields is **excluded** rather than admitted by
/// the evaluator's boolean `!`.
fn null_guarded_not(e: FilterExpression) -> FilterExpression {
    let mut guarded = vec![FilterExpression::Not(Box::new(e.clone()))];
    for field in value_compared_fields(&e) {
        guarded.push(FilterExpression::Comparison {
            field,
            operator: ComparisonOperator::IsNotNull,
            value: serde_json::Value::Null,
        });
    }
    match guarded.len() {
        // No value comparison inside (e.g. a bare null test) — nothing to guard.
        1 => FilterExpression::Not(Box::new(e)),
        _ => FilterExpression::And(guarded),
    }
}

/// Every field the expression compares **by value**, in stable order.
///
/// `IsNull`/`IsNotNull` are excluded: they are decided by presence and are
/// already absence-correct, so guarding them would make `Not(IsNotNull(f))`
/// (correctly `f IS NULL`) unsatisfiable.
fn value_compared_fields(e: &FilterExpression) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_value_compared_fields(e, &mut out);
    out
}

fn collect_value_compared_fields(e: &FilterExpression, out: &mut BTreeSet<String>) {
    match e {
        FilterExpression::Comparison {
            field, operator, ..
        } => {
            if !matches!(
                operator,
                ComparisonOperator::IsNull | ComparisonOperator::IsNotNull
            ) {
                out.insert(field.clone());
            }
        }
        FilterExpression::And(parts) | FilterExpression::Or(parts) => {
            for p in parts {
                collect_value_compared_fields(p, out);
            }
        }
        FilterExpression::Not(inner) => collect_value_compared_fields(inner, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::sql_value_filter::evaluate_filter_resolved;
    use serde_json::json;

    fn eq_expr(field: &str, value: serde_json::Value) -> FilterExpression {
        FilterExpression::Comparison {
            field: field.to_string(),
            operator: ComparisonOperator::Equals,
            value,
        }
    }

    fn restricted(field: &str, value: serde_json::Value) -> SecurityFilter {
        SecurityFilter::Restricted(eq_expr(field, value))
    }

    /// Evaluate an expression against a record represented as a JSON map.
    fn admits(expr: &FilterExpression, row: &serde_json::Value) -> bool {
        evaluate_filter_resolved(expr, &|field| row.get(field).cloned())
    }

    fn admits_filter(f: &SecurityFilter, row: &serde_json::Value) -> bool {
        match f.clone().into_expression() {
            None => true, // Unrestricted
            Some(e) => admits(&e, row),
        }
    }

    // --- the unsatisfiable expression is really unsatisfiable ---

    #[test]
    fn the_unsatisfiable_expression_admits_no_row() {
        let expr = unsatisfiable_expression();
        for row in [
            json!({}),
            json!({ "a": 1 }),
            json!({ UNSATISFIABLE_FIELD: "anything" }),
            json!({ UNSATISFIABLE_FIELD: null }),
            // The value that would have satisfied the old sentinel filter.
            json!({ "classification": "__rls_access_denied__" }),
        ] {
            assert!(!admits(&expr, &row), "admitted row {row}");
        }
    }

    // --- S1: the Not(AlwaysAllow) inversion ---

    #[test]
    fn negating_unrestricted_denies_instead_of_returning_the_full_table() {
        // This is the round-2 break: deny-all inverted to admit-all because
        // `None` meant both "nothing to negate" and "no restriction".
        let denied = SecurityFilter::Unrestricted.negate();
        assert_eq!(denied, SecurityFilter::Unsatisfiable);
        assert!(!admits_filter(&denied, &json!({ "a": 1 })));
        // …and it is a real expression, not "no filter".
        assert!(denied.into_expression().is_some());
    }

    #[test]
    fn negating_unsatisfiable_permits_everything() {
        assert_eq!(
            SecurityFilter::Unsatisfiable.negate(),
            SecurityFilter::Unrestricted
        );
    }

    #[test]
    fn double_negation_of_a_restriction_still_admits_a_subset() {
        // Not(Not(P)) is not required to be exactly P — only to stay deny-biased.
        let p = restricted("dept", json!("eng"));
        let dd = p.clone().negate().negate();
        for row in [json!({"dept": "eng"}), json!({"dept": "hr"}), json!({})] {
            if admits_filter(&dd, &row) {
                assert!(
                    admits_filter(&p, &row),
                    "double negation admitted a row the source denies: {row}"
                );
            }
        }
    }

    // --- S1: empty and all-permit conjunctions ---

    #[test]
    fn an_empty_conjunction_denies_rather_than_being_vacuously_true() {
        assert_eq!(SecurityFilter::all(vec![]), SecurityFilter::Unsatisfiable);
        assert_eq!(SecurityFilter::any(vec![]), SecurityFilter::Unsatisfiable);
    }

    #[test]
    fn a_conjunction_of_permit_alls_still_permits_all() {
        // Distinct from the empty case: these members each said "no restriction".
        assert_eq!(
            SecurityFilter::all(vec![
                SecurityFilter::Unrestricted,
                SecurityFilter::Unrestricted
            ]),
            SecurityFilter::Unrestricted
        );
    }

    #[test]
    fn one_deny_anywhere_in_a_conjunction_wins() {
        assert_eq!(
            SecurityFilter::all(vec![
                restricted("dept", json!("eng")),
                SecurityFilter::Unsatisfiable,
                SecurityFilter::Unrestricted,
            ]),
            SecurityFilter::Unsatisfiable
        );
    }

    #[test]
    fn a_permit_all_branch_of_a_disjunction_permits_all() {
        assert_eq!(
            SecurityFilter::any(vec![
                restricted("dept", json!("eng")),
                SecurityFilter::Unrestricted,
            ]),
            SecurityFilter::Unrestricted
        );
    }

    #[test]
    fn a_disjunction_of_denies_denies() {
        assert_eq!(
            SecurityFilter::any(vec![
                SecurityFilter::Unsatisfiable,
                SecurityFilter::Unsatisfiable
            ]),
            SecurityFilter::Unsatisfiable
        );
    }

    #[test]
    fn a_denied_branch_drops_out_of_a_disjunction() {
        assert_eq!(
            SecurityFilter::any(vec![
                SecurityFilter::Unsatisfiable,
                restricted("dept", json!("eng")),
            ]),
            restricted("dept", json!("eng"))
        );
    }

    // --- S2: the NULL leak ---

    #[test]
    fn negation_excludes_a_row_missing_the_compared_field() {
        // The round-2 break: `Not(Eq(classification,"TS"))` over a record with no
        // `classification` at all evaluated `!false` = admit, leaking exactly the
        // records a classification policy exists to withhold.
        let negated = restricted("classification", json!("TS")).negate();

        assert!(
            !admits_filter(&negated, &json!({ "other": 1 })),
            "a record with no classification must not be admitted by a negated classification filter"
        );
        assert!(
            !admits_filter(&negated, &json!({ "classification": null })),
            "an explicit null classification must not be admitted either"
        );
        // The rows it should admit still come through.
        assert!(admits_filter(
            &negated,
            &json!({ "classification": "PUBLIC" })
        ));
        assert!(!admits_filter(&negated, &json!({ "classification": "TS" })));
    }

    #[test]
    fn negation_guards_every_field_in_a_compound_expression() {
        let compound = SecurityFilter::Restricted(FilterExpression::And(vec![
            eq_expr("dept", json!("eng")),
            eq_expr("region", json!("eu")),
        ]));
        let negated = compound.negate();

        // Missing either field ⇒ excluded, not admitted.
        assert!(!admits_filter(&negated, &json!({ "dept": "eng" })));
        assert!(!admits_filter(&negated, &json!({ "region": "eu" })));
        assert!(!admits_filter(&negated, &json!({})));
        // Both present and not both matching ⇒ admitted.
        assert!(admits_filter(
            &negated,
            &json!({ "dept": "hr", "region": "eu" })
        ));
    }

    #[test]
    fn a_negated_null_test_is_not_guarded_into_a_contradiction() {
        // `Not(IsNotNull(f))` is correctly `f IS NULL`; guarding it with
        // IsNotNull(f) would make it unsatisfiable. Null tests are already
        // absence-aware, so they are excluded from the guard set.
        let is_not_null = SecurityFilter::Restricted(FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::IsNotNull,
            value: serde_json::Value::Null,
        });
        let negated = is_not_null.negate();
        assert!(admits_filter(&negated, &json!({})));
        assert!(!admits_filter(&negated, &json!({ "dept": "eng" })));
    }

    #[test]
    fn a_bare_not_equals_excludes_a_row_missing_the_field() {
        // `Not` is not the only negation: `dept != "eng"` admitted every record
        // with no `dept` at all, because the evaluator reads absent-or-null as
        // "not equal to anything" (TF-2 §1.4's NotEquals deny→admit).
        let ne = SecurityFilter::comparison(
            "dept".to_string(),
            ComparisonOperator::NotEquals,
            json!("eng"),
        );
        assert!(!admits_filter(&ne, &json!({})));
        assert!(!admits_filter(&ne, &json!({ "dept": null })));
        assert!(admits_filter(&ne, &json!({ "dept": "hr" })));
        assert!(!admits_filter(&ne, &json!({ "dept": "eng" })));
    }

    #[test]
    fn a_bare_not_in_excludes_a_row_missing_the_field() {
        let ni = SecurityFilter::comparison(
            "dept".to_string(),
            ComparisonOperator::NotIn,
            json!(["eng", "hr"]),
        );
        assert!(!admits_filter(&ni, &json!({})));
        assert!(admits_filter(&ni, &json!({ "dept": "legal" })));
    }

    #[test]
    fn a_positive_operator_is_not_guarded() {
        // A positive comparison already denies a missing field; adding a guard
        // would be redundant structure on every scan.
        let eq = SecurityFilter::comparison(
            "dept".to_string(),
            ComparisonOperator::Equals,
            json!("eng"),
        );
        assert_eq!(
            eq,
            SecurityFilter::Restricted(eq_expr("dept", json!("eng")))
        );
    }

    #[test]
    fn value_compared_fields_skips_null_tests() {
        let expr = FilterExpression::And(vec![
            eq_expr("a", json!(1)),
            FilterExpression::Comparison {
                field: "b".to_string(),
                operator: ComparisonOperator::IsNull,
                value: serde_json::Value::Null,
            },
            FilterExpression::Not(Box::new(eq_expr("c", json!(2)))),
        ]);
        let fields = value_compared_fields(&expr);
        assert!(fields.contains("a"));
        assert!(fields.contains("c"));
        assert!(!fields.contains("b"));
    }

    // --- the deny-biased subset property (S1–S3 joint) ---

    /// Reference semantics for the *intended* meaning of a lowered expression,
    /// evaluated three-valued: a value comparison over an absent or null field is
    /// UNKNOWN, `Not(UNKNOWN)` is UNKNOWN, and only TRUE admits.
    ///
    /// The property under test is that the lowering never admits a row this
    /// reference denies — i.e. the bridge may only ever *tighten*.
    fn reference_admits(e: &FilterExpression, row: &serde_json::Value) -> Option<bool> {
        match e {
            FilterExpression::And(parts) => {
                let mut seen_unknown = false;
                for p in parts {
                    match reference_admits(p, row) {
                        Some(false) => return Some(false),
                        None => seen_unknown = true,
                        Some(true) => {}
                    }
                }
                if seen_unknown { None } else { Some(true) }
            }
            FilterExpression::Or(parts) => {
                let mut seen_unknown = false;
                for p in parts {
                    match reference_admits(p, row) {
                        Some(true) => return Some(true),
                        None => seen_unknown = true,
                        Some(false) => {}
                    }
                }
                if seen_unknown { None } else { Some(false) }
            }
            FilterExpression::Not(inner) => reference_admits(inner, row).map(|v| !v),
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                let present = row.get(field).filter(|v| !v.is_null());
                match operator {
                    ComparisonOperator::IsNull => Some(present.is_none()),
                    ComparisonOperator::IsNotNull => Some(present.is_some()),
                    ComparisonOperator::Equals => present.map(|v| v == value),
                    ComparisonOperator::NotEquals => present.map(|v| v != value),
                    // Other operators are not generated by this property's grammar.
                    _ => None,
                }
            }
        }
    }

    /// Exhaustive rather than random: the space below is small enough to cover
    /// completely, which makes the property deterministic in CI (no seed to
    /// reproduce, no flake) while still covering the shapes that broke.
    fn predicate_grammar() -> Vec<SecurityFilter> {
        let leaves = vec![
            SecurityFilter::Unrestricted,
            SecurityFilter::Unsatisfiable,
            restricted("dept", json!("eng")),
            restricted("dept", json!("hr")),
            restricted("region", json!("eu")),
            SecurityFilter::comparison(
                "dept".to_string(),
                ComparisonOperator::NotEquals,
                json!("eng"),
            ),
        ];

        let mut all = leaves.clone();
        for a in &leaves {
            all.push(a.clone().negate());
            for b in &leaves {
                all.push(SecurityFilter::all(vec![a.clone(), b.clone()]));
                all.push(SecurityFilter::any(vec![a.clone(), b.clone()]));
                all.push(SecurityFilter::all(vec![a.clone(), b.clone()]).negate());
                all.push(SecurityFilter::any(vec![a.clone(), b.clone()]).negate());
            }
        }
        all
    }

    fn row_space() -> Vec<serde_json::Value> {
        vec![
            json!({}),
            json!({ "dept": "eng" }),
            json!({ "dept": "hr" }),
            json!({ "region": "eu" }),
            json!({ "dept": "eng", "region": "eu" }),
            json!({ "dept": "hr", "region": "us" }),
            json!({ "dept": null }),
            json!({ "dept": null, "region": "eu" }),
        ]
    }

    #[test]
    fn the_lowering_is_deny_biased_over_the_whole_grammar() {
        // For every predicate in the grammar × every row: if the lowered filter
        // admits the row, the three-valued reference must admit it too. The
        // bridge may tighten; it may never widen.
        let rows = row_space();
        let mut checked = 0usize;
        for f in predicate_grammar() {
            let expr = f.clone().into_expression();
            for row in &rows {
                let lowered_admits = match &expr {
                    None => true,
                    Some(e) => admits(e, row),
                };
                if !lowered_admits {
                    continue;
                }
                if let Some(e) = &expr {
                    assert_eq!(
                        reference_admits(e, row),
                        Some(true),
                        "lowered filter admitted a row the 3-valued reference does not: \
                         filter={f:?} row={row}"
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 0, "the property covered no admitting case");
    }

    #[test]
    fn every_unsatisfiable_in_the_grammar_admits_nothing() {
        // The other half of the property: a deny-derived node must yield zero
        // rows, not a dropped conjunct that reads the full table.
        let rows = row_space();
        for f in predicate_grammar() {
            if !f.is_unsatisfiable() {
                continue;
            }
            let expr = f
                .clone()
                .into_expression()
                .expect("unsatisfiable must lower to a real expression, never None");
            for row in &rows {
                assert!(!admits(&expr, row), "unsatisfiable admitted {row}");
            }
        }
    }
}
