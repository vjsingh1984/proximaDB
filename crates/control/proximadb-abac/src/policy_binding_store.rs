// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Phase 5b — the durable policy-binding store.
//!
//! ABAC's enforcement needs two server-side sources of truth: the **authority**
//! (a subject's attributes — [`AttributeAuthority`](crate::authority::AttributeAuthority),
//! Phase 5a) and the **policy** (the `PolicyBinding`s that govern a target — this
//! module). FA-c Phase 2 / #1324 wired the `AbacEnforcer` into the relational read
//! path, but every test supplies the bindings in-memory: there was no durable
//! source, so `abac-policy`-on in production denied every read fail-closed
//! (`DenyReason::NoApplicablePolicy`). This module is that missing half.
//!
//! ## Why an empty binding set is *correct* here (unlike the authority)
//!
//! A subtlety that distinguishes this store from [`AttributeAuthority`]:
//! [`resolve_effective_policy`](proximadb_catalog::fc_metamodel::resolve_effective_policy)
//! is **deny-biased** — a tenant with *no* bindings composes to a fail-closed
//! `Deny` (`applicable == 0`). So returning an empty `Vec` for an unknown tenant
//! is a *well-defined* "deny everything" policy, not an error. Contrast the
//! authority, where an empty attribute bag would sail through a Permit-with-no-
//! predicate policy and admit everything (fail-*open*), which is why the authority
//! treats a missing binding as [`AuthorityError`](crate::AuthorityError) instead.
//! A *store outage* (unreachable/corrupt file) is still an error
//! ([`PolicyStoreError::Unavailable`]) — an outage is not a policy.
//!
//! ## OSS mechanism (ADR-060)
//!
//! The durable impl ([`FileSystemPolicyBindingStore`]) uses the same persistence
//! *mechanism* as [`FileSystemAttributeAuthority`](crate::FileSystemAttributeAuthority):
//! atomic temp-file + rename, load-on-open, best-effort persist. The commercial
//! crate supplies the IdP-administered production store; this is the
//! development/reference impl that survives a process restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proximadb_catalog::fc_metamodel::{PolicyBinding, TenantStableId};

/// A durable source of a tenant's [`PolicyBinding`]s — the "policy" half of the
/// ABAC durable substrate (the "authority" half is
/// [`AttributeAuthority`](crate::authority::AttributeAuthority)).
///
/// Tenant-scoped by argument: a read for tenant T must consult only T's
/// bindings, even when the same target id exists in another tenant (the
/// `tenant_stable_id` field on every [`PolicyBinding`] is what keeps policy from
/// crossing the tenant boundary). Implementations are `Send + Sync`: the
/// [`AbacEnforcer`](crate::AbacEnforcer) (well, the adapter in the root crate)
/// holds the store behind a `Box<dyn PolicyBindingStore + Send + Sync>` so it can
/// be shared across the read-serving services.
pub trait PolicyBindingStore: Send + Sync {
    /// All policy bindings the store holds for `tenant_stable_id`. An empty
    /// `Vec` is the well-defined "deny everything" policy (deny-biased
    /// resolution), **not** an error — see the module docs.
    fn bindings_for(&self, tenant_stable_id: TenantStableId) -> Vec<PolicyBinding>;
}

/// Why a policy-binding store could not be consulted.
///
/// Only the **outage** case is modeled: a missing/corrupt/on-disk-unreachable
/// store. "No bindings for this tenant" is *not* an error — it is a well-defined
/// deny-everything policy (see module docs). Callers must treat [`Unavailable`]
/// as deny: an authorization substrate does not degrade to "allow" when its
/// source of truth is unreachable.
///
/// [`Unavailable`]: PolicyStoreError::Unavailable
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyStoreError {
    /// The store could not be read or written (file IO, deserialization). The
    /// caller must deny on this — never return an empty `Vec`, which would be a
    /// *policy decision* (deny-everything) rather than the outage it is.
    #[error("policy binding store unavailable: {detail}")]
    Unavailable {
        /// Operator-facing detail.
        detail: String,
    },
}

/// An in-memory [`PolicyBindingStore`] — the reference implementation, and the
/// one tests use.
///
/// Mirrors [`InMemoryAttributeAuthority`](crate::authority::InMemoryAttributeAuthority):
/// a `tenant → Vec<binding>` map. The durable implementation stores bindings as
/// catalog objects in the reserved system namespace; this type deliberately keeps
/// the same `(tenant, object_id)`-keyed shape so the durable impl can wrap it.
#[derive(Debug, Default)]
pub struct InMemoryPolicyBindingStore {
    bindings: BTreeMap<TenantStableId, Vec<PolicyBinding>>,
}

impl InMemoryPolicyBindingStore {
    /// A store holding no bindings (so every resolution denies).
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace one binding, keyed by `(tenant_stable_id, object_id)`.
    /// A binding with an `object_id` already present in its tenant replaces the
    /// old one (idempotent re-PUT); otherwise it is appended.
    pub fn upsert(&mut self, binding: PolicyBinding) {
        let tenant = binding.tenant_stable_id;
        let vec = self.bindings.entry(tenant).or_default();
        if let Some(slot) = vec.iter_mut().find(|b| b.object_id == binding.object_id) {
            *slot = binding;
        } else {
            vec.push(binding);
        }
    }

    /// Replace a tenant's entire binding set atomically. This is the natural
    /// admin operation ("publish tenant T's policy"); it drops every existing
    /// binding for `tenant_stable_id` and installs `bindings`.
    pub fn replace_tenant(
        &mut self,
        tenant_stable_id: TenantStableId,
        bindings: Vec<PolicyBinding>,
    ) {
        self.bindings.insert(tenant_stable_id, bindings);
    }

    /// Remove a single binding by `(tenant, object_id)`. Returns whether a
    /// binding was present.
    pub fn remove(&mut self, tenant_stable_id: TenantStableId, object_id: u64) -> bool {
        if let Some(vec) = self.bindings.get_mut(&tenant_stable_id) {
            let before = vec.len();
            vec.retain(|b| b.object_id != object_id);
            vec.len() != before
        } else {
            false
        }
    }

    /// An iterator over all bindings — for serialization/durability.
    pub fn bindings(&self) -> impl Iterator<Item = &PolicyBinding> {
        self.bindings.values().flatten()
    }
}

impl PolicyBindingStore for InMemoryPolicyBindingStore {
    fn bindings_for(&self, tenant_stable_id: TenantStableId) -> Vec<PolicyBinding> {
        self.bindings
            .get(&tenant_stable_id)
            .cloned()
            .unwrap_or_default()
    }
}

// ===========================================================================
// FileSystemPolicyBindingStore — durable backing (Phase 5b)
// ===========================================================================

/// A [`PolicyBindingStore`] backed by an atomic-rename JSON snapshot on the
/// filesystem — modeled on
/// [`FileSystemAttributeAuthority`](crate::FileSystemAttributeAuthority).
///
/// **OSS mechanism** (ADR-060): this is the persistence *mechanism* — atomic
/// rename, load-on-open, best-effort persist. The commercial crate supplies the
/// IdP-administered production store; this is the development/reference impl
/// that survives a process restart.
///
/// Writes are **synchronous and atomic**: `upsert`/`replace_tenant`/`remove`
/// immediately persist the full binding set via a temp-file + rename — the same
/// trade-off [`FileSystemAttributeAuthority`] makes (simple, correct, adequate
/// for development; a production impl might use per-binding rows).
pub struct FileSystemPolicyBindingStore {
    inner: InMemoryPolicyBindingStore,
    path: PathBuf,
}

impl FileSystemPolicyBindingStore {
    /// Open (or create) the store at `path`. If the file exists, bindings are
    /// loaded — this is the **restart-recovery** path. A missing file is an empty
    /// store (deny-everything), not an error.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PolicyStoreError> {
        let path = path.as_ref().to_path_buf();
        let mut inner = InMemoryPolicyBindingStore::new();
        if path.exists() {
            let data = std::fs::read(&path).map_err(|e| PolicyStoreError::Unavailable {
                detail: format!("read {}: {e}", path.display()),
            })?;
            let bindings: Vec<PolicyBinding> =
                serde_json::from_slice(&data).map_err(|e| PolicyStoreError::Unavailable {
                    detail: format!("deserialize {}: {e}", path.display()),
                })?;
            for b in bindings {
                inner.upsert(b);
            }
        }
        Ok(Self { inner, path })
    }

    /// Insert or replace a binding and persist immediately (atomic rename).
    pub fn upsert(&mut self, binding: PolicyBinding) {
        self.inner.upsert(binding);
        let _ = self.persist();
    }

    /// Replace a tenant's binding set and persist immediately.
    pub fn replace_tenant(
        &mut self,
        tenant_stable_id: TenantStableId,
        bindings: Vec<PolicyBinding>,
    ) {
        self.inner.replace_tenant(tenant_stable_id, bindings);
        let _ = self.persist();
    }

    /// Remove a binding and persist immediately.
    pub fn remove(&mut self, tenant_stable_id: TenantStableId, object_id: u64) -> bool {
        let removed = self.inner.remove(tenant_stable_id, object_id);
        if removed {
            let _ = self.persist();
        }
        removed
    }

    /// Write the full binding set atomically (temp + rename).
    fn persist(&self) -> Result<(), PolicyStoreError> {
        let bindings: Vec<PolicyBinding> = self.inner.bindings().cloned().collect();
        let json =
            serde_json::to_vec_pretty(&bindings).map_err(|e| PolicyStoreError::Unavailable {
                detail: format!("serialize bindings: {e}"),
            })?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| PolicyStoreError::Unavailable {
            detail: format!("write {}: {e}", tmp.display()),
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|e| PolicyStoreError::Unavailable {
            detail: format!("rename {} → {}: {e}", tmp.display(), self.path.display()),
        })?;
        Ok(())
    }
}

impl PolicyBindingStore for FileSystemPolicyBindingStore {
    fn bindings_for(&self, tenant_stable_id: TenantStableId) -> Vec<PolicyBinding> {
        self.inner.bindings_for(tenant_stable_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_catalog::fc_metamodel::{
        Effect, FieldMask, Scope, Target, resolve_effective_policy,
    };

    /// One permit-with-predicate binding governing table 200 in tenant 7.
    fn permit_binding(object_id: u64, tenant: u64, table: u32, pred: Option<u64>) -> PolicyBinding {
        PolicyBinding {
            object_id,
            tenant_stable_id: tenant,
            scope: Scope::Table(table),
            effect: Effect::Permit,
            predicate_ref: pred,
            field_mask: None,
        }
    }

    #[test]
    fn in_memory_returns_only_the_tenants_bindings() {
        let mut store = InMemoryPolicyBindingStore::new();
        store.upsert(permit_binding(1, 7, 200, Some(42)));
        store.upsert(permit_binding(2, 8, 200, Some(43)));

        assert_eq!(store.bindings_for(7).len(), 1);
        assert_eq!(store.bindings_for(8).len(), 1);
        assert!(
            store.bindings_for(9).is_empty(),
            "an unknown tenant has no bindings (deny-everything), not an error"
        );
    }

    #[test]
    fn upsert_replaces_by_object_id() {
        let mut store = InMemoryPolicyBindingStore::new();
        store.upsert(permit_binding(1, 7, 200, Some(42)));
        // Same (tenant, object_id) with a different predicate → replace, not append.
        store.upsert(permit_binding(1, 7, 200, Some(99)));
        let bindings = store.bindings_for(7);
        assert_eq!(bindings.len(), 1, "re-PUT replaces, it does not duplicate");
        assert_eq!(bindings[0].predicate_ref, Some(99));
    }

    #[test]
    fn replace_tenant_drops_the_old_set() {
        let mut store = InMemoryPolicyBindingStore::new();
        store.upsert(permit_binding(1, 7, 200, Some(42)));
        store.replace_tenant(7, vec![permit_binding(2, 7, 201, Some(43))]);

        let bindings = store.bindings_for(7);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].object_id, 2, "the old binding 1 is gone");
    }

    #[test]
    fn empty_binding_set_resolves_to_fail_closed_deny() {
        // The whole point: an empty store is a well-defined "deny everything"
        // policy, NOT an error. resolve_effective_policy must deny.
        let store = InMemoryPolicyBindingStore::new();
        let bindings = store.bindings_for(7);
        let eff = resolve_effective_policy(
            &bindings,
            &Target {
                namespace: 3,
                table: 200,
                column: None,
            },
        );
        assert_eq!(eff.decision, Effect::Deny);
        assert_eq!(eff.applicable, 0);
    }

    #[test]
    fn a_permit_binding_resolves_to_permit() {
        let mut store = InMemoryPolicyBindingStore::new();
        store.upsert(permit_binding(1, 7, 200, None));
        let eff = resolve_effective_policy(
            &store.bindings_for(7),
            &Target {
                namespace: 3,
                table: 200,
                column: None,
            },
        );
        assert_eq!(eff.decision, Effect::Permit);
        assert_eq!(eff.applicable, 1);
    }

    #[test]
    fn bindings_round_trip_through_serde_json() {
        // FileSystemPolicyBindingStore serializes Vec<PolicyBinding>; confirm the
        // whole struct (scope/effect/field_mask/optionals) survives.
        let bindings = vec![
            permit_binding(1, 7, 200, Some(42)),
            PolicyBinding {
                object_id: 2,
                tenant_stable_id: 7,
                scope: Scope::Column {
                    table: 200,
                    column: 3,
                },
                effect: Effect::Deny,
                predicate_ref: None,
                field_mask: Some(FieldMask::Redact),
            },
        ];
        let json = serde_json::to_vec(&bindings).expect("serialize");
        let back: Vec<PolicyBinding> = serde_json::from_slice(&json).expect("deserialize");
        assert_eq!(bindings, back);
    }

    // --- FileSystemPolicyBindingStore: restart recovery (the Phase-5b ratchet) ---

    fn unique_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proximadb-abac-policy-store-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn fs_store_bindings_survive_a_restart() {
        let dir = unique_dir("restart");
        let path = dir.join("policy.json");

        // Write phase: create the store, publish tenant 7's policy, drop it.
        {
            let mut store = FileSystemPolicyBindingStore::open(&path).expect("open");
            store.replace_tenant(
                7,
                vec![
                    permit_binding(1, 7, 200, Some(42)),
                    permit_binding(2, 7, 201, None),
                ],
            );
            assert!(path.exists(), "persist wrote the file");
        }
        // `store` is dropped — in-memory state is gone.

        // Read phase: reopen from the same path — both bindings must survive.
        let store = FileSystemPolicyBindingStore::open(&path).expect("reopen");
        let bindings = store.bindings_for(7);
        assert_eq!(bindings.len(), 2, "tenant 7's policy survived the restart");

        let eff = resolve_effective_policy(
            &bindings,
            &Target {
                namespace: 3,
                table: 200,
                column: None,
            },
        );
        assert_eq!(eff.decision, Effect::Permit);
        assert_eq!(eff.applicable, 1, "only the table-200 binding applies");

        // A different tenant still has no bindings (no cross-tenant leak).
        assert!(store.bindings_for(8).is_empty());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn fs_store_upsert_and_remove_persist_individually() {
        let dir = unique_dir("upsert-remove");
        let path = dir.join("policy.json");

        {
            let mut store = FileSystemPolicyBindingStore::open(&path).expect("open");
            store.upsert(permit_binding(1, 7, 200, Some(42)));
            assert!(store.remove(7, 1), "binding 1 was present");
            assert!(!store.remove(7, 1), "binding 1 is now gone");
        }

        let store = FileSystemPolicyBindingStore::open(&path).expect("reopen");
        assert!(
            store.bindings_for(7).is_empty(),
            "the removed binding must not survive restart"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn fs_store_opening_a_missing_file_is_an_empty_store_not_an_error() {
        let dir = unique_dir("missing");
        let path = dir.join("does-not-exist.json");

        let store = FileSystemPolicyBindingStore::open(&path).expect("missing file ⇒ empty store");
        assert!(store.bindings_for(7).is_empty());

        let _ = std::fs::remove_dir(&dir);
    }
}
