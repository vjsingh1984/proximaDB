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
use std::path::{Path, PathBuf};
use std::sync::RwLock;

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
/// # Scope: cluster-global, operator-administered — NOT tenant-partitioned
///
/// This doc previously claimed the trait was "structurally tenant-scoped" and
/// that "a cross-tenant ref fails to resolve." **No implementation has ever
/// provided that**, and `FileSystemPredicateObjectStore`'s own doc says the
/// opposite three screens below. Two contradictory guarantees in one file is
/// worse than a documented limitation: a reader cannot tell which one the code
/// keeps. The accurate contract is written here instead.
///
/// A predicate object is a **cluster-scope** object. Both the write path
/// (`PUT /api/v2/abac/predicate-objects/{object_id}`) and the binding that
/// names one (`PUT /api/v2/abac/policy-bindings/{tenant}/{object_id}`) require
/// `SystemAdmin` ∪ `ConfigureSystem` via `authorize_operator` — verified for
/// all 14 handlers in `rest/canonical/abac_admin.rs`. So the same id resolving
/// for two tenants is **not** a cross-tenant escalation: only a cluster
/// operator, who already administers every tenant, can create one or reference
/// it. `predicate_objects_are_cluster_scope_not_tenant_scoped` pins that.
///
/// If a tenant-facing policy API is ever added, this becomes load-bearing and
/// the store must be keyed by `(TenantStableId, ObjectId)` **before** that API
/// ships — tracked in TD-AUTHZ-1 L2.3.
pub trait PredicateObjectStore {
    /// Look up the predicate object registered under `id`. Returns `None` when
    /// the id is unknown or revoked — [`compile_security_filter`] treats `None`
    /// as fail-closed. It does **not** return `None` for another tenant's
    /// object; see the scope note above.
    ///
    /// Returns an **owned** `FilterExpression` (not a borrow) so a durable
    /// implementation can hold its cache behind a lock — a reference could not
    /// escape the read guard. The single consumer
    /// ([`compile_security_filter`]) already cloned the borrow, so this removes a
    /// clone rather than adding one.
    fn get(&self, id: ObjectId) -> Option<FilterExpression>;
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
    fn get(&self, id: ObjectId) -> Option<FilterExpression> {
        self.objects.get(&id).cloned()
    }
}

// ===========================================================================
// FileSystemPredicateObjectStore — durable backing (Phase 5b)
// ===========================================================================

/// A [`PredicateObjectStore`] backed by an atomic-rename JSON snapshot on the
/// filesystem — the durable counterpart to [`InMemoryPredicateObjectStore`],
/// modeled on [`FileSystemPolicyBindingStore`](crate::FileSystemPolicyBindingStore).
///
/// This is the second half of the durable substrate that makes ABAC row-level
/// enforcement work in production: a `PolicyBinding.predicate_ref` points here,
/// and [`compile_security_filter`] resolves it. Without a durable predicate
/// store, a binding that carries a predicate ref resolves against an empty
/// in-memory store in production and denies fail-closed — so only
/// predicate-free table-level grants evaluated end-to-end. With this store,
/// admin-defined row predicates (e.g. `dept == $subject.dept`) survive a restart.
///
/// **OSS mechanism** (ADR-060): atomic temp-file + rename, load-on-open,
/// best-effort persist — the same mechanism
/// [`FileSystemAttributeAuthority`](crate::FileSystemAttributeAuthority) and
/// [`FileSystemPolicyBindingStore`] use. The commercial crate supplies the
/// IdP-administered production store; this is the development/reference impl.
///
/// Predicate objects are keyed by global catalog `ObjectId` (not tenant-
/// partitioned), matching [`InMemoryPredicateObjectStore`] and the enforcer's
/// single shared store — a predicate ref resolves the same `FilterExpression`
/// regardless of tenant. This is deliberate and operator-gated; see the scope
/// note on [`PredicateObjectStore`]. `Send + Sync` (held by the enforcer behind a shared
/// `Arc<dyn PredicateObjectStore + Send + Sync>`). The in-memory cache is behind
/// a [`RwLock`] so an admin write through a shared `Arc` handle is visible to the
/// live enforcer without a restart (hot-reload): writes take the write-lock and
/// persist; reads take the read-lock.
pub struct FileSystemPredicateObjectStore {
    inner: RwLock<InMemoryPredicateObjectStore>,
    path: PathBuf,
}

impl FileSystemPredicateObjectStore {
    /// Open (or create) the store at `path`. If the file exists, predicate
    /// objects are loaded — this is the **restart-recovery** path. A missing
    /// file is an empty store (every ref fails to resolve → deny), not an error.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PredicateStoreError> {
        let path = path.as_ref().to_path_buf();
        let mut inner = InMemoryPredicateObjectStore::new();
        if path.exists() {
            let data = std::fs::read(&path).map_err(|e| PredicateStoreError::Unavailable {
                detail: format!("read {}: {e}", path.display()),
            })?;
            let objects: Vec<(ObjectId, FilterExpression)> = serde_json::from_slice(&data)
                .map_err(|e| PredicateStoreError::Unavailable {
                    detail: format!("deserialize {}: {e}", path.display()),
                })?;
            for (id, expr) in objects {
                inner.register(id, expr);
            }
        }
        Ok(Self {
            inner: RwLock::new(inner),
            path,
        })
    }

    /// Register or replace a predicate object and persist immediately (atomic rename).
    ///
    /// Takes `&self` (not `&mut self`): the in-memory cache lives behind a
    /// [`RwLock`], so a shared `Arc<FileSystemPredicateObjectStore>` handle can
    /// mutate the store — the admin-provisioning path (TD-ABAC control-plane)
    /// writes through the same instance the live enforcer reads, and the change
    /// is visible without a restart.
    pub fn register(&self, id: ObjectId, expr: FilterExpression) {
        {
            let mut guard = self.inner.write().unwrap_or_else(|p| p.into_inner());
            guard.register(id, expr);
        }
        let _ = self.persist();
    }

    /// Remove a predicate object and persist immediately. Subsequent resolves of
    /// `id` fail-closed. `&self` for the same hot-reload reason as [`register`](Self::register).
    pub fn revoke(&self, id: ObjectId) {
        {
            let mut guard = self.inner.write().unwrap_or_else(|p| p.into_inner());
            guard.revoke(id);
        }
        let _ = self.persist();
    }

    /// A snapshot of the stored predicate objects — for inspection/audit. Owned
    /// (not an iterator borrowing `&self`) because the cache is behind a [`RwLock`].
    pub fn objects(&self) -> Vec<(ObjectId, FilterExpression)> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .objects
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }

    /// Write the full predicate-object set atomically (temp + rename). Re-reads
    /// the live cache under the read-lock — so it always reflects the latest
    /// committed state (no lost update under concurrent admin writes).
    fn persist(&self) -> Result<(), PredicateStoreError> {
        let objects: Vec<(ObjectId, FilterExpression)> = self
            .inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .objects
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        let json =
            serde_json::to_vec_pretty(&objects).map_err(|e| PredicateStoreError::Unavailable {
                detail: format!("serialize predicate objects: {e}"),
            })?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| PredicateStoreError::Unavailable {
            detail: format!("write {}: {e}", tmp.display()),
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|e| PredicateStoreError::Unavailable {
            detail: format!("rename {} → {}: {e}", tmp.display(), self.path.display()),
        })?;
        Ok(())
    }
}

impl PredicateObjectStore for FileSystemPredicateObjectStore {
    fn get(&self, id: ObjectId) -> Option<FilterExpression> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .objects
            .get(&id)
            .cloned()
    }
}

/// Why a predicate-object store could not be consulted (mirrors
/// [`PolicyStoreError`](crate::PolicyStoreError)). Only the **outage** case is
/// modeled — a missing predicate ref is *not* an error, it is a fail-closed
/// deny (see [`compile_security_filter`]). Callers must treat [`Unavailable`] as
/// deny: an authorization substrate does not degrade to "allow" when its source
/// of truth is unreachable.
///
/// [`Unavailable`]: PredicateStoreError::Unavailable
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PredicateStoreError {
    /// The store could not be read or written (file IO, deserialization).
    #[error("predicate object store unavailable: {detail}")]
    Unavailable {
        /// Operator-facing detail.
        detail: String,
    },
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
            Some(expr) => resolved.push(expr),
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

    /// TD-AUTHZ-1 L2.3, **corrected by measurement**. The tracker recorded
    /// "today cross-tenant deref resolves" as an open isolation gap. It does
    /// resolve — but that is not an escalation, and this test pins why so the
    /// entry is not re-opened as a security bug.
    ///
    /// Both the write path (`PUT /api/v2/abac/predicate-objects/{id}`) and the
    /// binding that names one (`PUT /api/v2/abac/policy-bindings/{tenant}/{id}`)
    /// require `SystemAdmin` ∪ `ConfigureSystem` — all 14 handlers in
    /// `abac_admin.rs` call the fail-closed `authorize_operator` first. Only a
    /// cluster operator, who already administers every tenant, can create a
    /// predicate object or point a binding at one. No tenant-facing surface
    /// writes either.
    ///
    /// **Teeth are structural, not behavioural.** A behavioural assertion here
    /// would be vacuous — with no tenant parameter to vary, "tenant A and
    /// tenant B get the same answer" is the same call twice. So the pin is on
    /// the *signature*: add a tenant argument to `get` and this stops
    /// compiling, which is exactly the moment someone should be re-reading the
    /// scope note on the trait.
    #[test]
    fn predicate_objects_are_cluster_scope_not_tenant_scoped() {
        let store = InMemoryPredicateObjectStore::new();

        // The pin: `get` takes an ObjectId and NOTHING else, so no
        // implementation of this trait can scope by tenant. Spelled as a
        // qualified call so the arity is the assertion — add a tenant argument
        // and this line stops compiling.
        let resolved: Option<FilterExpression> = PredicateObjectStore::get(&store, 42);
        assert_eq!(resolved, None, "an unregistered id is unknown to the store");

        // The fail-closed property that DOES hold, and must keep holding: an id
        // nobody registered denies rather than waving the read through.
        assert_eq!(
            compile_security_filter(&[42], &store),
            Some(unsatisfiable_filter()),
            "an unregistered ref must compile to a deny"
        );
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

    // --- FileSystemPredicateObjectStore: restart recovery (Phase-5b ratchet) ---

    fn unique_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proximadb-abac-pred-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn fs_predicate_store_objects_survive_a_restart() {
        let dir = unique_dir("restart");
        let path = dir.join("predicates.json");

        // Write phase: register predicate 42 (dept=eng) + 7 (clearance>=3), drop.
        {
            let store = FileSystemPredicateObjectStore::open(&path).expect("open");
            store.register(42, eq_dept("eng"));
            store.register(7, gt_clearance(3));
            assert!(path.exists(), "persist wrote the file");
        }

        // Read phase: reopen — both predicates must survive, byte-identical.
        let store = FileSystemPredicateObjectStore::open(&path).expect("reopen");
        assert_eq!(store.get(42), Some(eq_dept("eng")));
        assert_eq!(store.get(7), Some(gt_clearance(3)));
        assert!(store.get(999).is_none(), "unknown ref resolves None (deny)");

        // compile_security_filter — the production enforcement path — resolves
        // the restarted refs, and a missing ref compiles to the fail-closed deny.
        assert!(compile_security_filter(&[42, 7], &store).is_some());
        assert!(
            compile_security_filter(&[42, 999], &store).is_some(),
            "a missing ref compiles to the unsatisfiable deny, not None"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn fs_predicate_store_register_and_revoke_persist() {
        let dir = unique_dir("revoke");
        let path = dir.join("predicates.json");

        {
            let store = FileSystemPredicateObjectStore::open(&path).expect("open");
            store.register(42, eq_dept("eng"));
            store.revoke(42);
        }
        let store = FileSystemPredicateObjectStore::open(&path).expect("reopen");
        assert!(
            store.get(42).is_none(),
            "revoked predicate must not survive restart"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn fs_predicate_store_missing_file_is_empty_not_error() {
        let dir = unique_dir("missing");
        let path = dir.join("nope.json");
        let store = FileSystemPredicateObjectStore::open(&path).expect("missing ⇒ empty store");
        assert!(store.get(42).is_none());
        let _ = std::fs::remove_dir(&dir);
    }
}
