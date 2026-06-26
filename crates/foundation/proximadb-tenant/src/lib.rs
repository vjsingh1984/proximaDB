//! Canonical tenant-attribution primitives for ProximaDB.
//!
//! # Why this is a foundation crate (cloud-native elasticity + economics)
//!
//! In a multi-tenant, horizontally-elastic cloud database, *who owns this data*
//! is not a storage detail — it is the key that every **metering** boundary
//! (KSU storage, KRU/KIU read/compute, KOU outgress, KEU embedding) and every
//! **isolation** boundary (`DrPathBuilder` prefixes, fail-closed `TenantContext`
//! gates) attributes work against. If two layers resolve the owning tenant
//! differently, billing drifts and isolation leaks. So tenant resolution must be
//! **one** primitive, shared by all tiers — storage, services, the network
//! gates, and the billing/governance plane — rather than a private helper on any
//! one of them.
//!
//! This crate is that single source of truth. It lives at the **foundation**
//! tier (depending only on `proximadb-proto`) precisely so every higher tier can
//! depend *down* on it; nothing has to reach *up* into the services layer to ask
//! "whose tenant is this?".
//!
//! * **Elasticity:** resolution is a pure function of the collection's own
//!   metadata — no shared state, no coordinator, no I/O. Any node in an
//!   autoscaled fleet resolves the same owner from the same `Collection`, so
//!   attribution is correct without cross-node consensus.
//! * **Economics:** because it is the *only* resolver, per-tenant meters across
//!   every dimension agree, and the bill reconciles.
//! * **Isolation (fail-closed):** an unresolved tenant returns [`None`]. Callers
//!   at an isolation boundary MUST treat `None` as *deny / do not attribute to a
//!   shared tenant* — never as a default or "public" tenant. Isolation is
//!   structural, not a best-effort guess.

use proximadb_proto::proximadb_v1::Collection;

/// Resolve the owning tenant id for a collection, or [`None`] if the collection
/// is not tenant-scoped (or carries no resolvable owner).
///
/// This is the **canonical** tenant resolver — the network gates, the storage
/// flush/compaction paths, and the collection service all call this one function
/// so tag/owner precedence is identical everywhere.
///
/// # Precedence (stable contract)
///
/// 1. An explicit `tenant:<id>` tag — the authoritative marker written at
///    provisioning time. The *first* `tenant:`-prefixed tag is taken; if its id
///    is empty, resolution stops at `None` (it does not skip to a later
///    `tenant:` tag). Wins over the isolated/owner path below.
/// 2. Otherwise, if the collection is flagged `tenant_isolated:true`, the
///    collection `owner` (when non-empty) is the tenant.
/// 3. Otherwise [`None`] — not tenant-scoped. **Fail closed** at isolation
///    boundaries: do not fall back to a shared/default tenant.
pub fn tenant_id_of(collection: &Collection) -> Option<String> {
    let config = collection.config.as_ref()?;

    if let Some(tag_tenant) = config
        .tags
        .iter()
        .find_map(|tag| tag.strip_prefix("tenant:"))
        .filter(|tenant_id| !tenant_id.is_empty())
    {
        return Some(tag_tenant.to_string());
    }

    let tenant_isolated = config.tags.iter().any(|tag| tag == "tenant_isolated:true");
    if tenant_isolated {
        return config
            .owner
            .as_ref()
            .filter(|owner| !owner.is_empty())
            .cloned();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_proto::proximadb_v1::CollectionConfig;

    fn collection(tags: &[&str], owner: Option<&str>) -> Collection {
        Collection {
            config: Some(CollectionConfig {
                tags: tags.iter().map(|t| t.to_string()).collect(),
                owner: owner.map(|o| o.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn explicit_tenant_tag_wins() {
        let c = collection(&["tenant:acme", "tenant_isolated:true"], Some("someone"));
        // Tag precedence beats the isolated/owner path.
        assert_eq!(tenant_id_of(&c).as_deref(), Some("acme"));
    }

    #[test]
    fn single_tenant_tag_resolves() {
        assert_eq!(
            tenant_id_of(&collection(&["env:prod", "tenant:beta"], None)).as_deref(),
            Some("beta")
        );
    }

    #[test]
    fn first_tenant_prefixed_tag_is_authoritative_even_if_empty() {
        // Documented contract: the first `tenant:`-prefixed tag is taken; if it
        // is empty the result is None (resolution does NOT skip ahead to a later
        // `tenant:` tag). Preserved verbatim from the original service helper.
        let c = collection(&["tenant:", "tenant:beta"], None);
        assert_eq!(tenant_id_of(&c), None);
    }

    #[test]
    fn isolated_falls_back_to_owner() {
        let c = collection(&["tenant_isolated:true"], Some("owner-co"));
        assert_eq!(tenant_id_of(&c).as_deref(), Some("owner-co"));
    }

    #[test]
    fn isolated_without_owner_is_unresolved() {
        // Fail closed: isolated but no owner -> None, never a default tenant.
        let c = collection(&["tenant_isolated:true"], None);
        assert_eq!(tenant_id_of(&c), None);
        let c_empty = collection(&["tenant_isolated:true"], Some(""));
        assert_eq!(tenant_id_of(&c_empty), None);
    }

    #[test]
    fn untagged_collection_is_not_tenant_scoped() {
        // No tenant markers -> None (fail closed at isolation boundaries).
        assert_eq!(
            tenant_id_of(&collection(&["env:prod"], Some("owner"))),
            None
        );
    }

    #[test]
    fn missing_config_is_unresolved() {
        let c = Collection {
            config: None,
            ..Default::default()
        };
        assert_eq!(tenant_id_of(&c), None);
    }
}
