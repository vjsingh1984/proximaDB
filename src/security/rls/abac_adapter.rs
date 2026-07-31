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
use crate::core::search::{
    FilterExpression, OptimizedSearchRecord, sql_value_filter::proxima_value_to_json,
};
#[cfg(feature = "abac-policy")]
use crate::security::rls::filter_lattice::admits_with_security;
#[cfg(feature = "abac-policy")]
use proximadb_abac::{
    AttributeAuthority, AuthorizedReadContext, DenyReason, PolicyBindingStore, PolicyEpochSource,
    PredicateObjectStore, compile_security_filter,
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
    authority: Box<dyn AttributeAuthority + Send + Sync>,
    store: Box<dyn PredicateObjectStore + Send + Sync>,
    epochs: Box<dyn PolicyEpochSource + Send + Sync>,
    /// The policy bindings this enforcer governs. Holding them makes the enforcer
    /// a self-contained policy a service can store once and call per-read
    /// (`predicate_for`); the per-call `scan_predicate_for(.., bindings)` variant
    /// remains for substrate unit tests.
    bindings: Vec<PolicyBinding>,
    /// The durable policy source (Phase 5b). When set, `predicate_for` loads the
    /// tenant's bindings from here per read — restart-surviving policy. When
    /// unset, it falls back to the held `bindings` (the in-memory/test path).
    binding_store: Option<Box<dyn PolicyBindingStore + Send + Sync>>,
}

#[cfg(feature = "abac-policy")]
#[allow(dead_code)]
impl AbacEnforcer {
    /// Construct from the three substrate stores. In production these are the
    /// durable-backed impls; in tests, the in-memory ones.
    pub fn new(
        authority: Box<dyn AttributeAuthority + Send + Sync>,
        store: Box<dyn PredicateObjectStore + Send + Sync>,
        epochs: Box<dyn PolicyEpochSource + Send + Sync>,
    ) -> Self {
        Self {
            authority,
            store,
            epochs,
            bindings: Vec::new(),
            binding_store: None,
        }
    }

    /// Install the policy bindings this enforcer governs (builder). After this,
    /// [`predicate_for`](Self::predicate_for) resolves against the held bindings
    /// — the one call a read-serving service makes per scan.
    pub fn with_bindings(mut self, bindings: Vec<PolicyBinding>) -> Self {
        self.bindings = bindings;
        self
    }

    /// Install the durable policy source (builder, Phase 5b). When set,
    /// [`predicate_for`](Self::predicate_for) loads the tenant's bindings from the
    /// store on every read — a restart-surviving policy. This is the production
    /// path; [`with_bindings`](Self::with_bindings) remains for tests.
    pub fn with_binding_store(mut self, store: Box<dyn PolicyBindingStore + Send + Sync>) -> Self {
        self.binding_store = Some(store);
        self
    }

    /// Resolve `subject`'s authorization for `target` and compile to a scan
    /// predicate. When a durable [`PolicyBindingStore`] is installed, the
    /// tenant's bindings are loaded from it per read (the production path);
    /// otherwise the enforcer's held `bindings` are used (the in-memory/test
    /// path). This is the one call a read-serving service makes per scan.
    pub fn predicate_for(
        &self,
        subject: &SubjectId,
        tenant: u64,
        target: Target,
    ) -> AbacScanResult {
        let owned;
        let bindings: &[PolicyBinding] = match &self.binding_store {
            Some(store) => {
                owned = store.bindings_for(tenant);
                &owned
            }
            None => &self.bindings,
        };
        self.scan_predicate_for(subject, tenant, target, bindings)
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

    /// Compile an **already-admitted** [`AuthorizedReadContext`]'s row predicates
    /// into a security `FilterExpression` (no re-resolution). For the vector
    /// push-down path: the caller (a network handler) resolves the subject →
    /// [`AuthorizedReadContext::resolve`], handles `Deny` fail-closed (→ empty
    /// results) THERE, and passes the admitted `Client` context here. Returns
    /// `Option`, not `Result`: deny is impossible at this stage because the
    /// context is already admitted — the caller MUST NOT collapse a `DenyReason`
    /// into `None` (that would be a fail-open hole). `None` = admitted with no
    /// row predicate.
    pub fn security_filter_for_context(
        &self,
        ctx: &AuthorizedReadContext,
    ) -> Option<FilterExpression> {
        compile_security_filter(ctx.row_predicate_refs(), self.store.as_ref())
    }

    /// Resolve `subject`'s authorization and **post-filter** vector search
    /// results. The ANN search runs first (over its own metadata filter), then
    /// this removes inadmissible results via the strict 3-valued walker.
    ///
    /// This is the **vector-path integration** (FA-c Phase 3, Option A): the
    /// search kernel is untouched; the security filter is a post-processing step
    /// on the results. Less efficient than a pre-filter (the search evaluates
    /// more rows than returned) but correct — the permissive search filter and
    /// the strict security filter are evaluated independently and ANDed, so the
    /// fail-open the `combine_filters` merge caused is structurally avoided.
    ///
    /// Returns `Ok(filtered)` on success, `Err(reason)` if the subject is denied
    /// entirely.
    #[allow(dead_code)]
    pub fn filter_search_results(
        &self,
        results: Vec<OptimizedSearchRecord>,
        subject: &SubjectId,
        tenant: u64,
        target: Target,
        bindings: &[PolicyBinding],
    ) -> Result<Vec<OptimizedSearchRecord>, DenyReason> {
        let security = self.security_expression_for(subject, tenant, target, bindings)?;
        Ok(match security {
            None => results,
            Some(expr) => post_filter_search_results(results, &expr),
        })
    }
}

/// Post-filter vector search results by a compiled security `FilterExpression`.
///
/// Each result's `metadata` is resolved against the expression under the strict
/// 3-valued walker. Inadmissible results are removed. This is the vector-path
/// analogue of `abac_scan_predicate` (relational): both evaluate
/// `admits_with_security` per record, but this operates on the *output* of the
/// search rather than as a *predicate* during the scan.
#[cfg(feature = "abac-policy")]
#[allow(dead_code)]
pub fn post_filter_search_results(
    results: Vec<OptimizedSearchRecord>,
    security: &FilterExpression,
) -> Vec<OptimizedSearchRecord> {
    results
        .into_iter()
        .filter(|record| {
            let resolve = |field: &str| -> Option<serde_json::Value> {
                record.metadata.get(field).map(proxima_value_to_json)
            };
            admits_with_security(None, Some(security), &resolve)
        })
        .collect()
}

#[cfg(all(test, feature = "abac-policy"))]
mod tests {
    use super::*;
    use crate::core::search::{ComparisonOperator, FilterExpression};
    use proximadb_abac::{
        FileSystemPolicyBindingStore, InMemoryAttributeAuthority, InMemoryPolicyEpochs,
        InMemoryPredicateObjectStore,
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

    // --- Phase 5b: durable policy-binding store (the enforcer loads from it) ---

    /// Shared store/authority/predicate-store config for the durable-policy tests.
    /// `dept=eng` is the only admitted dept; tenant 7's policy permits table 200
    /// under predicate ref 42.
    fn predicate_store_for_dept() -> InMemoryPredicateObjectStore {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(
            42,
            FilterExpression::Comparison {
                field: "dept".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("eng"),
            },
        );
        store
    }

    #[test]
    fn enforcer_loads_bindings_from_the_store_not_the_held_set() {
        // The enforcer is built with EMPTY held bindings + a durable store holding
        // tenant 7's permit. predicate_for must still resolve — proof it consulted
        // the store, not the (empty) held set.
        let dir = std::env::temp_dir().join(format!(
            "proximadb-abac-enforcer-store-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("policy.json");

        let mut authority = InMemoryAttributeAuthority::new();
        authority.upsert(
            proximadb_abac::AttributeBinding::new("alice", 7)
                .with_attr("dept", AttrValue::Str("eng".into())),
        );
        let mut store = FileSystemPolicyBindingStore::open(&path).expect("open");
        store.replace_tenant(
            7,
            vec![PolicyBinding {
                object_id: 1,
                tenant_stable_id: 7,
                scope: Scope::Table(200),
                effect: Effect::Permit,
                predicate_ref: Some(42),
                field_mask: None,
            }],
        );

        let enforcer = AbacEnforcer::new(
            Box::new(authority),
            Box::new(predicate_store_for_dept()),
            Box::new(InMemoryPolicyEpochs::new()),
        )
        // NOTE: no with_bindings — held bindings are empty by design.
        .with_binding_store(Box::new(store));

        match enforcer.predicate_for(
            &SubjectId("alice".into()),
            7,
            Target {
                namespace: 3,
                table: 200,
                column: None,
            },
        ) {
            AbacScanResult::Restricted(pred) => {
                assert!(
                    pred(&record_with("eng")),
                    "alice (dept=eng) admits eng rows"
                );
                assert!(!pred(&record_with("hr")), "dept=hr is denied");
            }
            _ => panic!("alice must be Restricted via the store, not Denied/Unrestricted"),
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn enforcer_reconstitutes_the_same_result_after_a_restart() {
        // The Phase-5b ratchet at the enforcer level: write tenant 7's policy via
        // the durable store, drop it, reopen it, rebuild the enforcer — and the
        // compiled AbacScanResult for alice is byte-identical. The policy survived
        // the restart; the enforcement outcome did too.
        let dir = std::env::temp_dir().join(format!(
            "proximadb-abac-enforcer-restart-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("policy.json");
        let target = Target {
            namespace: 3,
            table: 200,
            column: None,
        };

        // Write phase: persist tenant 7's permit, then drop everything.
        {
            let mut store = FileSystemPolicyBindingStore::open(&path).expect("open");
            store.replace_tenant(
                7,
                vec![PolicyBinding {
                    object_id: 1,
                    tenant_stable_id: 7,
                    scope: Scope::Table(200),
                    effect: Effect::Permit,
                    predicate_ref: Some(42),
                    field_mask: None,
                }],
            );
        }

        // Read phase: reopen the durable store and build a fresh enforcer over it
        // (authority/predicate-store/epochs are reconstructed identically — only
        // the policy source is what persisted).
        fn fresh_enforcer(store: FileSystemPolicyBindingStore) -> AbacEnforcer {
            let mut authority = InMemoryAttributeAuthority::new();
            authority.upsert(
                proximadb_abac::AttributeBinding::new("alice", 7)
                    .with_attr("dept", AttrValue::Str("eng".into())),
            );
            AbacEnforcer::new(
                Box::new(authority),
                Box::new(predicate_store_for_dept()),
                Box::new(InMemoryPolicyEpochs::new()),
            )
            .with_binding_store(Box::new(store))
        }

        let store = FileSystemPolicyBindingStore::open(&path).expect("reopen");
        let enforcer = fresh_enforcer(store);
        let result = enforcer.predicate_for(&SubjectId("alice".into()), 7, target);

        match result {
            AbacScanResult::Restricted(pred) => {
                assert!(pred(&record_with("eng")));
                assert!(!pred(&record_with("hr")));
            }
            _ => panic!("alice must be Restricted after restart"),
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    // --- Phase 3: vector-path post-filter ---

    use proximadb_search_types::results::OptimizedSearchRecord;
    use std::collections::HashMap;

    fn search_result(id: &str, dept: &str) -> OptimizedSearchRecord {
        let mut metadata = HashMap::new();
        metadata.insert("dept".to_string(), ProximaValue::String(dept.to_string()));
        OptimizedSearchRecord {
            id: id.to_string(),
            metadata,
            ..Default::default()
        }
    }

    #[test]
    fn post_filter_removes_inadmissible_search_results() {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(
            42,
            FilterExpression::Comparison {
                field: "dept".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("eng"),
            },
        );

        let results = vec![
            search_result("r1", "eng"),
            search_result("r2", "hr"),
            search_result("r3", "eng"),
            search_result("r4", "legal"),
        ];

        let security = compile_security_filter(&[42], &store).expect("compiled");
        let filtered = post_filter_search_results(results, &security);

        assert_eq!(filtered.len(), 2, "only dept=eng results survive");
        assert_eq!(filtered[0].id, "r1");
        assert_eq!(filtered[1].id, "r3");
    }

    #[test]
    fn enforcer_filter_search_results_denies_unbound() {
        let (enforcer, bindings) = enforcer_with_alice("eng");
        let results = vec![search_result("r1", "eng")];
        let outcome = enforcer.filter_search_results(
            results,
            &SubjectId("mallory".into()),
            7,
            Target {
                namespace: 3,
                table: 200,
                column: None,
            },
            &bindings,
        );
        assert!(outcome.is_err(), "unbound subject denied");
    }
}
