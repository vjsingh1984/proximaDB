//! TD-TENANT-1 item 3: the production [`TenantStableIdResolver`] — a root-layer
//! adapter over the catalog's account registry.
//!
//! The ABAC `tenant_stable_id` (u64) IS the ADR-0083 account u32 widened (the
//! composite identity's tenant component); the catalog does NOT mint a separate
//! tenant u64. See the ADR-0083 addendum. This resolver is held by the request
//! middleware (REST `TenantExtractor::with_stable_id_resolver`) so every
//! authenticated request carries a real `tenant_stable_id` for ABAC enforcement.
//!
//! Lookup-only + authoritative: exactly ONE place derives the tenant u64 (the
//! account registry), so the request path and the (future) ABAC binding-
//! provisioning path can never disagree. `None` (unminted / brief lock
//! contention) ⇒ fail-closed deny — the id is an optimization, never a second
//! source of truth.

use std::sync::Arc;

use proximadb_tenant::TenantStableIdResolver;

use crate::catalog::CatalogManager;

/// Resolve a tenant's stable id (u64) by looking up its ADR-0083 account u32 in
/// the catalog's account registry (via [`CatalogManager::account_id_u32_lookup`]),
/// widened to u64.
#[derive(Clone)]
pub struct CatalogTenantStableIdResolver {
    catalog_manager: Arc<CatalogManager>,
}

impl CatalogTenantStableIdResolver {
    pub fn new(catalog_manager: Arc<CatalogManager>) -> Self {
        Self { catalog_manager }
    }
}

impl TenantStableIdResolver for CatalogTenantStableIdResolver {
    fn stable_id_of(&self, tenant_id: &str) -> Option<u64> {
        self.catalog_manager
            .account_id_u32_lookup(tenant_id)
            .map(|id| id as u64)
    }
}
