//! Unified Catalog System for ProximaDB
//!
//! Provides a pluggable catalog abstraction supporting multiple backends:
//! - Native: Cloud-first ProximaDB catalog with object storage
//! - AWS Glue: AWS Glue Data Catalog integration (feature-gated)
//! - Unity: Databricks Unity Catalog integration (feature-gated)
//! - Polaris: Apache Polaris (Iceberg REST Catalog) (feature-gated)
//! - Hive: Apache Hive Metastore (Thrift)
//! - Iceberg: Generic Iceberg catalogs (REST, JDBC, Hadoop)
//!
//! Design Principles:
//! - Cloud-first: Object storage as primary, local as cache
//! - Serverless-friendly: Stateless operations, external state
//! - Lakehouse-native: Iceberg/Delta/Hudi table format support
//! - Multi-tenant: Namespace isolation with RBAC

// Core traits
// `traits` was the local Catalog trait module. Option B consolidation
// (PR INT-3 line-of-work) merged its method surface into
// `proximadb_catalog::Catalog`. The module now lives only as a shim
// that re-exports the canonical types so historical import paths
// (`crate::catalog::traits::Catalog`, etc.) keep working without
// touching every importer.
pub mod traits {
    pub use proximadb_catalog::{Catalog, CatalogHealth, LakehouseExtension, TableFormat};
}

// Partition pruning for query optimization
pub use proximadb_catalog::partition_pruning;

// CATALOG_OBJECT_MODEL #3 read-port: catalog adapter for AXIS index-location resolution.
#[cfg(feature = "axis")]
pub mod index_location_resolver;

// ADR-035 / TD-SC-1: per-tenant system-catalog hot read cache (byte-bounded,
// TTL'd, corpus-version-stamped) — fronts the canonical catalog so metadata
// reads avoid the 1+N+M object-store round-trips.
pub use proximadb_catalog::syscat_cache;

// ADR-035 / TD-SC-2: on-disk warm tier between the hot cache and the canonical
// catalog (OS page cache instead of object-store round-trips on a hot miss).
pub use proximadb_catalog::syscat_warm;

// Internal schema registry (multi-model unified catalog)
pub use proximadb_catalog::internal;

// Iceberg REST catalog server — service layer translating internal ↔ Iceberg REST types.
// Moved to proximadb-catalog (unblocked by the `ObjectStoreBridge` contract descending
// into that crate); re-exported so `crate::catalog::iceberg_rest_service::*` is unchanged.
pub use proximadb_catalog::iceberg_rest_service;

// PAX segment registry — bridges write path (gRPC v2) with Iceberg REST snapshot stats.
// Moved to proximadb-catalog (unblocked by `SegmentMeta` moving into proximadb-block-format).
pub use proximadb_catalog::segment_registry;
pub use segment_registry::SegmentRegistry;

// Catalog federation (unified view across internal and external catalogs).
// Moved to proximadb-catalog (Slice 3); re-exported so `crate::catalog::federation::*`
// paths are unchanged.
pub use proximadb_catalog::federation;

// Tenant tier store — per-tenant policy + budget + feature flags (LLD §3, §4).
pub mod tenant_tier;
pub use tenant_tier::{
    BudgetDecision, CachedTenantTierStore, FeatureFlags, InMemoryTenantTierStore, TenantTierRecord,
    TenantTierStore, Tier,
};

// Recall probe gate — gating logic for the quantized route default-on (LLD §5).
pub use proximadb_catalog::recall_probe;
pub use recall_probe::{ProbeConfig, ProbeOutcome, ProbeScope, ProbeState, RecallProbeGate};

/// Soft-cap budget guard — gateway-side check returning structured rejection.
pub mod budget_guard;
pub use budget_guard::{BudgetRejection, EnforcedBudget, enforce as enforce_budget};

/// Tenant tier transition detector — classifies before/after tier
/// record pairs as upgrade / downgrade / lateral / no-change for the
/// audit log + billing reconciliation.
pub mod tier_transition;
pub use tier_transition::{
    AxisDelta, AxisDirection, TierTransitionEvent, TransitionClass, detect as detect_transition,
};

/// Tier recommendation — consumes a WorkloadMix + signal counts and
/// recommends Upgrade / Hold / Downgrade with bounded reason labels.
pub mod tier_recommendation;
pub use tier_recommendation::{
    Recommendation, RecommendationInputs, RecommendationKind, RecommendationPolicy, SignalCounts,
    recommend as recommend_tier,
};

pub use corpus_version::CorpusVersionRegistry;
/// Corpus version registry — process-wide monotonic counter per
/// (tenant, collection) for plan-cache invalidation. Catalog write
/// paths call into this when they make a schema/segment/stats
/// change visible to the planner.
// Relocated to the proximadb-catalog crate; re-exported here for source compatibility.
pub use proximadb_catalog::corpus_version;

pub use corpus_version_fs_store::FileSystemCorpusVersionStore;
/// File-backed CorpusVersionStore — first concrete durable backend
/// for single-node deployments. Other backends (catalog row, KV)
/// can implement the trait independently.
pub use proximadb_catalog::corpus_version_fs_store;

// Feature-gated catalog backends — canonical impls live in `proximadb-catalog`.
#[cfg(feature = "delta-lake")]
pub use proximadb_catalog::delta::{DeltaCatalog, DeltaCatalogConfig};
#[cfg(feature = "aws")]
pub use proximadb_catalog::glue::GlueCatalog;
#[cfg(feature = "polaris-catalog")]
pub use proximadb_catalog::polaris::PolarisCatalog;
#[cfg(feature = "unity-catalog")]
pub use proximadb_catalog::unity::UnityCatalog;

pub use self::partition_pruning::{
    PartitionInfo, PartitionPruner, PruningResult, parse_partition_path,
};
pub use self::traits::*;
pub use proximadb_catalog::cache::CatalogCache;
pub use proximadb_catalog::*;
// Explicitly re-export the local Catalog trait to disambiguate from proximadb_catalog::Catalog
// (which is also pulled in via `pub use proximadb_catalog::*`).
pub use self::traits::{Catalog, CatalogHealth};

// CatalogManager + TableOpLockRegistry + the CatalogFilesystemResolver port now
// live in `proximadb-catalog` (decomposition Slice 2). Re-exported so all
// `crate::catalog::CatalogManager` / `::TableOpLockRegistry` paths are unchanged.
// Object-store catalog URLs route through the injected resolver port (no
// catalog->storage up-edge); the root composition root injects a
// `FilesystemFactory`-backed impl.
pub use proximadb_catalog::{CatalogFilesystemResolver, CatalogManager, TableOpLockRegistry};

// Root half of the CatalogFilesystemResolver dependency inversion: a lazily-
// initialized `FilesystemFactory` wrapper. The composition root injects this
// into `CatalogManager` so object-store catalog URLs resolve without a
// catalog→storage up-edge.
pub mod filesystem_resolver;
pub use filesystem_resolver::LazyFilesystemResolver;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    // TD-108: the local `types` shim (`pub use proximadb_catalog::*`) was deleted.
    // Alias the canonical crate so the existing `types::Catalog*` test paths keep
    // resolving to exactly what the shim pointed at — no test churn.
    use proximadb_catalog as types;

    // ============================================================
    // Re-export surface guard (TD-108 regression)
    // ============================================================
    // The `crate::catalog::*` facade must keep re-exporting the canonical
    // xCatalog contract types from `proximadb-catalog`. TD-108 deleted the
    // local `types`/`cache`/backend shim modules; this guard fails to COMPILE
    // if any of those re-exports is dropped or a type is renamed/moved,
    // catching the exact regression where the facade dangled against a
    // removed module. Pure type-level references — no runtime assertions.
    #[allow(dead_code, unused_imports)]
    mod reexport_surface_guard {
        // Contract types (were `crate::catalog::types::*`).
        use crate::catalog::{
            CatalogColumn, CatalogIndex, CatalogIndexType, CatalogProjection, CatalogTableSchema,
            TableIdentifier,
        };
        use proximadb_data_model::ProximaType;
        // Cache surface (was `crate::catalog::cache::CatalogCache`).
        use crate::catalog::CatalogCache;
        // Local trait re-exported explicitly to win over `proximadb_catalog::Catalog`.
        use crate::catalog::{Catalog, CatalogHealth};

        // Force each path to be a real, named item (not just a glob hit).
        fn _surface(
            _id: TableIdentifier,
            _schema: CatalogTableSchema,
            _col: CatalogColumn,
            _dt: ProximaType,
            _idx: CatalogIndex,
            _idxt: CatalogIndexType,
            _proj: CatalogProjection,
            _cache: &CatalogCache,
            _h: CatalogHealth,
        ) {
        }
        // Trait object reachability through the facade.
        type _CatalogDyn = dyn Catalog;
    }

    // ========================
    // TableIdentifier Tests
    // ========================

    #[test]
    fn test_table_identifier_parse() {
        let id = TableIdentifier::parse("db.schema.users");
        assert_eq!(id.namespace, vec!["db", "schema"]);
        assert_eq!(id.name, "users");
    }

    #[test]
    fn test_table_identifier_simple() {
        let id = TableIdentifier::parse("users");
        assert!(id.namespace.is_empty());
        assert_eq!(id.name, "users");
    }

    #[test]
    fn test_table_identifier_to_fqn() {
        let id = TableIdentifier::new(
            vec!["db".to_string(), "schema".to_string()],
            "users".to_string(),
        );
        assert_eq!(id.to_fqn(), "db.schema.users");
    }

    #[test]
    fn test_table_identifier_single_namespace() {
        let id = TableIdentifier::parse("mydb.users");
        assert_eq!(id.namespace, vec!["mydb"]);
        assert_eq!(id.name, "users");
    }

    #[test]
    fn test_table_identifier_display() {
        let id = TableIdentifier::new(
            vec!["catalog".to_string(), "schema".to_string()],
            "table".to_string(),
        );
        assert_eq!(format!("{}", id), "catalog.schema.table");
    }

    #[test]
    fn test_table_identifier_empty_namespace_fqn() {
        let id = TableIdentifier::new(vec![], "users".to_string());
        assert_eq!(id.to_fqn(), "users");
    }

    #[test]
    fn test_table_identifier_equality() {
        let id1 = TableIdentifier::new(vec!["db".to_string()], "table".to_string());
        let id2 = TableIdentifier::new(vec!["db".to_string()], "table".to_string());
        let id3 = TableIdentifier::new(vec!["other".to_string()], "table".to_string());

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    // ========================
    // CatalogManager Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_manager_new() {
        let manager = CatalogManager::new();
        assert!(manager.list_catalogs().await.is_empty());
    }

    #[tokio::test]
    async fn test_catalog_manager_with_cache() {
        let manager = CatalogManager::with_cache(5000, 600);
        assert!(manager.list_catalogs().await.is_empty());
    }

    #[tokio::test]
    async fn test_catalog_manager_default() {
        let manager = CatalogManager::default();
        assert!(manager.list_catalogs().await.is_empty());
    }

    #[tokio::test]
    async fn test_catalog_manager_no_default_catalog() {
        let manager = CatalogManager::new();
        let result = manager.default_catalog().await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("No default catalog"));
    }

    #[tokio::test]
    async fn test_catalog_manager_get_nonexistent() {
        let manager = CatalogManager::new();
        let result = manager.get_catalog("nonexistent").await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn resolve_table_scoped_rejects_reserved_underscore_tenant() {
        // A tenant id becomes namespace[0]; a `_`-prefixed tenant would shadow a
        // reserved control-plane/system subtree. The guard validates the tenant
        // BEFORE catalog lookup, so the reserved-tenant error fires regardless of
        // whether the table exists (here: no catalog registered).
        let manager = CatalogManager::new();
        for tenant in [
            "_operator",
            "_metering",
            "_trace",
            "_branches",
            "_manifests",
        ] {
            // `(Arc<dyn Catalog>, TableIdentifier)` isn't `Debug`, so match rather
            // than `expect_err`.
            let err = match manager
                .resolve_table_scoped("some_table", Some(tenant))
                .await
            {
                Ok(_) => panic!("reserved underscore tenant {tenant} must be rejected"),
                Err(e) => e,
            };
            assert!(
                err.to_string().contains("must not begin with '_'"),
                "unexpected error for tenant {tenant}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn resolve_table_scoped_does_not_double_prefix_explicit_tenant_namespace() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = CatalogManager::new();
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let (_, table_id) = manager
            .resolve_table_scoped("native.acmecorp.analytics.events", Some("acmecorp"))
            .await
            .expect("resolve table");

        assert_eq!(
            table_id.namespace,
            vec!["acmecorp".to_string(), "analytics".to_string()]
        );
        assert_eq!(table_id.name, "events");
    }

    #[tokio::test]
    async fn test_catalog_manager_set_default_nonexistent() {
        let manager = CatalogManager::new();
        let result = manager.set_default_catalog("nonexistent").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_catalog_manager_unregister_nonexistent() {
        let manager = CatalogManager::new();
        let result = manager.unregister_catalog("nonexistent").await;
        assert!(result.is_ok());
        assert!(!result.expect("Expected Ok result")); // Returns false for nonexistent
    }

    #[tokio::test]
    async fn test_catalog_manager_cache_access() {
        let manager = CatalogManager::new();
        let cache = manager.cache();
        // Cache should be valid
        assert!(Arc::strong_count(&cache) >= 1);
    }

    // ========================
    // Factory Methods Tests (Feature Stubs)
    // ========================

    #[tokio::test]
    #[cfg(not(feature = "aws"))]
    async fn test_create_glue_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_glue_catalog("glue", "us-east-1", "123456789012")
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("aws"));
    }

    #[tokio::test]
    #[cfg(not(feature = "unity-catalog"))]
    async fn test_create_unity_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_unity_catalog(
                "unity",
                "https://example.cloud.databricks.com",
                "token",
                "main",
            )
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("unity-catalog"));
    }

    #[tokio::test]
    #[cfg(not(feature = "polaris-catalog"))]
    async fn test_create_polaris_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_polaris_catalog(
                "polaris",
                "https://polaris.example.com",
                "warehouse",
                "cred",
            )
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("polaris-catalog"));
    }

    #[tokio::test]
    #[cfg(not(feature = "delta-lake"))]
    async fn test_create_delta_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_delta_catalog("delta", "file:///tmp/delta")
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("delta-lake"));
    }

    // ========================
    // Iceberg Catalog Tests
    // ========================

    #[tokio::test]
    async fn test_create_iceberg_catalog() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_iceberg_catalog");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let result = manager
            .create_iceberg_catalog(
                "iceberg",
                "memory://",
                &format!("file://{}", temp_dir.display()),
            )
            .await;

        assert!(result.is_ok());
        let catalogs = manager.list_catalogs().await;
        assert_eq!(catalogs.len(), 1);
        assert!(catalogs.contains(&"iceberg".to_string()));

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_iceberg_catalog_operations() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_iceberg_ops");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let catalog = manager
            .create_iceberg_catalog(
                "test_iceberg",
                "memory://",
                &format!("file://{}", temp_dir.display()),
            )
            .await
            .expect("Expected catalog creation to succeed");

        assert_eq!(catalog.name(), "test_iceberg");
        assert_eq!(catalog.catalog_type(), "iceberg");

        // Health check
        let health = catalog
            .health_check()
            .await
            .expect("Expected health check to succeed");
        assert!(health.is_healthy);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Native Catalog Tests
    // ========================

    #[tokio::test]
    async fn test_create_native_catalog() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_native_catalog");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let result = manager
            .create_native_catalog("native", &format!("file://{}", temp_dir.display()))
            .await;

        assert!(result.is_ok());
        let catalogs = manager.list_catalogs().await;
        assert!(catalogs.contains(&"native".to_string()));

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_native_catalog_first_is_default() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_native_default");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("first", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(default.name(), "first");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    /// TD-CAT-1b (S0): the injected-`FileSystem` seam persists catalog metadata
    /// durably and reads it back across a fresh catalog instance. This is the
    /// exact code path `create_native_catalog` now uses for object-store URLs,
    /// proven here over a real `file://` backend (the local backend is always
    /// registered; cloud backends are feature-gated and covered by emulator
    /// tests). Before #415 + this slice, object-store catalog URLs fail-closed;
    /// this asserts the routed-through-backend write+read round-trips.
    #[tokio::test]
    async fn native_catalog_injected_filesystem_round_trips() {
        use crate::storage::persistence::filesystem::FilesystemFactory;
        use proximadb_catalog::Catalog;
        use proximadb_catalog::native::{NativeCatalog, NativeCatalogConfig};

        let temp_dir = std::env::temp_dir().join("proximadb_s0_injected_fs");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        let url = format!("file://{}", temp_dir.display());

        let factory = FilesystemFactory::create_default()
            .await
            .expect("filesystem factory");
        let fs = factory.get_filesystem(&url).expect("file:// backend");
        let cache = CatalogManager::new().cache();
        let config = NativeCatalogConfig {
            storage_url: url.clone(),
            ..Default::default()
        };

        // Write a namespace THROUGH the injected backend (fs = Some).
        let catalog = NativeCatalog::new_with_filesystem(
            "injected".to_string(),
            config.clone(),
            cache.clone(),
            Some(fs.clone()),
        )
        .await
        .expect("construct catalog with injected fs");
        catalog
            .create_namespace(&["s0_ns".to_string()], std::collections::HashMap::new())
            .await
            .expect("create namespace via injected fs");

        // A FRESH catalog over the same URL+backend must load it from disk —
        // proving the metadata was persisted durably through the backend, not
        // just held in memory.
        let reopened =
            NativeCatalog::new_with_filesystem("injected".to_string(), config, cache, Some(fs))
                .await
                .expect("reopen catalog with injected fs");
        assert!(
            reopened
                .namespace_exists(&["s0_ns".to_string()])
                .await
                .expect("namespace_exists"),
            "namespace written through the injected backend must survive a reopen (durable)"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Hive Catalog Tests
    // ========================

    #[tokio::test]
    async fn test_create_hive_catalog() {
        let manager = CatalogManager::new();

        // Hive catalog creation should work (even without a real Thrift server)
        let result = manager
            .create_hive_catalog("hive", "thrift://localhost:9083")
            .await;

        assert!(result.is_ok());
        assert!(manager.list_catalogs().await.contains(&"hive".to_string()));
    }

    // ========================
    // Multi-catalog Tests
    // ========================

    #[tokio::test]
    async fn test_multiple_catalogs() {
        let manager = CatalogManager::new();
        let temp_dir1 = std::env::temp_dir().join("proximadb_test_multi1");
        let temp_dir2 = std::env::temp_dir().join("proximadb_test_multi2");
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;

        manager
            .create_native_catalog("catalog1", &format!("file://{}", temp_dir1.display()))
            .await
            .expect("Expected catalog creation to succeed");
        manager
            .create_iceberg_catalog(
                "catalog2",
                "memory://",
                &format!("file://{}", temp_dir2.display()),
            )
            .await
            .expect("Expected catalog creation to succeed");

        let catalogs = manager.list_catalogs().await;
        assert_eq!(catalogs.len(), 2);
        assert!(catalogs.contains(&"catalog1".to_string()));
        assert!(catalogs.contains(&"catalog2".to_string()));

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;
    }

    #[tokio::test]
    async fn test_set_and_get_default_catalog() {
        let manager = CatalogManager::new();
        let temp_dir1 = std::env::temp_dir().join("proximadb_test_default1");
        let temp_dir2 = std::env::temp_dir().join("proximadb_test_default2");
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;

        manager
            .create_native_catalog("cat1", &format!("file://{}", temp_dir1.display()))
            .await
            .expect("Expected catalog creation to succeed");
        manager
            .create_native_catalog("cat2", &format!("file://{}", temp_dir2.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // First catalog should be default
        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(default.name(), "cat1");

        // Change default
        manager
            .set_default_catalog("cat2")
            .await
            .expect("Expected set_default_catalog to succeed");
        let new_default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(new_default.name(), "cat2");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;
    }

    #[tokio::test]
    async fn test_unregister_catalog() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_unregister");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("to_remove", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        assert_eq!(manager.list_catalogs().await.len(), 1);

        let removed = manager
            .unregister_catalog("to_remove")
            .await
            .expect("Expected unregister to succeed");
        assert!(removed);
        assert!(manager.list_catalogs().await.is_empty());

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_unregister_default_catalog() {
        let manager = CatalogManager::new();
        let temp_dir1 = std::env::temp_dir().join("proximadb_test_unreg_def1");
        let temp_dir2 = std::env::temp_dir().join("proximadb_test_unreg_def2");
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;

        manager
            .create_native_catalog("cat1", &format!("file://{}", temp_dir1.display()))
            .await
            .expect("Expected catalog creation to succeed");
        manager
            .create_native_catalog("cat2", &format!("file://{}", temp_dir2.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // Remove default catalog
        manager
            .unregister_catalog("cat1")
            .await
            .expect("Expected unregister to succeed");

        // cat2 should become the new default
        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(default.name(), "cat2");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;
    }

    // ========================
    // Resolve Table Tests
    // ========================

    #[tokio::test]
    async fn test_resolve_table_simple() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("default", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // Simple table name
        let (catalog, id) = manager
            .resolve_table("users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "default");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["default"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_resolve_table_with_namespace() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve_ns");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("default", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // namespace.table
        let (catalog, id) = manager
            .resolve_table("mydb.users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "default");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["mydb"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_resolve_table_fully_qualified() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve_fqn");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("mycat", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // catalog.namespace.table
        let (catalog, id) = manager
            .resolve_table("mycat.mydb.users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "mycat");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["mydb"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_resolve_table_multi_level_namespace() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve_multi");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("catalog", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // catalog.ns1.ns2.table
        let (catalog, id) = manager
            .resolve_table("catalog.db.schema.users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "catalog");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["db", "schema"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // CatalogManager Creation Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_manager_creation() {
        // Default construction
        let manager = CatalogManager::new();
        assert!(manager.list_catalogs().await.is_empty());

        // No default catalog should be set
        let default_result = manager.default_catalog().await;
        assert!(default_result.is_err());

        // With custom cache settings
        let manager_custom = CatalogManager::with_cache(50000, 600);
        assert!(manager_custom.list_catalogs().await.is_empty());

        // Default trait implementation
        let manager_default = CatalogManager::default();
        assert!(manager_default.list_catalogs().await.is_empty());

        // Cache should always be accessible
        let cache = manager.cache();
        assert!(std::sync::Arc::strong_count(&cache) >= 1);
    }

    // ========================
    // Catalog Namespace Operations Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_namespace_operations() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_ns_ops");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        // Create a native catalog so we have something to operate against
        let catalog = manager
            .create_native_catalog("test_ns", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected native catalog creation to succeed");

        // Catalog should be registered
        let catalogs = manager.list_catalogs().await;
        assert!(catalogs.contains(&"test_ns".to_string()));

        // It should be the default (first registered)
        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog");
        assert_eq!(default.name(), "test_ns");
        assert_eq!(default.catalog_type(), "native");

        // Create a namespace via the catalog trait
        let ns = catalog
            .create_namespace(&["analytics".to_string()], {
                let mut props = std::collections::HashMap::new();
                props.insert("owner".to_string(), "data_team".to_string());
                props
            })
            .await
            .expect("Expected namespace creation to succeed");

        assert_eq!(ns.levels, vec!["analytics"]);
        assert_eq!(ns.properties.get("owner"), Some(&"data_team".to_string()));

        // Check namespace exists
        let exists = catalog
            .namespace_exists(&["analytics".to_string()])
            .await
            .expect("Expected namespace_exists to succeed");
        assert!(exists);

        // List namespaces
        let namespaces = catalog
            .list_namespaces(None)
            .await
            .expect("Expected list_namespaces to succeed");
        assert!(!namespaces.is_empty());

        // Drop namespace
        let dropped = catalog
            .drop_namespace(&["analytics".to_string()], false)
            .await
            .expect("Expected drop_namespace to succeed");
        assert!(dropped);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Catalog Table Registration Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_table_registration() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_table_reg");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let catalog = manager
            .create_native_catalog("test_tbl", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected native catalog creation to succeed");

        // Create namespace first
        catalog
            .create_namespace(&["default".to_string()], std::collections::HashMap::new())
            .await
            .expect("Expected namespace creation to succeed");

        // Create a table with schema
        let table_id = TableIdentifier::new(vec!["default".to_string()], "vectors".to_string());

        let schema = types::CatalogTableSchema::new("vectors")
            .with_column(
                types::CatalogColumn::new(1, "id", proximadb_data_model::ProximaType::String)
                    .nullable(false),
            )
            .with_column({
                let mut col = types::CatalogColumn::new(
                    2,
                    "embedding",
                    proximadb_data_model::ProximaType::DenseVector {
                        element: proximadb_data_model::VectorElement::Float32,
                        dim: 0,
                    },
                );
                col.properties
                    .insert("dimension".to_string(), "768".to_string());
                col
            })
            .with_column(types::CatalogColumn::new(
                3,
                "category",
                proximadb_data_model::ProximaType::String,
            ))
            .with_primary_key(vec!["id".to_string()]);

        let created_schema = catalog
            .create_table(&table_id, schema)
            .await
            .expect("Expected table creation to succeed");

        assert_eq!(created_schema.name, "vectors");
        assert_eq!(created_schema.columns.len(), 3);
        assert_eq!(created_schema.primary_key, vec!["id"]);

        // Verify table exists
        let exists = catalog
            .table_exists(&table_id)
            .await
            .expect("Expected table_exists to succeed");
        assert!(exists);

        // List tables in namespace
        let tables = catalog
            .list_tables(&["default".to_string()])
            .await
            .expect("Expected list_tables to succeed");
        assert!(!tables.is_empty());
        assert!(tables.iter().any(|t| t.name == "vectors"));

        // Resolve the table through the manager
        let (resolved_catalog, resolved_id) = manager
            .resolve_table("test_tbl.default.vectors")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(resolved_catalog.name(), "test_tbl");
        assert_eq!(resolved_id.name, "vectors");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Catalog Schema Introspection Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_schema_introspection() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_schema_intro");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let catalog = manager
            .create_native_catalog("test_intro", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected native catalog creation to succeed");

        // Create namespace
        catalog
            .create_namespace(&["mydb".to_string()], std::collections::HashMap::new())
            .await
            .expect("Expected namespace creation to succeed");

        // Create a table with a rich schema
        let table_id = TableIdentifier::new(vec!["mydb".to_string()], "products".to_string());

        let schema = types::CatalogTableSchema::new("products")
            .with_column(
                types::CatalogColumn::new(1, "product_id", proximadb_data_model::ProximaType::Uuid)
                    .nullable(false)
                    .with_comment("Primary key UUID"),
            )
            .with_column(
                types::CatalogColumn::new(2, "name", proximadb_data_model::ProximaType::String)
                    .nullable(false),
            )
            .with_column(
                types::CatalogColumn::new(3, "price", proximadb_data_model::ProximaType::Float64)
                    .with_default("0.0"),
            )
            .with_column(types::CatalogColumn::new(
                4,
                "created_at",
                proximadb_data_model::ProximaType::TimestampTz(
                    proximadb_data_model::TimeUnit::Nanosecond,
                ),
            ))
            .with_column({
                let mut col = types::CatalogColumn::new(
                    5,
                    "embedding",
                    proximadb_data_model::ProximaType::DenseVector {
                        element: proximadb_data_model::VectorElement::Float32,
                        dim: 0,
                    },
                );
                col.properties
                    .insert("dimension".to_string(), "768".to_string());
                col
            })
            .with_primary_key(vec!["product_id".to_string()])
            .with_index(types::CatalogIndex::new(
                "idx_name",
                vec!["name".to_string()],
                types::CatalogIndexType::BTree,
            ));

        catalog
            .create_table(&table_id, schema)
            .await
            .expect("Expected table creation to succeed");

        // Retrieve the schema and introspect it
        let retrieved = catalog
            .get_table(&table_id)
            .await
            .expect("Expected get_table to succeed");

        assert_eq!(retrieved.name, "products");
        assert_eq!(retrieved.columns.len(), 5);
        assert_eq!(retrieved.schema_version, 1);

        // Verify individual columns
        let id_col = retrieved
            .columns
            .iter()
            .find(|c| c.name == "product_id")
            .expect("product_id column should exist");
        assert!(!id_col.nullable);
        assert_eq!(id_col.data_type, proximadb_data_model::ProximaType::Uuid);
        assert_eq!(id_col.comment.as_deref(), Some("Primary key UUID"));

        let price_col = retrieved
            .columns
            .iter()
            .find(|c| c.name == "price")
            .expect("price column should exist");
        assert_eq!(
            price_col.data_type,
            proximadb_data_model::ProximaType::Float64
        );
        assert_eq!(price_col.default_value.as_deref(), Some("0.0"));
        assert!(price_col.nullable); // Default is true

        let embed_col = retrieved
            .columns
            .iter()
            .find(|c| c.name == "embedding")
            .expect("embedding column should exist");
        assert_eq!(
            embed_col.data_type,
            proximadb_data_model::ProximaType::DenseVector {
                element: proximadb_data_model::VectorElement::Float32,
                dim: 0,
            }
        );

        // Verify primary key
        assert_eq!(retrieved.primary_key, vec!["product_id"]);

        // Verify index
        assert!(!retrieved.indexes.is_empty());
        let idx = &retrieved.indexes[0];
        assert_eq!(idx.name, "idx_name");
        assert_eq!(idx.columns, vec!["name"]);
        assert_eq!(idx.index_type, types::CatalogIndexType::BTree);

        // Check schema version
        let version = catalog
            .get_schema_version(&table_id)
            .await
            .expect("Expected get_schema_version to succeed");
        assert_eq!(version, 1);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
