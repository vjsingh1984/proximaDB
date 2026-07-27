// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! P3 — subject + policy-epoch cache keying (TF-2 S10).
//!
//! A cache above the enforcement seam defeats the whole plane: subject A warms
//! `SELECT *`, subject B in the same tenant issues the identical query, and the
//! result cache — keyed on `(tenant, namespace, query)` — hands B the rows B is
//! not allowed to see. The result cache, the vector-search cache and the LLM
//! semantic cache all have this shape today.
//!
//! Two things must be folded into every client-servable cache key:
//!
//! * **The subject's effective-attribute digest**, so two subjects who see
//!   different worlds cannot share an entry. It keys on the *world*, not the
//!   principal — two subjects with identical load-bearing attributes legitimately
//!   share cache entries, which is where the hit rate comes back.
//! * **The policy epoch**, so editing a policy invalidates every dependent entry
//!   without walking the cache. A stale epoch is a stale key, and a stale key
//!   simply misses.
//!
//! Caches that cannot adopt the key are **bypassed** while `abac-policy` is on;
//! there is no third option.

use std::collections::BTreeMap;

use proximadb_catalog::fc_metamodel::{NamespaceId, TenantStableId};

use crate::authority::AttributeDigest;

/// A monotonically increasing counter, bumped on every change to the policy
/// bindings that govern a `(tenant, namespace)`.
///
/// Monotonic and opaque: consumers may only compare it for equality and read it
/// as an opaque key component. Bumping is the invalidation hook — no cache walk,
/// no per-entry tracking of which policy produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct PolicyEpoch(pub u64);

impl PolicyEpoch {
    /// The epoch of a `(tenant, namespace)` whose policy has never changed.
    pub const INITIAL: PolicyEpoch = PolicyEpoch(0);

    /// The next epoch. Saturating: an epoch that stops advancing would silently
    /// stop invalidating, so it pins at `u64::MAX` rather than wrapping back into
    /// a previously used value.
    pub fn next(self) -> Self {
        PolicyEpoch(self.0.saturating_add(1))
    }
}

/// Where a read's current policy epoch comes from.
pub trait PolicyEpochSource {
    /// The current epoch for a `(tenant, namespace)`.
    ///
    /// A `(tenant, namespace)` never seen before reads as [`PolicyEpoch::INITIAL`]
    /// — safe because the epoch only ever needs to *change* when policy changes;
    /// a never-edited scope has nothing to invalidate.
    fn epoch(&self, tenant_stable_id: TenantStableId, namespace: NamespaceId) -> PolicyEpoch;
}

/// An in-memory [`PolicyEpochSource`] — the reference implementation.
#[derive(Debug, Default)]
pub struct InMemoryPolicyEpochs {
    epochs: BTreeMap<(TenantStableId, NamespaceId), PolicyEpoch>,
}

impl InMemoryPolicyEpochs {
    /// An epoch registry where every scope is at [`PolicyEpoch::INITIAL`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the epoch for a `(tenant, namespace)` — the invalidation hook a
    /// policy write calls after it commits. Returns the new epoch.
    pub fn bump(
        &mut self,
        tenant_stable_id: TenantStableId,
        namespace: NamespaceId,
    ) -> PolicyEpoch {
        let entry = self
            .epochs
            .entry((tenant_stable_id, namespace))
            .or_insert(PolicyEpoch::INITIAL);
        *entry = entry.next();
        *entry
    }
}

impl PolicyEpochSource for InMemoryPolicyEpochs {
    fn epoch(&self, tenant_stable_id: TenantStableId, namespace: NamespaceId) -> PolicyEpoch {
        self.epochs
            .get(&(tenant_stable_id, namespace))
            .copied()
            .unwrap_or(PolicyEpoch::INITIAL)
    }
}

/// The component every client-servable cache key must fold in, alongside
/// whatever the cache already keys on (query text, vector, question, …).
///
/// It is deliberately *not* a whole cache key: each cache keeps its own key type
/// and adds this. That keeps the invariant checkable — "does this cache's key
/// contain a `SubjectCacheKey`?" — instead of asking every cache to reimplement
/// subject binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubjectCacheKey {
    /// Tenant (ADR-075 stable id) — structural isolation, still keyed explicitly.
    pub tenant_stable_id: TenantStableId,
    /// Namespace (ADR-074 boundary).
    pub namespace: NamespaceId,
    /// The policy generation this entry was computed under.
    pub policy_epoch: PolicyEpoch,
    /// Digest of the subject's load-bearing attributes.
    pub attribute_digest: AttributeDigest,
}

impl SubjectCacheKey {
    /// Build the key component for a subject in a scope.
    pub fn new(
        tenant_stable_id: TenantStableId,
        namespace: NamespaceId,
        policy_epoch: PolicyEpoch,
        attribute_digest: AttributeDigest,
    ) -> Self {
        Self {
            tenant_stable_id,
            namespace,
            policy_epoch,
            attribute_digest,
        }
    }

    /// A stable string rendering, for caches whose keys are strings.
    pub fn as_key_component(&self) -> String {
        format!(
            "t{}:n{}:e{}:a{}",
            self.tenant_stable_id,
            self.namespace,
            self.policy_epoch.0,
            self.attribute_digest.to_hex()
        )
    }
}

#[cfg(test)]
mod tests {
    use proximadb_catalog::fc_metamodel::{AttrValue, SubjectId};

    use super::*;
    use crate::authority::{
        AttributeAuthority, AttributeBinding, InMemoryAttributeAuthority, ResolvedSubject,
    };

    fn world() -> InMemoryAttributeAuthority {
        let mut a = InMemoryAttributeAuthority::new();
        a.upsert(AttributeBinding::new("alice", 7).with_attr("dept", AttrValue::Str("eng".into())));
        a.upsert(AttributeBinding::new("bob", 7).with_attr("dept", AttrValue::Str("hr".into())));
        a.upsert(AttributeBinding::new("carol", 7).with_attr("dept", AttrValue::Str("eng".into())));
        a
    }

    fn resolve(a: &InMemoryAttributeAuthority, who: &str) -> ResolvedSubject {
        a.resolve_effective_attributes(&SubjectId(who.into()), 7)
            .expect("bound")
    }

    fn key_for(a: &InMemoryAttributeAuthority, who: &str, epoch: PolicyEpoch) -> SubjectCacheKey {
        SubjectCacheKey::new(7, 3, epoch, resolve(a, who).digest)
    }

    #[test]
    fn two_subjects_in_different_departments_do_not_share_a_cache_entry() {
        // The S10 scenario: alice warms a query, bob issues the identical one.
        let a = world();
        assert_ne!(
            key_for(&a, "alice", PolicyEpoch::INITIAL),
            key_for(&a, "bob", PolicyEpoch::INITIAL)
        );
    }

    #[test]
    fn subjects_seeing_the_same_world_do_share_a_cache_entry() {
        // Keying on the world rather than the principal is what keeps the cache
        // useful — alice and carol are both dept=eng.
        let a = world();
        assert_eq!(
            key_for(&a, "alice", PolicyEpoch::INITIAL),
            key_for(&a, "carol", PolicyEpoch::INITIAL)
        );
    }

    #[test]
    fn a_policy_change_invalidates_by_changing_the_key() {
        let a = world();
        let mut epochs = InMemoryPolicyEpochs::new();
        let before = key_for(&a, "alice", epochs.epoch(7, 3));
        let after_epoch = epochs.bump(7, 3);
        let after = key_for(&a, "alice", after_epoch);

        assert_ne!(before, after);
        assert_eq!(after_epoch, PolicyEpoch(1));
    }

    #[test]
    fn epochs_are_scoped_per_tenant_and_namespace() {
        let mut epochs = InMemoryPolicyEpochs::new();
        epochs.bump(7, 3);
        // Bumping tenant 7's namespace 3 must not invalidate anyone else's cache.
        assert_eq!(epochs.epoch(7, 3), PolicyEpoch(1));
        assert_eq!(epochs.epoch(7, 4), PolicyEpoch::INITIAL);
        assert_eq!(epochs.epoch(8, 3), PolicyEpoch::INITIAL);
    }

    #[test]
    fn an_unknown_scope_reads_as_the_initial_epoch() {
        assert_eq!(
            InMemoryPolicyEpochs::new().epoch(99, 99),
            PolicyEpoch::INITIAL
        );
    }

    #[test]
    fn the_epoch_never_wraps_back_onto_a_used_value() {
        // Wrapping would resurrect a key that cached results already live under.
        assert_eq!(PolicyEpoch(u64::MAX).next(), PolicyEpoch(u64::MAX));
    }

    #[test]
    fn the_string_rendering_carries_every_component() {
        let a = world();
        let k = key_for(&a, "alice", PolicyEpoch(5));
        let s = k.as_key_component();
        assert!(s.starts_with("t7:n3:e5:a"));
        // …and distinguishes the same subject at a different epoch.
        assert_ne!(s, key_for(&a, "alice", PolicyEpoch(6)).as_key_component());
    }

    #[test]
    fn the_key_separates_namespaces_within_a_tenant() {
        let a = world();
        let digest = resolve(&a, "alice").digest;
        assert_ne!(
            SubjectCacheKey::new(7, 3, PolicyEpoch::INITIAL, digest),
            SubjectCacheKey::new(7, 4, PolicyEpoch::INITIAL, digest)
        );
    }
}
