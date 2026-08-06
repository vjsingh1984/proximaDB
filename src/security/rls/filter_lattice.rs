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

use crate::core::search::{
    ComparisonOperator, FilterExpression,
    sql_value_filter::{evaluate_filter_resolved, evaluate_filter_resolved_type_strict},
};

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
    /// to its plain complement. No synthetic null-guards: the security expression
    /// is walked by the 3-valued strict walker, where an absent field is UNKNOWN
    /// and `Not(UNKNOWN)=UNKNOWN→deny` — so the guard the 2-valued walker needed
    /// is now redundant (and was the source of the `Not(And)` over-deny).
    pub fn negate(self) -> SecurityFilter {
        match self {
            SecurityFilter::Unrestricted => SecurityFilter::Unsatisfiable,
            SecurityFilter::Unsatisfiable => SecurityFilter::Unrestricted,
            SecurityFilter::Restricted(e) => {
                SecurityFilter::Restricted(FilterExpression::Not(Box::new(e)))
            }
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

/// Admit a row under **separate evaluation**: the user's filter is walked by the
/// permissive (2-valued) walker, the security predicate by the **strict**
/// (3-valued) walker, and a row is admitted only if BOTH admit. This is the
/// evaluation model TF-2 §3.4 prescribes and the one the lattice's lowering now
/// assumes — it is why the null-guards could be dropped.
///
/// `AND`-associativity makes this exactly equivalent to a single conjunction for
/// any row the *security* predicate admits, and strictly tighter for rows it
/// denies (the security walker's UNKNOWN ⇒ deny). The user filter's permissive
/// semantics are untouched.
///
/// FA-c wires this at the read primitive: it compiles `AuthorizedReadContext`'s
/// `row_predicate_refs` into the `security` expression and supplies the user's
/// query filter, then admits each row via this function. The legacy
/// `services::operations::combine_filters` *merges* the two trees into one
/// permissively-walked `And` — the merge that originally forced the null-guards;
/// FA-c supersedes it.
pub fn admits_with_security<F>(
    user: Option<&FilterExpression>,
    security: Option<&FilterExpression>,
    resolve: &F,
) -> bool
where
    F: Fn(&str) -> Option<serde_json::Value>,
{
    let user_admits = user.is_none_or(|e| evaluate_filter_resolved(e, resolve));
    let security_admits = security.is_none_or(|e| evaluate_filter_resolved_type_strict(e, resolve));
    user_admits && security_admits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::sql_value_filter::evaluate_filter_resolved_type_strict_tri;
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

    /// Admit/deny under the SECURITY walker (3-valued, UNKNOWN⇒deny). Every test in
    /// this module is a security-filter test, so this — not the permissive walker —
    /// is the correct evaluator for the lowering.
    fn admits(expr: &FilterExpression, row: &serde_json::Value) -> bool {
        strict_tri(expr, row).unwrap_or(false)
    }

    /// The security walker's tri-valued result (None = UNKNOWN → deny).
    fn strict_tri(expr: &FilterExpression, row: &serde_json::Value) -> Option<bool> {
        evaluate_filter_resolved_type_strict_tri(expr, &|field| row.get(field).cloned())
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

    // --- the deny-biased subset property (S1–S3 joint) ---

    /// Reference semantics for the *intended* meaning of a lowered expression,
    /// evaluated three-valued: a value comparison over an absent or null field is
    /// UNKNOWN, `Not(UNKNOWN)` is UNKNOWN, and only TRUE admits.
    ///
    /// The property under test is that the lowering never admits a row this
    /// reference denies — i.e. the bridge may only ever *tighten*.
    /// Independent 3-valued (SQL/Kleene) oracle — the SPEC the walker is tested
    /// against. Independent of the production comparator on the axes that broke
    /// (Not propagation, absence): absence is UNKNOWN for value comparisons, and
    /// `Not(UNKNOWN)=UNKNOWN`. The equality/null leaves are implemented directly;
    /// the ordered/string/list leaves reuse the (separately, exhaustively tested)
    /// `compare_json_op_type_strict` for their within-class semantics, so this
    /// oracle does not duplicate that grid.
    fn reference_admits(e: &FilterExpression, row: &serde_json::Value) -> Option<bool> {
        use crate::core::search::sql_value_filter::compare_json_op_type_strict;
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
            // The fix under test: Not(UNKNOWN) = UNKNOWN, not `!false` = admit.
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
                    // Absence is UNKNOWN, and a present value is delegated to the
                    // type-strict comparator (cross-class ⇒ None for EVERY
                    // operator, including Equals — a structural `==` would wrongly
                    // call a cross-class pair "unequal" and let `Not` admit it).
                    // The leaf is exhaustively tested elsewhere (the 7×7×operator
                    // grid); this oracle's independence is on the tree combinators
                    // and the absence axis.
                    _ => match present {
                        None => None,
                        Some(fv) => compare_json_op_type_strict(operator, fv, value),
                    },
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
        // Spans presence/absence, null, and the cross-class pairs that exposed
        // the Not(comparison) fail-open (a string where a number was expected,
        // and vice versa).
        vec![
            json!({}),
            json!({ "dept": "eng" }),
            json!({ "dept": "hr" }),
            json!({ "region": "eu" }),
            json!({ "dept": "eng", "region": "eu" }),
            json!({ "dept": "hr", "region": "us" }),
            json!({ "dept": null }),
            json!({ "dept": null, "region": "eu" }),
            json!({ "level": 3 }),
            json!({ "level": 5 }),
            json!({ "level": "TS" }), // cross-class: string where number expected
            json!({ "level": null }),
            json!({ "flag": true }),
            json!({ "dept": "eng", "level": 5 }),
            json!({ "dept": "eng", "level": "TS" }),
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
    // ---- P3: separate evaluation (security strict ∧ user permissive) ----

    #[test]
    fn separate_evaluation_denies_a_row_the_security_predicate_denies() {
        // The user filter would admit (permissive Gt on a cross-class pair), but
        // the security predicate denies it strictly. The AND must deny.
        let resolve = |f: &str| match f {
            "level" => Some(json!("TS")),
            _ => None,
        };
        let user = Some(&FilterExpression::Comparison {
            field: "level".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(3),
        });
        let security = user; // same shape; permissive admits, strict denies
        assert!(
            evaluate_filter_resolved(user.unwrap(), &resolve),
            "precondition: the permissive walker admits the cross-class row"
        );
        assert!(
            !evaluate_filter_resolved_type_strict(security.unwrap(), &resolve),
            "the strict walker denies it"
        );
        assert!(
            !admits_with_security(user, security, &resolve),
            "separate evaluation must deny: security's strict deny wins"
        );
    }

    #[test]
    fn separate_evaluation_admits_when_both_admit() {
        let resolve = |f: &str| match f {
            "dept" => Some(json!("eng")),
            _ => None,
        };
        let user = Some(&FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("eng"),
        });
        assert!(admits_with_security(user, user, &resolve));
    }

    #[test]
    fn no_security_predicate_means_user_filter_alone() {
        let resolve = |f: &str| match f {
            "dept" => Some(json!("eng")),
            _ => None,
        };
        let user = Some(&FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("eng"),
        });
        assert!(admits_with_security(user, None, &resolve));
        // And no filters at all admits (the scan's own predicate is the gate).
        assert!(admits_with_security(None, None, &resolve));
    }

    // ---- P2: the behaviors the guard-drop FIXES (red-team Findings 2 & 4) ----

    #[test]
    fn negated_conjunction_admits_a_row_matching_one_arm() {
        // Finding 4: the old null-guard conjoined IsNotNull for EVERY field, so
        // Not(And([Eq(owner), Eq(tenant)])) denied a row missing `tenant` even
        // though the owner arm is already false. With guards gone + strict 3VL,
        // Kleene AND(false, UNKNOWN) = false, Not = admit.
        let policy = SecurityFilter::all(vec![
            restricted("owner", json!("alice")),
            restricted("tenant", json!("t1")),
        ])
        .negate();
        assert!(
            admits_filter(&policy, &json!({ "owner": "bob" })),
            "a row matching one arm of a negated conjunction must be admitted"
        );
        assert!(!admits_filter(
            &policy,
            &json!({ "owner": "alice", "tenant": "t1" })
        ));
        assert!(admits_filter(
            &policy,
            &json!({ "owner": "carol", "tenant": "t2" })
        ));
    }

    #[test]
    fn double_negation_is_the_identity() {
        // The case the De-Morgan attempt broke: Not(Not(Eq)) must equal Eq,
        // including over an absent field (UNKNOWN, not admit).
        let p = restricted("dept", json!("eng"));
        let dd = p.clone().negate().negate();
        for row in [json!({"dept": "eng"}), json!({"dept": "hr"}), json!({})] {
            assert_eq!(
                admits_filter(&p, &row),
                admits_filter(&dd, &row),
                "double negation must be the identity: {row}"
            );
        }
    }

    #[test]
    fn negated_equals_denies_a_cross_class_row() {
        // Finding 2: Not(Equals(owner, "u/alice")) must deny {owner: 42} — a
        // cross-class pair is UNKNOWN, Not(UNKNOWN) = UNKNOWN → deny.
        let policy = restricted("owner", json!("u/alice")).negate();
        assert!(!admits_filter(&policy, &json!({ "owner": 42 })));
        assert!(admits_filter(&policy, &json!({ "owner": "bob" })));
        assert!(!admits_filter(&policy, &json!({ "owner": "u/alice" })));
    }

    /// A `FilterExpression` grammar spanning every operator and the And/Or/Not
    /// compositions, so the walker is exercised over the whole surface, not just
    /// the equality corner the old grammar covered.
    fn filter_grammar() -> Vec<FilterExpression> {
        use ComparisonOperator::*;
        let mut ops = vec![
            Equals,
            NotEquals,
            LessThan,
            LessThanOrEqual,
            GreaterThan,
            GreaterThanOrEqual,
            In,
            NotIn,
            Between,
            Contains,
            StartsWith,
            EndsWith,
            Like,
            IsNull,
            IsNotNull,
        ];
        let leaves: Vec<FilterExpression> = ops
            .drain(..)
            .flat_map(|op| {
                let lit_for = |op: ComparisonOperator| -> serde_json::Value {
                    match op {
                        In | NotIn => json!(["eng", "hr"]),
                        Between => json!(["a", "z"]),
                        _ => json!("eng"),
                    }
                };
                let on_level = FilterExpression::Comparison {
                    field: "level".to_string(),
                    operator: op.clone(),
                    value: match op {
                        In | NotIn => json!([3, 5]),
                        Between => json!([1, 10]),
                        _ => json!(3),
                    },
                };
                vec![
                    FilterExpression::Comparison {
                        field: "dept".to_string(),
                        operator: op.clone(),
                        value: lit_for(op),
                    },
                    on_level,
                ]
            })
            .collect();

        let mut all = leaves.clone();
        for a in &leaves {
            all.push(FilterExpression::Not(Box::new(a.clone())));
            all.push(FilterExpression::Not(Box::new(FilterExpression::Not(
                Box::new(a.clone()),
            ))));
            for b in &leaves {
                all.push(FilterExpression::And(vec![a.clone(), b.clone()]));
                all.push(FilterExpression::Or(vec![a.clone(), b.clone()]));
                all.push(FilterExpression::Not(Box::new(FilterExpression::And(
                    vec![a.clone(), b.clone()],
                ))));
                all.push(FilterExpression::Not(Box::new(FilterExpression::Or(vec![
                    a.clone(),
                    b.clone(),
                ]))));
            }
        }
        all
    }

    /// P1 — the security walker MUST equal the independent 3-valued reference over
    /// the whole grammar × row space. Equivalence (not a one-sided subset): it
    /// catches fail-open (walker admits, reference denies) AND over-deny (walker
    /// denies, reference admits). This is the property the narrow old grammar could
    /// not catch.
    #[test]
    fn strict_walker_equals_the_three_valued_reference_over_the_whole_grammar() {
        for expr in filter_grammar() {
            for row in row_space() {
                assert_eq!(
                    strict_tri(&expr, &row),
                    reference_admits(&expr, &row),
                    "walker ≠ 3-valued reference:
  expr = {expr:?}
  row  = {row}"
                );
            }
        }
    }
}
