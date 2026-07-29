// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! The ABAC-to-scan-predicate adapter: turns compiled `ObjectId` refs into a
//! per-row `Fn(&ProximaRecord) -> bool` that `scan_records_filtered` ANDs into
//! its existing `RecordScanPredicate`.
//!
//! This is the **scan-path integration point** (FA-c Phase 2b/2c). It compiles
//! the `AuthorizedReadContext`'s `row_predicate_refs` once (at request-scope),
//! then the returned closure evaluates each record under the strict 3-valued
//! walker via [`admits_with_security`](super::filter_lattice::admits_with_security).

#[cfg(feature = "abac-policy")]
use crate::core::search::{FilterExpression, sql_value_filter::proxima_value_to_json};
#[cfg(feature = "abac-policy")]
use crate::security::rls::filter_lattice::admits_with_security;
#[cfg(feature = "abac-policy")]
use proximadb_abac::{
    AttributeAuthority, AuthorizedReadContext, DenyReason, PolicyEpochSource, PredicateObjectStore,
    compile_security_filter,
};
#[cfg(feature = "abac-policy")]
use proximadb_catalog::fc_metamodel::{ObjectId, PolicyBinding, SubjectId, Target};
#[cfg(feature = "abac-policy")]
use proximadb_records::{ProximaRecord, ProximaTreeNode};

/// Compile `refs` into a per-row predicate for `scan_records_filtered`.
///
/// Returns `None` when the refs are empty (no row restriction) or when the
/// compiled expression is `None`. Returns `Some(closure)` that evaluates each
/// `ProximaRecord` under the strict security walker. A missing predicate ref
/// (fail-closed in `compile_security_filter`) yields an unsatisfiable closure
/// that denies every row.
///
/// The closure owns the compiled `FilterExpression`; it is `Send + Sync` and
/// can be passed as a `RecordScanPredicate` to `scan_records_filtered`.
/// FA-c Phase 2c wires this into `scan_records_filtered`'s predicate param; until
/// then it has no production caller (the feature is default-OFF and the function is
/// the integration point the wiring step consumes).
#[cfg(feature = "abac-policy")]
#[allow(dead_code)]
pub fn abac_scan_predicate(
    refs: &[ObjectId],
    store: &dyn PredicateObjectStore,
) -> Option<Box<dyn Fn(&ProximaRecord) -> bool + Send + Sync>> {
    let security = compile_security_filter(refs, store)?;

    Some(Box::new(move |record: &ProximaRecord| {
        // Resolve each field the security expression references from the record's
        // props. Using `proxima_value_to_json` (not the whole-tree map) avoids a
        // per-row HashMap allocation — only the fields the expression actually
        // touches are extracted, lazily.
        let resolve = |field: &str| -> Option<serde_json::Value> {
            record.props.get(field).and_then(|node| match node {
                ProximaTreeNode::Value(pv) => Some(proxima_value_to_json(pv)),
                _ => None,
            })
        };
        admits_with_security(None, Some(&security), &resolve)
    }))
}

/// The outcome of an ABAC resolution for a scan: how to handle the rows.
#[cfg(feature = "abac-policy")]
#[allow(dead_code)]
pub enum AbacScanResult {
    /// No row restriction (the subject is permitted with no row predicates).
    Unrestricted,
    /// A per-row predicate that `scan_records_filtered` ANDs into its hook.
    Restricted(Box<dyn Fn(&ProximaRecord) -> bool + Send + Sync>),
    /// The subject is denied entirely — return zero rows.
    Denied(DenyReason),
}

/// The service-facing ABAC enforcement API. Holds the three substrate stores
/// and provides one method a scan call site calls: resolve the subject's
/// authorization, compile it, and return a scan predicate (or a deny).
///
/// FA-c Phase 2c constructs this at the scan boundary; Phase 4 (FA-b) threads
/// it (or its `AuthorizedReadContext`) as a required parameter.
#[cfg(feature = "abac-policy")]
#[allow(dead_code)]
pub struct AbacEnforcer {
    authority: Box<dyn AttributeAuthority>,
    store: Box<dyn PredicateObjectStore>,
    epochs: Box<dyn PolicyEpochSource>,
}

#[cfg(feature = "abac-policy")]
impl AbacEnforcer {
    /// Construct from the three substrate stores. In production these are the
    /// durable-backed impls; in tests, the in-memory ones.
    pub fn new(
        authority: Box<dyn AttributeAuthority>,
        store: Box<dyn PredicateObjectStore>,
        epochs: Box<dyn PolicyEpochSource>,
    ) -> Self {
        Self {
            authority,
            store,
            epochs,
        }
    }

    /// Resolve `subject`'s authorization for `target` and compile to a scan
    /// predicate. The caller provides the `bindings` (loaded from the policy
    /// store — Phase 5 makes that durable; today they are supplied directly).
    ///
    /// This is the one call a scan call site makes: it ties resolve → compile
    /// → `abac_scan_predicate` into a single `AbacScanResult`.
    pub fn scan_predicate_for(
        &self,
        subject: &SubjectId,
        tenant: u64,
        target: Target,
        bindings: &[PolicyBinding],
    ) -> AbacScanResult {
        match AuthorizedReadContext::resolve(
            self.authority.as_ref(),
            self.epochs.as_ref(),
            bindings,
            subject,
            tenant,
            target,
        ) {
            proximadb_abac::ReadDecision::Deny(reason) => AbacScanResult::Denied(reason),
            proximadb_abac::ReadDecision::Admit(ctx) => {
                match abac_scan_predicate(ctx.row_predicate_refs(), self.store.as_ref()) {
                    Some(predicate) => AbacScanResult::Restricted(predicate),
                    None => AbacScanResult::Unrestricted,
                }
            }
        }
    }

    /// Resolve `subject`'s authorization and return the compiled security
    /// `FilterExpression` directly — for read paths that take a `FilterExpression`
    /// natively (e.g. `unified_search_native`'s `filter` param), rather than a
    /// per-record closure.
    ///
    /// Returns `Ok(None)` when the subject is permitted with no row predicates;
    /// `Ok(Some(expr))` when a security filter applies; `Err(reason)` when denied.
    /// The caller ANDs `expr` with the user's own filter before the search.
    pub fn security_expression_for(
        &self,
        subject: &SubjectId,
        tenant: u64,
        target: Target,
        bindings: &[PolicyBinding],
    ) -> Result<Option<FilterExpression>, DenyReason> {
        match AuthorizedReadContext::resolve(
            self.authority.as_ref(),
            self.epochs.as_ref(),
            bindings,
            subject,
            tenant,
            target,
        ) {
            proximadb_abac::ReadDecision::Deny(reason) => Err(reason),
            proximadb_abac::ReadDecision::Admit(ctx) => Ok(compile_security_filter(
                ctx.row_predicate_refs(),
                self.store.as_ref(),
            )),
        }
    }
}

#[cfg(all(test, feature = "abac-policy"))]
mod tests {
    use super::*;
    use crate::core::search::{ComparisonOperator, FilterExpression};
    use proximadb_abac::{
        InMemoryAttributeAuthority, InMemoryPolicyEpochs, InMemoryPredicateObjectStore,
    };
    use proximadb_catalog::fc_metamodel::{AttrValue, Effect, Scope};
    use proximadb_data_model::ProximaValue;
    use proximadb_records::{ProximaRecord, ProximaTreeNode};
    use serde_json::json;

    fn record_with(dept: &str) -> ProximaRecord {
        let mut rec = ProximaRecord::default();
        rec.props.insert(
            "dept".to_string(),
            ProximaTreeNode::Value(ProximaValue::String(dept.to_string())),
        );
        rec
    }

    #[test]
    fn predicate_filters_rows_by_dept() {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(
            42,
            FilterExpression::Comparison {
                field: "dept".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("eng"),
            },
        );

        let predicate = abac_scan_predicate(&[42], &store).expect("refs non-empty → predicate");

        assert!(predicate(&record_with("eng")), "dept=eng must be admitted");
        assert!(!predicate(&record_with("hr")), "dept=hr must be denied");
    }

    #[test]
    fn empty_refs_produce_no_predicate() {
        let store = InMemoryPredicateObjectStore::new();
        assert!(abac_scan_predicate(&[], &store).is_none());
    }

    #[test]
    fn missing_ref_denies_every_row() {
        let store = InMemoryPredicateObjectStore::new();
        let predicate =
            abac_scan_predicate(&[999], &store).expect("non-empty refs → unsatisfiable predicate");
        assert!(!predicate(&record_with("eng")), "missing ref denies all");
    }

    // --- AbacEnforcer: the service-facing one-call API ---

    fn enforcer_with_alice(dept: &str) -> (AbacEnforcer, Vec<PolicyBinding>) {
        let mut authority = InMemoryAttributeAuthority::new();
        authority.upsert(
            proximadb_abac::AttributeBinding::new("alice", 7)
                .with_attr("dept", AttrValue::Str(dept.into())),
        );
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(
            42,
            FilterExpression::Comparison {
                field: "dept".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!(dept),
            },
        );
        let bindings = vec![PolicyBinding {
            object_id: 1,
            tenant_stable_id: 7,
            scope: Scope::Table(200),
            effect: Effect::Permit,
            predicate_ref: Some(42),
            field_mask: None,
        }];
        let enforcer = AbacEnforcer::new(
            Box::new(authority),
            Box::new(store),
            Box::new(InMemoryPolicyEpochs::new()),
        );
        (enforcer, bindings)
    }

    #[test]
    fn enforcer_returns_restricted_predicate_for_dept_eng() {
        let (enforcer, bindings) = enforcer_with_alice("eng");
        let result = enforcer.scan_predicate_for(
            &SubjectId("alice".into()),
            7,
            Target {
                namespace: 3,
                table: 200,
                column: None,
            },
            &bindings,
        );
        match result {
            AbacScanResult::Restricted(pred) => {
                assert!(pred(&record_with("eng")));
                assert!(!pred(&record_with("hr")));
            }
            _ => panic!("alice (dept=eng, binding present) must be Restricted"),
        }
    }

    #[test]
    fn enforcer_denies_an_unbound_subject() {
        let (enforcer, bindings) = enforcer_with_alice("eng");
        let result = enforcer.scan_predicate_for(
            &SubjectId("mallory".into()),
            7,
            Target {
                namespace: 3,
                table: 200,
                column: None,
            },
            &bindings,
        );
        assert!(matches!(result, AbacScanResult::Denied(_)));
    }

    #[test]
    fn enforcer_returns_security_expression_for_vector_path() {
        // The vector search path (unified_search_native) takes a FilterExpression,
        // not a closure. security_expression_for returns it directly.
        let (enforcer, bindings) = enforcer_with_alice("eng");
        let result = enforcer
            .security_expression_for(
                &SubjectId("alice".into()),
                7,
                Target {
                    namespace: 3,
                    table: 200,
                    column: None,
                },
                &bindings,
            )
            .expect("alice is admitted");

        let expr = result.expect("alice has a predicate ref → Some(expr)");
        // The expression should be the dept=eng filter.
        match expr {
            FilterExpression::Comparison {
                field,
                operator: ComparisonOperator::Equals,
                value,
            } => {
                assert_eq!(field, "dept");
                assert_eq!(value, json!("eng"));
            }
            _ => panic!("expected a single Eq(dept, eng) comparison"),
        }
    }

    #[test]
    fn enforcer_expression_denies_unbound_subject() {
        let (enforcer, bindings) = enforcer_with_alice("eng");
        let result = enforcer.security_expression_for(
            &SubjectId("mallory".into()),
            7,
            Target {
                namespace: 3,
                table: 200,
                column: None,
            },
            &bindings,
        );
        assert!(result.is_err(), "unbound subject must be denied");
    }
}
