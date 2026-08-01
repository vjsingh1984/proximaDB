// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! P1 — the attribute-authority store.
//!
//! ABAC's "A" needs a server-side authority. Today there is none: role
//! assignments are `user_id`-keyed, single-assignment and permission-only, and
//! attributes (`dept`/`clearance`/`region`) exist only as SSO claims — i.e. as
//! values the *tenant's own IdP* asserts. TF-2 §3.1's attribute-trust boundary
//! says a value a predicate reads to admit or deny must be re-resolved
//! server-side against the authority of **the tenant whose policy is being
//! evaluated**. This module is that authority.
//!
//! Two structural properties, both tested:
//!
//! * **It is the only mint for load-bearing attributes.** Everything it returns
//!   is `AttrSource::ServerResolved`; claims are never copied in. A caller can
//!   still attach claim-sourced labels afterwards, and they stay non-load-bearing
//!   because `SubjectAttributes::load_bearing` refuses them by type.
//! * **A missing or unavailable binding is an error, never an empty bag.** An
//!   empty bag would sail through a `Permit`-with-no-predicate policy and admit
//!   everything — fail-open by omission. [`AuthorityError`] denies instead.

use std::collections::BTreeMap;

use proximadb_catalog::fc_metamodel::{AttrValue, SubjectAttributes, SubjectId, TenantStableId};
use sha2::{Digest, Sha256};

/// A durable binding of attributes to one `(subject, tenant)` pair — the record
/// the authority resolves against.
///
/// Unlike the `user_role_assignments` it replaces, a binding is
/// **`(subject, tenant)`-keyed** (so the same principal may hold different
/// attributes in different tenants) and **multi-valued** (roles are an ordinary
/// `AttrValue::List` attribute, not a single assignment column — per TF-2 §3.1,
/// "a role is just an attribute").
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttributeBinding {
    /// The principal this binding is for.
    pub subject_id: SubjectId,
    /// The tenant whose authority issued it (ADR-075 stable id).
    pub tenant_stable_id: TenantStableId,
    /// The attribute set. Ordered, so the digest is stable.
    pub attrs: BTreeMap<String, AttrValue>,
}

impl AttributeBinding {
    /// A binding with no attributes yet.
    pub fn new(subject_id: impl Into<String>, tenant_stable_id: TenantStableId) -> Self {
        Self {
            subject_id: SubjectId(subject_id.into()),
            tenant_stable_id,
            attrs: BTreeMap::new(),
        }
    }

    /// Add one attribute (builder form).
    pub fn with_attr(mut self, key: impl Into<String>, value: AttrValue) -> Self {
        self.attrs.insert(key.into(), value);
        self
    }
}

/// A collision-resistant digest of the **load-bearing** attribute set a
/// resolution produced.
///
/// It exists so a cache key can bind to "which subject-visible world this result
/// was computed under" without carrying the attributes themselves. It is minted
/// only by an [`AttributeAuthority`] — the same component that resolved the
/// attributes — so it cannot drift from the set it claims to summarize.
///
/// SHA-256-derived (truncated to 128 bits): a cache-key collision here is a
/// **cross-subject disclosure**, so a fast non-cryptographic hash is not an
/// acceptable trade (TF-2 S10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AttributeDigest([u8; 16]);

impl AttributeDigest {
    /// Digest an ordered attribute set. Length-prefixed field framing so
    /// `{"ab": "c"}` and `{"a": "bc"}` cannot collide by concatenation.
    fn of(tenant_stable_id: TenantStableId, attrs: &BTreeMap<String, AttrValue>) -> Self {
        let mut h = Sha256::new();
        h.update(b"proximadb-abac-attrs-v1");
        h.update(tenant_stable_id.to_be_bytes());
        h.update((attrs.len() as u64).to_be_bytes());
        for (k, v) in attrs {
            h.update((k.len() as u64).to_be_bytes());
            h.update(k.as_bytes());
            let encoded = encode_value(v);
            h.update((encoded.len() as u64).to_be_bytes());
            h.update(&encoded);
        }
        let out = h.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&out[..16]);
        Self(bytes)
    }

    /// The raw digest bytes, for folding into a larger cache key.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Lowercase hex, for cache keys that are strings and for audit logs.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Type-tagged, length-framed encoding of an attribute value, so values of
/// different types cannot produce the same bytes (`Int(1)` vs `Str("1")`).
fn encode_value(v: &AttrValue) -> Vec<u8> {
    let mut out = Vec::new();
    match v {
        AttrValue::Str(s) => {
            out.push(1);
            out.extend_from_slice(s.as_bytes());
        }
        AttrValue::Int(i) => {
            out.push(2);
            out.extend_from_slice(&i.to_be_bytes());
        }
        AttrValue::Bool(b) => {
            out.push(3);
            out.push(u8::from(*b));
        }
        AttrValue::List(items) => {
            out.push(4);
            out.extend_from_slice(&(items.len() as u64).to_be_bytes());
            for item in items {
                out.extend_from_slice(&(item.len() as u64).to_be_bytes());
                out.extend_from_slice(item.as_bytes());
            }
        }
    }
    out
}

/// What an [`AttributeAuthority`] resolved: the load-bearing attribute bag plus
/// the digest of exactly that bag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSubject {
    /// The subject's attributes, all `ServerResolved`.
    pub attributes: SubjectAttributes,
    /// Digest of the attribute set these were resolved from (P3's cache-key input).
    pub digest: AttributeDigest,
}

/// Why an attribute resolution denied.
///
/// Every variant is a **deny**. There is deliberately no "resolved to nothing"
/// success case: an empty bag would satisfy any policy that carries no predicate,
/// turning a missing binding into full access.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    /// The subject holds no binding in this tenant — it is not a member, or its
    /// binding was revoked.
    #[error("subject '{subject}' has no attribute binding in tenant {tenant}")]
    NoBinding {
        /// The principal that failed to resolve.
        subject: String,
        /// The tenant the lookup was scoped to.
        tenant: TenantStableId,
    },
    /// The authority could not be consulted (store outage, timeout). Denies —
    /// an authorization substrate does not degrade to "allow" when its source of
    /// truth is unreachable.
    #[error("attribute authority unavailable: {detail}")]
    Unavailable {
        /// Operator-facing detail.
        detail: String,
    },
}

/// The server-side authority for a subject's ABAC attributes (TF-2 §3.1 / S6).
///
/// Implementations are **tenant-scoped by argument**: a resolution for tenant T
/// must consult T's authority and return only T's bindings, even when the same
/// principal exists in another tenant. This is what makes a cross-tenant
/// `Reference` traversal evaluate the far policy against the *far* tenant's
/// attributes rather than the visitor's self-asserted ones.
pub trait AttributeAuthority {
    /// Resolve `subject`'s load-bearing attributes in `tenant_stable_id`.
    ///
    /// Fail-closed: `Err` on any doubt. Callers must not treat an error as an
    /// empty attribute set.
    fn resolve_effective_attributes(
        &self,
        subject: &SubjectId,
        tenant_stable_id: TenantStableId,
    ) -> Result<ResolvedSubject, AuthorityError>;
}

/// An in-memory [`AttributeAuthority`] — the reference implementation, and the
/// one tests use.
///
/// The durable implementation stores bindings as catalog objects in the reserved
/// system namespace (TF-2 §3.3), which is why this type deliberately keeps the
/// same shape: a `(subject, tenant)`-keyed lookup returning one binding.
#[derive(Debug, Default)]
pub struct InMemoryAttributeAuthority {
    bindings: BTreeMap<(TenantStableId, String), AttributeBinding>,
    /// When set, every resolution fails with this detail — models a store outage
    /// so callers can pin their fail-closed behavior.
    unavailable: Option<String>,
}

impl InMemoryAttributeAuthority {
    /// An authority holding no bindings (so every resolution denies).
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a binding.
    pub fn upsert(&mut self, binding: AttributeBinding) {
        self.bindings.insert(
            (binding.tenant_stable_id, binding.subject_id.0.clone()),
            binding,
        );
    }

    /// Remove a subject's binding in one tenant. Subsequent resolutions deny with
    /// [`AuthorityError::NoBinding`].
    pub fn revoke(&mut self, subject: &SubjectId, tenant_stable_id: TenantStableId) {
        self.bindings.remove(&(tenant_stable_id, subject.0.clone()));
    }

    /// An iterator over all bindings — for serialization/durability.
    pub fn bindings(&self) -> impl Iterator<Item = &AttributeBinding> {
        self.bindings.values()
    }

    /// Simulate the authority being unreachable (tests for the fail-closed path).
    pub fn set_unavailable(&mut self, detail: impl Into<String>) {
        self.unavailable = Some(detail.into());
    }
}

impl AttributeAuthority for InMemoryAttributeAuthority {
    fn resolve_effective_attributes(
        &self,
        subject: &SubjectId,
        tenant_stable_id: TenantStableId,
    ) -> Result<ResolvedSubject, AuthorityError> {
        if let Some(detail) = &self.unavailable {
            return Err(AuthorityError::Unavailable {
                detail: detail.clone(),
            });
        }

        let binding = self
            .bindings
            .get(&(tenant_stable_id, subject.0.clone()))
            .ok_or_else(|| AuthorityError::NoBinding {
                subject: subject.0.clone(),
                tenant: tenant_stable_id,
            })?;

        // Every attribute the authority emits is ServerResolved — this function is
        // the only place load-bearing attributes come into existence.
        let mut attributes = SubjectAttributes::new(subject.0.clone(), tenant_stable_id);
        for (k, v) in &binding.attrs {
            attributes = attributes.with_resolved(k.clone(), v.clone());
        }

        Ok(ResolvedSubject {
            attributes,
            digest: AttributeDigest::of(tenant_stable_id, &binding.attrs),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority() -> InMemoryAttributeAuthority {
        let mut a = InMemoryAttributeAuthority::new();
        a.upsert(
            AttributeBinding::new("alice", 7)
                .with_attr("dept", AttrValue::Str("eng".into()))
                .with_attr("clearance", AttrValue::Int(3))
                .with_attr(
                    "roles",
                    AttrValue::List(vec!["reader".into(), "analyst".into()]),
                ),
        );
        a
    }

    #[test]
    fn resolved_attributes_are_load_bearing() {
        let r = authority()
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("alice is bound in tenant 7");

        assert_eq!(
            r.attributes.load_bearing("dept"),
            Some(&AttrValue::Str("eng".into()))
        );
        assert_eq!(
            r.attributes.load_bearing("clearance"),
            Some(&AttrValue::Int(3))
        );
        // A role is an ordinary attribute, and multi-valued — the single-assignment
        // limit of user_role_assignments is gone.
        assert_eq!(
            r.attributes.load_bearing("roles"),
            Some(&AttrValue::List(vec!["reader".into(), "analyst".into()]))
        );
    }

    #[test]
    fn an_unbound_subject_denies_rather_than_resolving_to_an_empty_bag() {
        // The failure that matters: an empty bag sails through a Permit-with-no-
        // predicate policy. Resolution must deny instead.
        let err = authority()
            .resolve_effective_attributes(&SubjectId("mallory".into()), 7)
            .expect_err("unbound subject must not resolve");
        assert!(matches!(err, AuthorityError::NoBinding { .. }));
    }

    #[test]
    fn an_unavailable_authority_denies() {
        let mut a = authority();
        a.set_unavailable("binding store timeout");
        let err = a
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect_err("an unreachable authority must not degrade to allow");
        assert!(matches!(err, AuthorityError::Unavailable { .. }));
    }

    #[test]
    fn a_binding_does_not_leak_across_tenants() {
        // Same principal name, different tenant: alice@7's clearance must not
        // answer a lookup scoped to tenant 8.
        let a = authority();
        assert!(matches!(
            a.resolve_effective_attributes(&SubjectId("alice".into()), 8),
            Err(AuthorityError::NoBinding { tenant: 8, .. })
        ));
    }

    #[test]
    fn revocation_takes_effect_immediately() {
        let mut a = authority();
        a.revoke(&SubjectId("alice".into()), 7);
        assert!(
            a.resolve_effective_attributes(&SubjectId("alice".into()), 7)
                .is_err()
        );
    }

    #[test]
    fn the_digest_is_stable_for_the_same_attributes_and_differs_otherwise() {
        let a = authority();
        let first = a
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("bound");
        let second = a
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("bound");
        assert_eq!(first.digest, second.digest, "same world ⇒ same cache key");

        // A different subject with the *same* attributes shares the digest: the
        // cache key binds to the visible world, not to the principal.
        let mut b = InMemoryAttributeAuthority::new();
        b.upsert(
            AttributeBinding::new("bob", 7)
                .with_attr("dept", AttrValue::Str("eng".into()))
                .with_attr("clearance", AttrValue::Int(3))
                .with_attr(
                    "roles",
                    AttrValue::List(vec!["reader".into(), "analyst".into()]),
                ),
        );
        let bob = b
            .resolve_effective_attributes(&SubjectId("bob".into()), 7)
            .expect("bound");
        assert_eq!(first.digest, bob.digest);
    }

    #[test]
    fn a_changed_attribute_changes_the_digest() {
        let mut a = InMemoryAttributeAuthority::new();
        a.upsert(AttributeBinding::new("alice", 7).with_attr("dept", AttrValue::Str("eng".into())));
        let eng = a
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("bound")
            .digest;

        a.upsert(AttributeBinding::new("alice", 7).with_attr("dept", AttrValue::Str("hr".into())));
        let hr = a
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("bound")
            .digest;

        assert_ne!(eng, hr, "dept=eng and dept=hr must not share a cache key");
    }

    #[test]
    fn the_same_tenant_is_folded_into_the_digest() {
        // Two tenants, identical attributes: the digests must differ, so a cache
        // key built from the digest cannot cross the tenant boundary even if the
        // rest of the key were somehow shared.
        let mut a = InMemoryAttributeAuthority::new();
        a.upsert(AttributeBinding::new("alice", 7).with_attr("dept", AttrValue::Str("eng".into())));
        a.upsert(AttributeBinding::new("alice", 8).with_attr("dept", AttrValue::Str("eng".into())));
        let t7 = a
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("bound")
            .digest;
        let t8 = a
            .resolve_effective_attributes(&SubjectId("alice".into()), 8)
            .expect("bound")
            .digest;
        assert_ne!(t7, t8);
    }

    #[test]
    fn field_framing_prevents_concatenation_collisions() {
        // {"ab": "c"} and {"a": "bc"} concatenate to the same bytes without
        // length framing — a collision here is a cross-subject cache hit.
        let mut a = InMemoryAttributeAuthority::new();
        a.upsert(AttributeBinding::new("x", 1).with_attr("ab", AttrValue::Str("c".into())));
        a.upsert(AttributeBinding::new("y", 1).with_attr("a", AttrValue::Str("bc".into())));
        let x = a
            .resolve_effective_attributes(&SubjectId("x".into()), 1)
            .expect("bound")
            .digest;
        let y = a
            .resolve_effective_attributes(&SubjectId("y".into()), 1)
            .expect("bound")
            .digest;
        assert_ne!(x, y);
    }

    #[test]
    fn values_of_different_types_do_not_collide() {
        let mut a = InMemoryAttributeAuthority::new();
        a.upsert(AttributeBinding::new("x", 1).with_attr("k", AttrValue::Str("1".into())));
        a.upsert(AttributeBinding::new("y", 1).with_attr("k", AttrValue::Int(1)));
        let x = a
            .resolve_effective_attributes(&SubjectId("x".into()), 1)
            .expect("bound")
            .digest;
        let y = a
            .resolve_effective_attributes(&SubjectId("y".into()), 1)
            .expect("bound")
            .digest;
        // The same widening that makes `clearance>=3` vs "2" fail open (S3) would
        // make these share a cache key.
        assert_ne!(x, y);
    }

    #[test]
    fn the_digest_renders_as_hex_for_audit() {
        let d = authority()
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("bound")
            .digest;
        assert_eq!(d.to_hex().len(), 32);
        assert_eq!(d.as_bytes().len(), 16);
    }
}

// ===========================================================================
// FileSystemAttributeAuthority — durable backing (Phase 5)
// ===========================================================================

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// A [`AttributeAuthority`] backed by an atomic-rename JSON snapshot on the
/// filesystem — modeled on `FileSystemCorpusVersionStore`
/// (`crates/control/proximadb-catalog/src/corpus_version_fs_store.rs`).
///
/// **OSS mechanism** (ADR-060): this is the persistence *mechanism* — atomic
/// rename, load-on-open, best-effort persist. The commercial crate supplies the
/// IdP-backed production `AttributeAuthority`; this is the development/reference
/// impl that survives a process restart.
///
/// Writes are **synchronous and atomic**: `upsert`/`revoke` immediately persist
/// the full binding set via a temp-file + rename. This is the same trade-off
/// `FileSystemCorpusVersionStore` makes — simple, correct, adequate for
/// development; a production impl might use per-binding rows. The in-memory cache
/// is behind a [`RwLock`] so an admin write through a shared `Arc` handle is
/// visible to the live enforcer without a restart (hot-reload): writes take the
/// write-lock and persist; reads take the read-lock.
pub struct FileSystemAttributeAuthority {
    inner: RwLock<InMemoryAttributeAuthority>,
    path: PathBuf,
}

impl FileSystemAttributeAuthority {
    /// Open (or create) the authority at `path`. If the file exists, bindings
    /// are loaded — this is the **restart-recovery** path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuthorityError> {
        let path = path.as_ref().to_path_buf();
        let mut inner = InMemoryAttributeAuthority::new();
        if path.exists() {
            let data = std::fs::read(&path).map_err(|e| AuthorityError::Unavailable {
                detail: format!("read {}: {e}", path.display()),
            })?;
            let bindings: Vec<AttributeBinding> =
                serde_json::from_slice(&data).map_err(|e| AuthorityError::Unavailable {
                    detail: format!("deserialize {}: {e}", path.display()),
                })?;
            for b in bindings {
                inner.upsert(b);
            }
        }
        Ok(Self {
            inner: RwLock::new(inner),
            path,
        })
    }

    /// Insert or replace a binding and persist immediately (atomic rename).
    ///
    /// Takes `&self` (not `&mut self`): the in-memory cache lives behind a
    /// [`RwLock`], so a shared `Arc<FileSystemAttributeAuthority>` handle can
    /// mutate the authority — the admin-provisioning path (TD-ABAC control-plane)
    /// writes through the same instance the live enforcer reads, and the change
    /// is visible without a restart.
    pub fn upsert(&self, binding: AttributeBinding) {
        {
            let mut guard = self.inner.write().unwrap_or_else(|p| p.into_inner());
            guard.upsert(binding);
        }
        let _ = self.persist();
    }

    /// Remove a binding and persist immediately. `&self` for the same hot-reload
    /// reason as [`upsert`](Self::upsert).
    pub fn revoke(&self, subject: &SubjectId, tenant_stable_id: TenantStableId) {
        {
            let mut guard = self.inner.write().unwrap_or_else(|p| p.into_inner());
            guard.revoke(subject, tenant_stable_id);
        }
        let _ = self.persist();
    }

    /// Write the full binding set atomically (temp + rename). Re-reads the live
    /// cache under the read-lock — so it always reflects the latest committed
    /// state (no lost update under concurrent admin writes).
    fn persist(&self) -> Result<(), AuthorityError> {
        let bindings: Vec<AttributeBinding> = {
            let guard = self.inner.read().unwrap_or_else(|p| p.into_inner());
            guard.bindings().cloned().collect()
        };
        let json = serde_json::to_vec(&bindings).map_err(|e| AuthorityError::Unavailable {
            detail: format!("serialize bindings: {e}"),
        })?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| AuthorityError::Unavailable {
            detail: format!("write {}: {e}", tmp.display()),
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|e| AuthorityError::Unavailable {
            detail: format!("rename {} → {}: {e}", tmp.display(), self.path.display()),
        })?;
        Ok(())
    }
}

impl AttributeAuthority for FileSystemAttributeAuthority {
    fn resolve_effective_attributes(
        &self,
        subject: &SubjectId,
        tenant_stable_id: TenantStableId,
    ) -> Result<ResolvedSubject, AuthorityError> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .resolve_effective_attributes(subject, tenant_stable_id)
    }
}

#[cfg(test)]
mod fs_authority_tests {
    use super::*;

    #[test]
    fn bindings_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!(
            "proximadb-abac-fs-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("attrs.json");

        // Write phase: create authority, add alice, drop it.
        {
            let auth = FileSystemAttributeAuthority::open(&path).unwrap();
            auth.upsert(
                AttributeBinding::new("alice", 7).with_attr("dept", AttrValue::Str("eng".into())),
            );
            // The binding is now persisted.
            assert!(path.exists(), "persist wrote the file");
        }
        // `auth` is dropped — in-memory state is gone.

        // Read phase: reopen from the same path — alice's binding must survive.
        let auth = FileSystemAttributeAuthority::open(&path).unwrap();
        let resolved = auth
            .resolve_effective_attributes(&SubjectId("alice".into()), 7)
            .expect("alice survived the restart");
        assert_eq!(
            resolved.attributes.load_bearing("dept"),
            Some(&AttrValue::Str("eng".into())),
            "the durable authority recovered alice's dept=eng binding"
        );

        // An unbound subject still denies.
        assert!(
            auth.resolve_effective_attributes(&SubjectId("bob".into()), 7)
                .is_err()
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn revocation_persists_across_restart() {
        let dir = std::env::temp_dir().join(format!(
            "proximadb-abac-fs-revoke-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("attrs.json");

        // Add alice, then revoke.
        {
            let auth = FileSystemAttributeAuthority::open(&path).unwrap();
            auth.upsert(
                AttributeBinding::new("alice", 7).with_attr("dept", AttrValue::Str("eng".into())),
            );
            auth.revoke(&SubjectId("alice".into()), 7);
        }

        // Reopen: alice must be gone.
        let auth = FileSystemAttributeAuthority::open(&path).unwrap();
        assert!(
            auth.resolve_effective_attributes(&SubjectId("alice".into()), 7)
                .is_err(),
            "revoked binding must not survive restart"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
