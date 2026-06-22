// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Collection Service - Core Business Logic and Collection Management
//!
//! This service is the central orchestrator for all collection-related operations in ProximaDB.
//! It provides a unified interface that abstracts storage details from the API layer while
//! managing collection lifecycle, metadata, and coordination with storage engines.
//!
//! ## Role in ProximaDB Architecture
//!
//! The CollectionService sits at the heart of the service layer:
//! ```text
//! API Handlers → CollectionService → Storage/Index/WAL
//!                     ↓
//!              Metadata Backend
//! ```
//!
//! ## Key Responsibilities
//!
//! 1. **Collection Lifecycle Management**:
//!    - Create, update, delete collections
//!    - UUID generation and management
//!    - Schema validation and evolution
//!
//! 2. **Storage Coordination**:
//!    - Storage engine selection based on workload
//!    - Multi-disk path assignment
//!    - Collection-to-storage affinity
//!
//! 3. **Metadata Management**:
//!    - Persistent metadata storage
//!    - Configuration caching with DashMap
//!    - Index configuration management
//!
//! 4. **Business Logic**:
//!    - Validation of collection parameters
//!    - Default value resolution
//!    - Compression strategy selection
//!    - Quantization configuration
//!
//! ## Design Principles
//!
//! - **Proto-First**: Uses native protocol buffer types (Collection, CollectionConfig)
//! - **Zero-Copy**: Minimal allocations and translations
//! - **UUID-Based**: All storage paths use UUIDs for uniqueness
//! - **Atomic Operations**: All operations are atomic with proper rollback
//! - **Cache-Friendly**: DashMap for lock-free concurrent access to metadata
//!
//! ## Integration Points
//!
//! - **Upstream**: Called by `UnifiedHandlers` for all collection operations
//! - **Downstream**:
//!   - `UniversalMetadataBackend` for metadata persistence
//!   - `FilesystemFactory` for storage access
//!   - Storage engines via `InternalCollectionProvider` trait
//!
//! ## Performance Optimizations
//!
//! - **Lock-Free Caching**: DashMap eliminates lock contention
//! - **Lazy Loading**: Metadata loaded on-demand
//! - **Batch Operations**: Support for bulk collection operations
//! - **Smart Defaults**: Automatic selection of optimal configurations

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

// Using String directly instead of String alias for proto-first architecture
use crate::catalog::{
    CatalogColumn, CatalogIndex, CatalogIndexType, CatalogManager, CatalogPhysicalFormat,
    CatalogProjection, CatalogProjectionKind, CatalogStorageLayout, CatalogStorageLayoutKind,
    CatalogTableSchema, ProjectionFreshness, TableIdentifier,
};
use crate::core::config::StorageConfig;
use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, CollectionStats, FilterableColumnSpec, FilterableDataType,
    IndexConfig, IndexingAlgorithm, StorageAssignment, StorageEngine,
};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::InternalCollectionProvider;
use proximadb_data_model::ProximaType;
use proximadb_storage_common::storage_path::StoragePath;

// Proto-first architecture - use crate::proto::proximadb_v1::Collection directly

// Local types to replace assignment service
/// Storage component type used for multi-disk path assignment.
#[derive(Debug, Clone)]
enum StorageComponentType {
    /// Write-ahead log component
    Wal,
    /// Data storage component
    Storage,
    /// Index storage component
    Index,
}

/// Collection service for unified business logic with multi-disk coordination
pub struct CollectionService {
    /// Backend provider for collection metadata persistence
    metadata_backend: Arc<dyn InternalCollectionProvider>,
    /// Factory for creating filesystem instances per collection path
    filesystem_factory: Arc<FilesystemFactory>,
    /// Cache for IndexConfig to avoid repeated deserialization
    /// Using dashmap for lock-free concurrent access
    index_config_cache: Arc<dashmap::DashMap<String, crate::index::config::RuntimeIndexConfig>>,
    /// Global storage configuration for engine and WAL settings
    storage_config: StorageConfig,
    /// Optional xCatalog manager. When present, collection lifecycle metadata is
    /// mirrored through xCatalog and the legacy backend remains a storage-compatibility cache.
    catalog_manager: Option<Arc<CatalogManager>>,

    // NEW: Multi-tenant integration
    /// Optional tenant manager for multi-tenant isolation
    tenant_manager: Option<Arc<crate::storage::tenant::TenantManager>>,
    /// Optional RBAC enforcer for role-based access control
    rbac_enforcer: Option<Arc<crate::storage::tenant::EnhancedRBACManager>>,

    /// Per-collection TurboQuant store registry (Phase P — Quantization
    /// Trait Convergence Plan). When present, `create_collection` with
    /// `enable_turboquant=true` registers the per-collection store
    /// immediately (no first-search latency hit) via
    /// `registry.get_or_create(...)`. When absent (default test paths
    /// + non-TurboQuant deployments), the create-time block falls back
    /// to logging-only behavior so existing fixtures keep working.
    ///
    /// Same `Arc<dyn>` instance lives on `SharedServices.turboquant_registry`
    /// — Phase P's hoist in `SharedServices::new` ensures the create-time
    /// wire and the boot-time hydration share one map.
    #[cfg(feature = "experimental-turboquant")]
    turboquant_registry: Option<
        Arc<dyn crate::compute::quantization::turboquant_store_registry::TurboQuantStoreRegistry>,
    >,
}

impl CollectionService {
    /// Create new collection service with multi-disk coordination
    pub async fn new(
        metadata_backend: Arc<dyn InternalCollectionProvider>,
        storage_config: StorageConfig,
    ) -> Result<Self> {
        // Create filesystem factory with proper config from storage_config
        let fs_config = crate::storage::persistence::filesystem::FilesystemConfig {
            default_fs: Some(storage_config.metadata_url.clone()),
            local: None,
            global_options: Default::default(),
            auth_config: None,
            performance_config: Default::default(),
            scheme_mapping: Default::default(),
        };

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(fs_config)
                .await
                .context("Failed to initialize filesystem factory")?,
        );

        Ok(Self {
            metadata_backend,
            filesystem_factory,
            index_config_cache: Arc::new(dashmap::DashMap::new()),
            storage_config,
            catalog_manager: None,
            tenant_manager: None, // Will be set via with_tenant_manager()
            rbac_enforcer: None,  // Will be set via with_rbac_enforcer()
            #[cfg(feature = "experimental-turboquant")]
            turboquant_registry: None, // Will be set via with_turboquant_registry()
        })
    }

    /// Attach a TurboQuant store registry (Phase P — Quantization Trait
    /// Convergence Plan). When set, `create_collection` with
    /// `enable_turboquant=true` registers the per-collection store
    /// immediately via `registry.get_or_create(...)`. Mirrors the
    /// `with_catalog_manager` pattern below.
    ///
    /// Production wiring: `SharedServices::new` hoists the registry
    /// construction (Phase P Site 2) and threads the same `Arc<dyn>`
    /// instance through here. Sharing one `Arc` means create-time
    /// registrations land in the same map the boot-time hydration loop
    /// populates.
    #[cfg(feature = "experimental-turboquant")]
    pub fn with_turboquant_registry(
        mut self,
        registry: Arc<
            dyn crate::compute::quantization::turboquant_store_registry::TurboQuantStoreRegistry,
        >,
    ) -> Self {
        self.turboquant_registry = Some(registry);
        self
    }

    /// Attach the shared xCatalog manager.
    ///
    /// During migration, xCatalog is the lifecycle metadata authority when configured while the
    /// legacy metadata backend is kept in sync for storage-engine callers that still read it.
    pub fn with_catalog_manager(mut self, catalog_manager: Arc<CatalogManager>) -> Self {
        self.catalog_manager = Some(catalog_manager);
        self
    }

    /// Set tenant manager for multi-tenant support
    pub fn with_tenant_manager(
        mut self,
        tenant_manager: Arc<crate::storage::tenant::TenantManager>,
    ) -> Self {
        self.tenant_manager = Some(tenant_manager);
        self
    }

    /// Set RBAC enforcer for permission validation
    pub fn with_rbac_enforcer(
        mut self,
        rbac_enforcer: Arc<crate::storage::tenant::EnhancedRBACManager>,
    ) -> Self {
        self.rbac_enforcer = Some(rbac_enforcer);
        self
    }

    /// Get storage configuration
    ///
    /// Returns the storage configuration for accessing storage locations.
    /// Used by Arrow Flight service to find .arrow files.
    pub fn storage_config(&self) -> &StorageConfig {
        &self.storage_config
    }

    /// Returns `true` if multi-tenant mode is enabled (a tenant manager is configured).
    pub fn multi_tenant_enabled(&self) -> bool {
        self.tenant_manager.is_some()
    }

    /// Load tenant context for the given tenant ID.
    ///
    /// Returns `Ok(None)` when multi-tenant mode is disabled.
    /// Returns an error if the tenant ID is missing or the tenant is not found.
    pub fn load_tenant_context(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<Option<crate::storage::tenant::TenantContext>> {
        match &self.tenant_manager {
            Some(tenant_manager) => {
                let tenant_id = tenant_id
                    .map(str::trim)
                    .filter(|tenant_id| !tenant_id.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Tenant context is required for this operation")
                    })?;
                let tenant_ctx = tenant_manager
                    .get_tenant(tenant_id)
                    .with_context(|| format!("Tenant '{}' not found", tenant_id))?;
                Ok(Some(tenant_ctx))
            }
            None => Ok(None),
        }
    }

    fn collection_tenant_id(collection: &Collection) -> Option<String> {
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

    async fn count_tenant_collections(&self, tenant_id: &str) -> Result<usize> {
        Ok(self
            .list_collections()
            .await?
            .into_iter()
            .filter(|collection| {
                Self::collection_tenant_id(collection).as_deref() == Some(tenant_id)
            })
            .count())
    }

    async fn validate_tenant_collection_access(
        &self,
        collection_identifier: &str,
        tenant_ctx: &crate::storage::tenant::TenantContext,
    ) -> Result<Option<Collection>> {
        if let Some(ref tenant_manager) = self.tenant_manager
            && !tenant_manager.is_tenant_active(&tenant_ctx.tenant_id)
        {
            warn!(
                "🚨 Tenant '{}' is not active; denying access to collection '{}'",
                tenant_ctx.tenant_id, collection_identifier
            );
            return Ok(None);
        }

        let collection = self.collection(collection_identifier).await?;

        let Some(collection) = collection else {
            return Ok(None);
        };

        let Some(collection_tenant) = Self::collection_tenant_id(&collection) else {
            warn!(
                "🚨 Collection '{}' is missing tenant metadata; denying tenant-scoped access",
                collection_identifier
            );
            return Ok(None);
        };

        if collection_tenant != tenant_ctx.tenant_id {
            warn!(
                "🚨 Cross-tenant access attempt blocked: user tenant {} tried to access collection owned by tenant {}",
                tenant_ctx.tenant_id, collection_tenant
            );
            return Ok(None);
        }

        if self.rbac_enforcer.is_some() {
            debug!(
                "RBAC enforcer configured for tenant '{}', but collection service access checks still need user context wiring",
                tenant_ctx.tenant_id
            );
        }

        Ok(Some(collection))
    }

    /// Create collection - single method for all handlers (REST, gRPC, etc)
    /// Takes native types directly, no proto/avro conversions needed
    /// NOW WITH MULTI-TENANT SUPPORT
    pub async fn create_collection(
        &self,
        config: &crate::proto::proximadb_v1::CollectionConfig,
    ) -> Result<CollectionServiceResponse> {
        self.create_collection_with_tenant_context(config, None)
            .await
    }

    /// Get collection with tenant validation
    pub async fn get_collection_with_tenant_context(
        &self,
        collection_name: &str,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<Option<crate::proto::proximadb_v1::Collection>> {
        if let Some(tenant_ctx) = tenant_context.filter(|_| self.tenant_manager.is_some()) {
            let collection = self
                .validate_tenant_collection_access(collection_name, tenant_ctx)
                .await?;

            if collection.is_some() {
                debug!(
                    "✅ Tenant ownership validation passed for collection access: tenant={}, collection={}",
                    tenant_ctx.tenant_id, collection_name
                );
            }

            return Ok(collection);
        }

        // Proceed with normal collection retrieval
        self.collection(collection_name).await
    }

    /// List all collections, filtered to the given tenant context if multi-tenant mode is active.
    pub async fn list_collections_with_tenant_context(
        &self,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<Vec<Collection>> {
        let collections = self.list_collections().await?;

        if let Some(tenant_ctx) = tenant_context.filter(|_| self.tenant_manager.is_some()) {
            if let Some(ref tenant_manager) = self.tenant_manager
                && !tenant_manager.is_tenant_active(&tenant_ctx.tenant_id)
            {
                warn!(
                    "🚨 Tenant '{}' is not active; returning empty collection list",
                    tenant_ctx.tenant_id
                );
                return Ok(Vec::new());
            }

            let filtered = collections
                .into_iter()
                .filter(|collection| {
                    Self::collection_tenant_id(collection).as_deref() == Some(&tenant_ctx.tenant_id)
                })
                .collect();
            return Ok(filtered);
        }

        Ok(collections)
    }

    /// Delete collection with tenant validation
    pub async fn delete_collection_with_tenant_context(
        &self,
        collection_name: &str,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<CollectionServiceResponse> {
        debug!("🗑️ Deleting collection: {}", collection_name);

        if let Some(tenant_ctx) = tenant_context.filter(|_| self.tenant_manager.is_some()) {
            let collection = self
                .validate_tenant_collection_access(collection_name, tenant_ctx)
                .await?;

            if collection.is_none() {
                return Ok(CollectionServiceResponse::error(
                    format!(
                        "TENANT_ACCESS_DENIED: collection {} is not accessible to tenant {}",
                        collection_name, tenant_ctx.tenant_id
                    ),
                    0,
                ));
            }

            debug!(
                "✅ Tenant ownership validation passed for collection deletion: tenant={}, collection={}",
                tenant_ctx.tenant_id, collection_name
            );
        }

        let response = self.delete_collection(collection_name).await?;

        // Bump the corpus_version for (tenant, collection) so the
        // process-wide PlanCache invalidates on the next planner
        // lookup. Deleting a collection definitionally invalidates
        // any cached plans for it. Only fires when we have a tenant
        // context — anonymous deletions can't be keyed.
        if let Some(tenant_ctx) = tenant_context
            && response.success
        {
            let version = crate::catalog::CorpusVersionRegistry::global()
                .bump(&tenant_ctx.tenant_id, collection_name)
                .await;
            debug!(
                "🔄 corpus_version bumped after delete: tenant={} collection={} version={}",
                tenant_ctx.tenant_id, collection_name, version
            );
        }

        Ok(response)
    }

    /// Create collection with tenant context validation
    pub async fn create_collection_with_tenant_context(
        &self,
        config: &crate::proto::proximadb_v1::CollectionConfig,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<CollectionServiceResponse> {
        debug!(
            "🆕 Creating collection: {} with distance_metric={:?}",
            config.name, config.distance_metric
        );
        let start_time = std::time::Instant::now();

        if let Some(ref tenant_manager) = self.tenant_manager {
            let Some(tenant_ctx) = tenant_context else {
                return Ok(CollectionServiceResponse::error(
                    "TENANT_CONTEXT_REQUIRED: tenant context is required when multi-tenant mode is enabled".to_string(),
                    start_time.elapsed().as_micros() as i64,
                ));
            };

            if !tenant_manager.is_tenant_active(&tenant_ctx.tenant_id) {
                return Ok(CollectionServiceResponse::error(
                    format!(
                        "TENANT_INACTIVE: tenant {} is not active",
                        tenant_ctx.tenant_id
                    ),
                    start_time.elapsed().as_micros() as i64,
                ));
            }

            let tenant_collection_count =
                self.count_tenant_collections(&tenant_ctx.tenant_id).await?;
            let tenant_limit = tenant_ctx.resource_limits.max_collections as usize;

            if tenant_collection_count >= tenant_limit {
                return Ok(CollectionServiceResponse::error(
                    format!(
                        "TENANT_COLLECTION_LIMIT_EXCEEDED: tenant {} has reached its collection limit ({})",
                        tenant_ctx.tenant_id, tenant_limit
                    ),
                    start_time.elapsed().as_micros() as i64,
                ));
            }

            if self.rbac_enforcer.is_some() {
                debug!(
                    "RBAC enforcer configured for tenant '{}', but collection creation checks still need user context wiring",
                    tenant_ctx.tenant_id
                );
            }

            debug!(
                "✅ Tenant validation passed for collection creation: tenant={}, existing_collections={}, limit={}",
                tenant_ctx.tenant_id, tenant_collection_count, tenant_limit
            );
        }

        let mut enriched_config = config.clone();

        // Persist an explicit default metric so every downstream subsystem
        // sees the same collection semantics instead of applying its own fallback.
        let resolved_distance_metric = enriched_config
            .distance_metric
            .and_then(|metric| crate::proto::proximadb_v1::DistanceMetric::try_from(metric).ok())
            .filter(|metric| *metric != crate::proto::proximadb_v1::DistanceMetric::Unspecified)
            .unwrap_or(crate::proto::proximadb_v1::DistanceMetric::Cosine);
        enriched_config.distance_metric = Some(resolved_distance_metric as i32);

        // Heuristic engine routing: collections that don't pin a
        // storage_engine fall through to the rules in
        // crate::services::collection::engine_selector. Vector
        // collections with neither an index nor quantization land on
        // HELIX (Hilbert-sorted blocks → usable recall without an
        // external index); everything else stays on SST. Caller-pinned
        // engine choices are passed through untouched.
        let (selected_engine, selection_reason) =
            crate::services::collection::engine_selector::infer_storage_engine(&enriched_config);
        let previous_engine_field = enriched_config.storage_engine;
        enriched_config.storage_engine = Some(selected_engine as i32);
        tracing::info!(
            target: "collection.engine_selector",
            collection = %enriched_config.name,
            chosen_engine = ?selected_engine,
            reason = selection_reason,
            previous_field = ?previous_engine_field,
            dimension = enriched_config.dimension,
            has_index = !enriched_config.index_configs.is_empty(),
            has_quantization = enriched_config
                .quantization
                .as_ref()
                .and_then(|q| q.enabled)
                .unwrap_or(false),
            "auto-selected storage engine"
        );

        // Recall-target advisor wiring: when the caller asked for a
        // specific recall (via a `recall_target:<f32>` tag), invoke
        // the algorithm-agnostic advisor. The selector picks HNSW
        // vs IVF based on declared budgets (max_memory_mb,
        // max_query_latency_ms) and sizes the chosen algorithm's
        // params; results are stamped into the matching IndexConfig.
        // See crate::services::collection::recall_target for the
        // parse + apply contract.
        if let Some(recall_target) =
            crate::services::collection::recall_target::parse_recall_target(&enriched_config)
        {
            let applied = crate::services::collection::recall_target::apply_advisor_to_indexes(
                &mut enriched_config,
                recall_target,
            );
            for advice in &applied {
                tracing::info!(
                    target: "collection.recall_target",
                    collection = %enriched_config.name,
                    index = %advice.index_name,
                    recall_target = recall_target,
                    algorithm = %advice.output.kind.label(),
                    clamped_by_budget = advice.output.clamped_by_budget,
                    projected_recall = ?advice.output.projected_recall,
                    estimated_memory_mb = advice.output.estimated_memory_mb,
                    estimated_per_query_work = advice.output.estimated_per_query_work,
                    rationale = %advice.output.rationale,
                    "auto-sized index from recall_target"
                );
            }
        }

        // Resolve compression and storage configuration

        // NEW: Add tenant metadata to collection if tenant context is provided
        if let Some(tenant_ctx) = tenant_context {
            enriched_config
                .tags
                .retain(|tag| !tag.starts_with("tenant:") && tag != "tenant_isolated:true");

            // Add tenant ID to collection tags for tenant isolation (metadata field doesn't exist)
            enriched_config
                .tags
                .push(format!("tenant:{}", tenant_ctx.tenant_id));
            enriched_config
                .tags
                .push("tenant_isolated:true".to_string());
            enriched_config
                .tags
                .push(format!("created_at:{}", chrono::Utc::now().to_rfc3339()));

            // Set owner field if available
            enriched_config.owner = Some(tenant_ctx.tenant_id.clone());

            debug!(
                "✅ Added tenant metadata to collection: tenant_id={}",
                tenant_ctx.tenant_id
            );
        }

        // Ensure storage_config exists and set compression within it
        if enriched_config.storage_config.is_none() {
            enriched_config.storage_config = Some(crate::proto::proximadb_v1::StorageConfig {
                ..Default::default()
            });
        }

        // Resolve compression within storage_config
        if let Some(ref mut storage_cfg) = enriched_config.storage_config {
            let resolved_compression = self.resolve_compression_config(
                None, // No existing compression config to resolve from
                config.storage_engine.unwrap_or(StorageEngine::Sst as i32),
            );
            if let Some(compression_config) = resolved_compression {
                storage_cfg.compression = Some(compression_config.algorithm);
            }
        }

        // Add default quantization configuration if not provided
        // Use smart defaults based on vector dimension for optimal performance
        if enriched_config.quantization.is_none() {
            use crate::compute::quantization::QuantizationSmartDefaults;

            match QuantizationSmartDefaults::generate_for_dimension(config.dimension as usize) {
                Ok(smart_config) => {
                    enriched_config.quantization = Some(smart_config);
                    info!(
                        "🧠 Generated smart quantization defaults for collection '{}' (dimension: {})",
                        config.name, config.dimension
                    );
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to generate smart defaults, using fallback: {}",
                        e
                    );
                    // Fallback to simple default
                    enriched_config.quantization = Some(crate::proto::proximadb_v1::QuantizationConfig {
                        enabled: Some(true),
                        strategy: Some(crate::proto::proximadb_v1::quantization_config::Strategy::SmartDefaults as i32),
                        custom_levels: vec![],
                        enable_progressive_search: Some(true),
                        binary_filter_selectivity: Some(0.3),
                        int8_ranking_selectivity: Some(0.1),
                        pq_ranking_selectivity: Some(0.05),
                        training_sample_size: Some(10000),
                        quality_threshold: Some(0.95),
                        enable_adaptive_training: Some(true),
                        optimize_for_storage: Some(false),
                        optimize_for_memory: Some(false),
                        enable_simd_acceleration: Some(true),
                        // NEW: Direct quantization type enables
                        enable_binary: Some(true),
                        enable_int8: Some(true),
                        enable_pq: Some(true),
                        // Product Quantization specific settings
                        pq_segments: Some(8),
                        pq_bits: Some(8),
                        pq_codebooks: Some(0),
                        // Thresholds for progressive search
                        binary_threshold: Some(0.3),
                        int8_threshold: Some(0.1),
                        pq_threshold: Some(0.05),
                        enable_turboquant: Some(false),
                    });
                }
            }
        }

        // Phase N (opt-in plumbing) + Phase P (create-time register) —
        // Quantization Trait Convergence Plan. When the SDK / handler
        // sets `quantization.enable_turboquant = true`:
        //
        // 1. On `cfg(experimental-turboquant)` builds with a registry
        //    attached: call `registry.get_or_create(...)` so the
        //    per-collection TurboQuant store is registered NOW. The first
        //    search reaches the kernel instead of a silent full-precision
        //    fallback. Failures are logged but DO NOT abort collection
        //    creation (Phase O "log + continue" pattern — registry
        //    transient errors must not block the catalog write).
        // 2. On `cfg(experimental-turboquant)` builds without a registry
        //    (test fixtures + paths constructed via `CollectionService::new`
        //    without the builder): emit the Phase N structured event so
        //    operator dashboards still see the intent.
        // 3. On builds without the feature: emit a `warn!` so silent
        //    drops never go unnoticed.
        //
        // Defaults surfaced (per ADR-021 §"Authority mode"):
        //   - `bit_width = 4`
        //   - `calibration_mode = tq_plus`
        //   - `rotation_seed = derive_rotation_seed(&collection_name)` —
        //     same FNV-1a hash every other Phase-A→O surface uses, so
        //     the runtime store, the EXPLAIN payload, and any future
        //     catalog row all agree on the per-collection seed.
        let opt_in = enriched_config
            .quantization
            .as_ref()
            .and_then(|q| q.enable_turboquant)
            .unwrap_or(false);
        if opt_in {
            #[cfg(feature = "experimental-turboquant")]
            {
                use proximadb_quantization_types::CalibrationMode;
                let seed = proximadb_quantization_types::derive_rotation_seed(&config.name);
                let bit_width: u8 = 4;
                if let Some(registry) = &self.turboquant_registry {
                    match registry
                        .get_or_create(
                            &config.name,
                            config.dimension as usize,
                            bit_width,
                            CalibrationMode::TqPlus,
                            seed,
                        )
                        .await
                    {
                        Ok(_store) => {
                            tracing::info!(
                                target: "proximadb::turboquant::opt_in",
                                collection = %config.name,
                                bit_width,
                                calibration_mode = "tq_plus",
                                rotation_seed = format!("{:#x}", seed),
                                "Phase P opt-in: TurboQuant store registered for new collection",
                            );
                        }
                        Err(e) => {
                            // Log + continue. Collection-create must NOT
                            // fail just because the registry hit an
                            // error — boot-time hydration recovers on
                            // next restart, and the next search retries
                            // `get_or_create` lazily.
                            tracing::warn!(
                                target: "proximadb::turboquant::opt_in",
                                collection = %config.name,
                                error = %e,
                                "Phase P opt-in: get_or_create failed; collection will fall \
                                 back to full-precision scoring until next boot",
                            );
                        }
                    }
                } else {
                    // Registry not attached (test path). Keep the Phase
                    // N logging-only behavior so existing fixtures don't
                    // break — the operator-visible intent still surfaces.
                    tracing::info!(
                        target: "proximadb::turboquant::opt_in",
                        collection = %config.name,
                        bit_width,
                        calibration_mode = "tq_plus",
                        rotation_seed = format!("{:#x}", seed),
                        "Phase N opt-in (no registry attached): TurboQuant registered for collection",
                    );
                }
            }
            #[cfg(not(feature = "experimental-turboquant"))]
            {
                tracing::warn!(
                    collection = %config.name,
                    "Collection requested enable_turboquant=true but the server build \
                     does not have the `experimental-turboquant` feature enabled; \
                     opt-in is silently dropped",
                );
            }
        }

        if enriched_config.index_configs.is_empty() {
            info!(
                "📊 Collection '{}' created without an ANN index; exact/brute-force retrieval remains the default until indexes are explicitly configured",
                config.name
            );
        }

        // Validate compression algorithm is supported by the storage engine
        // SDK defines compression config in collection metadata and it drives datablock compression
        if let Some(ref storage_cfg) = enriched_config.storage_config {
            // storage_cfg.compression is i32 in proto v1, check if it's set
            if storage_cfg.compression.unwrap_or(0) != 0 {
                use crate::proto::proximadb_v1::CompressionAlgorithm;
                use crate::storage::engine_capabilities::EngineCapabilities;

                // Convert engine type to enum
                let engine = EngineCapabilities::engine_from_int(
                    config.storage_engine.unwrap_or(StorageEngine::Sst as i32),
                );

                // Try to convert compression algorithm from i32
                if let Ok(algorithm) =
                    CompressionAlgorithm::try_from(storage_cfg.compression.unwrap_or(0))
                {
                    if !EngineCapabilities::is_compression_supported(engine, algorithm) {
                        let engine_name = EngineCapabilities::get_engine_name(engine);
                        let unsupported =
                            EngineCapabilities::get_unsupported_compression_algorithms(engine);
                        return Ok(CollectionServiceResponse::error(
                            format!(
                                "UNSUPPORTED_COMPRESSION: Compression algorithm {:?} is not supported by {} engine. Unsupported algorithms: {:?}",
                                algorithm, engine_name, unsupported
                            ),
                            start_time.elapsed().as_micros() as i64,
                        ));
                    }
                } else {
                    return Ok(CollectionServiceResponse::error(
                        format!(
                            "INVALID_COMPRESSION: Invalid compression algorithm: {:?}",
                            storage_cfg.compression
                        ),
                        start_time.elapsed().as_micros() as i64,
                    ));
                }
            }
        }

        // Input validation
        if config.name.is_empty() {
            return Ok(CollectionServiceResponse::error(
                "INVALID_NAME: Collection name cannot be empty".to_string(),
                start_time.elapsed().as_micros() as i64,
            ));
        }

        // No artificial minimum name length. SQL/ANSI identifiers — and hence
        // relational tables created over pgwire (e.g. TPC-H `part`, `orders`,
        // `region`) — are routinely short. Name shape is validated elsewhere
        // (CollectionNameValidator: non-empty, valid pattern, not reserved); a
        // length floor here only blocked legitimate short table names.

        if config.dimension == 0 || config.dimension > 1_000_000 {
            return Ok(CollectionServiceResponse::error(
                "INVALID_DIMENSION: Invalid dimension: must be between 1 and 1,000,000".to_string(),
                start_time.elapsed().as_micros() as i64,
            ));
        }

        // Validate quantization configuration
        if let Some(quant_config) = &enriched_config.quantization
            && quant_config.enabled.unwrap_or(false)
        {
            info!(
                "⚠️ Collection '{}' has quantization enabled. All vectors MUST have unique IDs for tracking quantized representations",
                config.name
            );
            // Note: We don't fail here, but log a warning. The actual validation happens during insert
            // This allows collections to be created with quantization enabled, but enforces IDs at insert time
        }

        if self.collection(&config.name).await?.is_some() {
            return Ok(CollectionServiceResponse {
                success: false,
                collection: None,
                storage_path: None,
                error_code: Some("COLLECTION_EXISTS".to_string()),
                processing_time_us: start_time.elapsed().as_micros() as i64,
            });
        }

        // Create proto collection directly - no Avro conversion needed!
        // Collection IDs are UUIDs. Legacy base62/time IDs remain resolvable from older metadata,
        // but new catalog assets use opaque UUID identity and keep names as stable logical aliases.
        let uuid = self.generate_unique_collection_id().await?;
        let now = chrono::Utc::now().timestamp_micros();

        // Get storage location - use provided or pick randomly from config
        let base_location = if let Some(ref storage_config) = enriched_config.storage_config {
            if storage_config
                .storage_path
                .as_ref()
                .is_some_and(|p| !p.is_empty())
            {
                // User provided storage location
                storage_config.storage_path.clone().unwrap_or_default()
            } else {
                // Pick randomly from configured locations
                use rand::seq::SliceRandom;
                self.storage_config
                    .storage_locations
                    .choose(&mut rand::thread_rng())
                    .ok_or_else(|| anyhow::anyhow!("No storage locations configured"))?
                    .url
                    .clone()
            }
        } else {
            // Pick randomly from configured locations
            use rand::seq::SliceRandom;
            self.storage_config
                .storage_locations
                .choose(&mut rand::thread_rng())
                .ok_or_else(|| anyhow::anyhow!("No storage locations configured"))?
                .url
                .clone()
        };

        // Create storage directories (tenant-isolated if multi-tenant mode)
        let tenant_id = tenant_context.map(|ctx| ctx.tenant_id.as_str());
        let _storage_created = self
            .create_storage_directories(&base_location, &enriched_config.name, &uuid, tenant_id)
            .await
            .context("Failed to create storage directories")?;

        // Build tenant-prefixed base location for storage assignment
        let tenant_base_location = match tenant_id {
            Some(tid) => StoragePath::tenant_root_path(&base_location, tid),
            None => base_location.clone(),
        };

        // Create proto collection with stats and storage assignment
        let proto_collection = Collection {
            id: uuid.clone(),
            config: Some(enriched_config.clone()), // Use enriched config with compression
            stats: Some(crate::proto::proximadb_v1::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: now,
            updated_at: now,
            storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
                primary_path: tenant_base_location.clone(),
                backup_paths: vec![],
                engine: config.storage_engine.unwrap_or(StorageEngine::Sst as i32),
                engine_config: std::collections::HashMap::new(),
                base_location: tenant_base_location.clone(), // Tenant-prefixed path
                assigned_at: chrono::Utc::now().timestamp_micros(),
            }),
        };

        if let Err(e) = self
            .upsert_collection_catalog_asset(&proto_collection)
            .await
        {
            return Ok(CollectionServiceResponse::error(
                format!("CATALOG_CREATE_FAILED: {}", e),
                start_time.elapsed().as_micros() as i64,
            ));
        }

        // Store proto collection using protobuf serialization (zero-copy). This remains as a
        // compatibility cache for storage engines until all callers read xCatalog directly.
        if let Err(e) = self
            .metadata_backend
            .upsert_collection_proto(&proto_collection)
            .await
            .context("Failed to store collection metadata_info")
        {
            let _ = self.drop_collection_catalog_asset(&proto_collection).await;
            return Err(e);
        }

        info!(
            "✅ Collection created: {} (UUID: {}) with storage at: {} in {}μs",
            config.name,
            uuid,
            base_location,
            start_time.elapsed().as_micros()
        );

        // Use proto collection directly - no conversion needed in proto-first architecture

        // Generate storage path template
        let storage_path = format!("${{base_path}}/collections/{}", uuid);

        // Bump the corpus_version for (tenant, collection) so the
        // process-wide PlanCache invalidates on the first planner
        // lookup against the freshly-created collection. New
        // collections start at version 2 (default was 1), so any
        // entry that ended up in the cache during a race condition
        // — e.g. a search that arrived between catalog upsert and
        // this bump — gets superseded immediately.
        if let Some(tenant_ctx) = tenant_context {
            let version = crate::catalog::CorpusVersionRegistry::global()
                .bump(&tenant_ctx.tenant_id, &config.name)
                .await;
            debug!(
                "🔄 corpus_version bumped after create: tenant={} collection={} version={}",
                tenant_ctx.tenant_id, config.name, version
            );
        }

        Ok(CollectionServiceResponse {
            success: true,
            collection: Some(proto_collection), // Direct proto usage - no conversion!
            storage_path: Some(storage_path),
            // error_message removed -  None,
            error_code: None,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    /// Get the full proto collection with all metadata - direct access to deserialized object
    pub async fn collection(&self, identifier: &str) -> Result<Option<Collection>> {
        self.get_native_proto(identifier).await
    }

    /// Get Collection by name or UUID
    async fn get_native_proto(&self, identifier: &str) -> Result<Option<Collection>> {
        if let Some(collection) = self.collection_from_catalog_asset(identifier).await? {
            return Ok(Some(collection));
        }

        // The catalog is the sole read authority: collection_from_catalog_asset
        // already resolves by both name and UUID, so a miss means the collection
        // does not exist.
        Ok(None)
    }

    /// ✅ RESOLVE COLLECTION NAME/ID TO COLLECTION ID
    /// This is the key method for collection identifier resolution
    /// - Input: Collection name OR collection ID
    /// - Output: Collection ID (base62) for internal use
    /// - Used by WAL, storage, and index path resolution
    pub async fn resolve_collection_id(&self, identifier: &str) -> Result<Option<String>> {
        tracing::debug!("🔍 Resolving collection identifier: '{}'", identifier);

        // Resolution is NAME-AUTHORITATIVE and shape-independent: `collection()`
        // looks up the catalog asset (by name) first, then the metadata backend
        // (by id). The historical base62-id "looks like an id" length heuristic is
        // gone — IDs are now opaque UUIDs (no overlap with user names), so name
        // length carries no meaning here. This is why short SQL/ANSI table names
        // are safe (TPC-H `part`/`orders`/`region`) and the 8-char floor was dropped.
        if let Some(collection) = self.collection(identifier).await? {
            let collection_id = collection.id;
            tracing::debug!(
                "✅ Resolved '{}' -> collection_id: '{}'",
                identifier,
                collection_id
            );
            Ok(Some(collection_id))
        } else {
            tracing::debug!("❌ Collection not found: '{}'", identifier);
            Ok(None)
        }
    }

    /// ✅ RESOLVE COLLECTION ID TO COLLECTION NAME  
    /// Reverse resolution for user-friendly displays
    pub async fn resolve_collection_name(&self, collection_id: &str) -> Result<Option<String>> {
        if let Some(collection) = self.collection(collection_id).await?
            && let Some(config) = &collection.config
        {
            return Ok(Some(config.name.clone()));
        }
        Ok(None)
    }

    /// Convert Collection to core Collection - direct proto to core mapping
    /// Get IndexConfig for a collection by name or UUID with caching
    pub async fn native_index_config(
        &self,
        identifier: &str,
    ) -> Result<Option<crate::index::config::RuntimeIndexConfig>> {
        debug!("🔍 Getting IndexConfig for collection: {}", identifier);

        // Check cache first
        if let Some(cached_config) = self.index_config_cache.get(identifier) {
            debug!(
                "📋 Retrieved IndexConfig from cache for collection: {}",
                identifier
            );
            return Ok(Some(cached_config.value().clone()));
        }

        if let Some(proto_collection) = self.get_native_proto(identifier).await? {
            let index_config = self.parse_index_config_from_proto(&proto_collection)?;

            // Cache the result
            self.index_config_cache
                .insert(identifier.to_string(), index_config.clone());
            self.index_config_cache
                .insert(proto_collection.id.clone(), index_config.clone()); // Cache by UUID too

            debug!("📋 Cached IndexConfig for collection: {}", identifier);
            Ok(Some(index_config))
        } else {
            Ok(None)
        }
    }

    /// Convert proto IndexConfig to internal IndexConfig
    fn convert_proto_index_config(
        &self,
        _proto_config: &crate::proto::proximadb_v1::IndexConfig,
    ) -> Result<crate::index::config::RuntimeIndexConfig> {
        // Extract algorithm name from proto config
        let _algorithm_name = match _proto_config.algorithm {
            1 => "HNSW",
            2 => "IVF",
            3 => "PQ",
            4 => "FLAT",
            5 => "ANNOY",
            _ => "HNSW", // Default to HNSW
        };

        // Use the from_proto method that handles all the config extraction
        crate::index::config::RuntimeIndexConfig::from_proto(_proto_config)
    }

    /// Parse IndexConfig from Collection
    fn parse_index_config_from_proto(
        &self,
        proto: &Collection,
    ) -> Result<crate::index::config::RuntimeIndexConfig> {
        // Check if proto has index_config field
        if let Some(config) = proto.config.as_ref()
            && !config.index_configs.is_empty()
        {
            // Take the first IndexConfig from proto (index_configs is a Vec)
            if let Some(first_config) = config.index_configs.first() {
                // Convert from proto IndexConfig to internal IndexConfig
                return self.convert_proto_index_config(first_config);
            }
        }

        // No IndexConfig found, create smart defaults based on algorithm
        let config = proto
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection has no config"))?;
        let indexing_algorithm: crate::core::IndexingAlgorithm =
            match config.primary_index.as_deref().unwrap_or("default") {
                "hnsw" => "hnsw".to_string(),
                "ivf" => "ivf".to_string(),
                "pq" => "pq".to_string(),
                "flat" => "flat".to_string(),
                "annoy" => "annoy".to_string(),
                "lsh" => "lsh".to_string(),
                _ => "hnsw".to_string(),
            };

        let algorithm_str = match indexing_algorithm.as_str() {
            "hnsw" => "HNSW",
            "ivf" => "IVF",
            "pq" => "PQ",
            "flat" => "FLAT",
            "annoy" => "ANNOY",
            "lsh" => "LSH",
            _ => "HNSW", // Default to HNSW
        };

        let smart_config = crate::index::config::RuntimeIndexConfig::create_smart_default(
            algorithm_str,
            config.dimension as usize,
            None, // Collection size hint not available
        );

        debug!(
            "📋 Created smart default IndexConfig for collection: {}",
            config.name
        );
        Ok(smart_config)
    }

    // Configuration parsing helper methods

    /// Get quantization configuration for a collection
    pub async fn native_quantization_config(
        &self,
        identifier: &str,
    ) -> Result<Option<crate::proto::proximadb_v1::QuantizationConfig>> {
        debug!(
            "🔍 Getting quantization config for collection: {}",
            identifier
        );

        if let Some(proto) = self.get_native_proto(identifier).await? {
            Ok(proto.config.and_then(|c| c.quantization))
        } else {
            Ok(None)
        }
    }

    /// Get search hints for a collection  
    pub async fn native_search_hints(&self, identifier: &str) -> Result<Option<serde_json::Value>> {
        debug!("🔍 Getting search hints for collection: {}", identifier);

        if let Some(_proto) = self.get_native_proto(identifier).await? {
            // Extract search hints from proto config
            if let Some(config) = _proto.config.as_ref() {
                // Build search hints from collection configuration
                let mut hints = serde_json::json!({
                    "ef_search": 200,
                    "max_candidates": 500,
                    "use_quantized": config.quantization.is_some(),
                    "enable_reranking": true
                });

                // Extract hints from index configs
                if let Some(first_index) = config.index_configs.first() {
                    // Override with algorithm-specific parameters
                    if let Some(hnsw_config) = &first_index.hnsw_config {
                        hints["ef_search"] = serde_json::json!(hnsw_config.ef_search);
                        hints["max_candidates"] =
                            serde_json::json!(hnsw_config.ef_search.unwrap_or(100) * 2);
                    }
                    if let Some(ivf_config) = &first_index.ivf_config {
                        hints["n_probe"] = serde_json::json!(ivf_config.n_probe);
                        hints["max_candidates"] =
                            serde_json::json!(ivf_config.n_probe.unwrap_or(10) * 100);
                    }
                }

                // Add storage engine specific hints
                hints["storage_engine"] =
                    serde_json::json!(crate::core::conversions::storage_engine_to_string(
                        config.storage_engine.unwrap_or(StorageEngine::Sst as i32)
                    ));

                Ok(Some(hints))
            } else {
                // Return default hints if no config
                Ok(Some(serde_json::json!({
                    "ef_search": 200,
                    "max_candidates": 500,
                    "use_quantized": false,
                    "enable_reranking": true
                })))
            }
        } else {
            Ok(None)
        }
    }

    /// Get index parameters for a collection
    pub async fn native_index_params(&self, identifier: &str) -> Result<Option<serde_json::Value>> {
        debug!("🔍 Getting index params for collection: {}", identifier);

        if let Some(proto) = self.get_native_proto(identifier).await? {
            if let Some(config) = proto.config {
                // Proto uses index_configs (plural) which is a vector of IndexConfig
                Ok(Some(serde_json::to_value(&config.index_configs)?))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    /// Get storage configuration for a collection
    pub async fn native_storage_config(
        &self,
        identifier: &str,
    ) -> Result<Option<serde_json::Value>> {
        debug!("🔍 Getting storage config for collection: {}", identifier);

        if let Some(_proto) = self.get_native_proto(identifier).await? {
            if let Some(config) = _proto.config.as_ref() {
                // Build storage config from proto
                let engine_name = crate::core::conversions::storage_engine_to_string(
                    config.storage_engine.unwrap_or(StorageEngine::Sst as i32),
                );

                let mut storage_config = serde_json::json!({
                    "engine": engine_name,
                    "enable_compression": true,
                    "enable_deduplication": false, // Not exposed in proto yet
                    "enable_multi_tenancy": false,  // Not exposed in proto yet
                });

                // Add engine-specific configurations
                if engine_name == "VIPER" {
                    storage_config["parquet_config"] = serde_json::json!({
                        "row_group_size": 65536,
                        "compression": "snappy",
                        "enable_statistics": true,
                        "enable_bloom_filter": true,
                    });

                    // Add quantization config if present
                    if let Some(quant_config) = &config.quantization {
                        storage_config["quantization_enabled"] = serde_json::json!(true);
                        storage_config["quantization_level"] = serde_json::json!(quant_config);
                    }
                }

                Ok(Some(storage_config))
            } else {
                // Return default storage config
                Ok(Some(serde_json::json!({
                    "engine": "LSM",
                    "enable_compression": true,
                    "enable_deduplication": false,
                    "enable_multi_tenancy": false,
                })))
            }
        } else {
            Ok(None)
        }
    }

    /// List all collections - returns proto Collections directly (proto-first architecture)
    pub async fn list_collections(&self) -> Result<Vec<Collection>> {
        debug!("📋 Listing all collections");
        // The catalog is the sole read authority.
        self.list_collections_from_catalog().await
    }

    /// Delete collection with comprehensive cleanup across all storage components
    pub async fn delete_collection(
        &self,
        collection_identifier: &str,
    ) -> Result<CollectionServiceResponse> {
        info!("🗑️ Deleting collection: {}", collection_identifier);
        let start_time = std::time::Instant::now();

        // Get collection record first to retrieve UUID and other details. xCatalog is checked
        // first; the legacy backend is only a compatibility fallback.
        let collection_record = self.collection(collection_identifier).await?;

        if let Some(record) = collection_record {
            let collection_uuid = record.id.clone();
            let collection_name = record.config.as_ref().map(|c| c.name.clone()).clone();

            info!(
                "🔍 Found collection to delete: {} (UUID: {})",
                collection_name.as_deref().unwrap_or("<unnamed>"),
                collection_uuid
            );

            // Step 1: Clean up all storage directories and files
            let cleanup_results = self
                .cleanup_storage_directories(
                    collection_name.as_deref().unwrap_or(collection_identifier),
                    &collection_uuid,
                )
                .await;
            match cleanup_results {
                Ok(cleaned_components) => {
                    info!(
                        "🧹 Cleaned up {} storage components for collection {}",
                        cleaned_components,
                        collection_name.as_deref().unwrap_or("<unnamed>")
                    );
                }
                Err(e) => {
                    warn!(
                        "⚠️ Some storage cleanup failed for collection {}: {}",
                        collection_name.as_deref().unwrap_or("<unnamed>"),
                        e
                    );
                    // Continue with metadata deletion even if storage cleanup partially fails
                }
            }

            // Step 2: Assignment removal is no longer needed
            // Storage assignment is now part of collection metadata which gets deleted

            // Step 3: Delete from xCatalog and metadata backend
            if let Err(e) = self.drop_collection_catalog_asset(&record).await {
                return Ok(CollectionServiceResponse::error(
                    format!("CATALOG_DELETE_FAILED: {}", e),
                    start_time.elapsed().as_micros() as i64,
                ));
            }

            if let Err(e) = self
                .metadata_backend
                .delete_collection(collection_name.as_deref().unwrap_or(collection_identifier))
                .await
            {
                if Self::is_not_found_error(&e) {
                    debug!(
                        "Legacy collection metadata cache was already absent for {}",
                        collection_identifier
                    );
                } else {
                    return Err(e);
                }
            }
            let deleted = true;

            if deleted {
                info!(
                    "✅ Collection deleted: {} (UUID: {}) in {}μs",
                    collection_name.as_deref().unwrap_or("<unnamed>"),
                    collection_uuid,
                    start_time.elapsed().as_micros()
                );

                Ok(CollectionServiceResponse {
                    success: true,
                    collection: Some(record.clone()), // Include the deleted collection record
                    storage_path: None,
                    // error_message removed -  None,
                    error_code: None,
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                })
            } else {
                Ok(CollectionServiceResponse {
                    success: false,
                    collection: Some(record.clone()), // Include the collection that failed to delete
                    storage_path: None,
                    error_code: Some("METADATA_DELETE_FAILED".to_string()),
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                })
            }
        } else {
            Ok(CollectionServiceResponse {
                success: false,
                collection: None,
                storage_path: None,
                error_code: Some("COLLECTION_NOT_FOUND".to_string()),
                processing_time_us: start_time.elapsed().as_micros() as i64,
            })
        }
    }

    /// Update collection statistics (called by storage engine after vector operations)
    pub async fn update_stats(
        &self,
        collection_name: &str,
        vector_delta: i64,
        size_delta: i64,
    ) -> Result<()> {
        debug!(
            "📊 Updating stats for {}: vectors={:+}, size={:+}",
            collection_name, vector_delta, size_delta
        );

        // Get current record, update stats, and save back
        if let Some(mut record) = self
            .metadata_backend
            .get_collection(collection_name)
            .await?
        {
            // Update stats manually for Collection
            if let Some(stats) = record.stats.as_mut() {
                stats.vector_count += vector_delta;
                stats.data_size_bytes += size_delta;
            }
            record.updated_at = chrono::Utc::now().timestamp_millis();

            self.upsert_collection_catalog_asset(&record).await?;

            self.metadata_backend
                .upsert_collection_proto(&record)
                .await?;
        } else {
            warn!(
                "⚠️ Attempted to update stats for non-existent collection: {}",
                collection_name
            );
        }

        Ok(())
    }

    /// Get collection statistics for cost-based query optimization
    ///
    /// Returns canonical `CollectionStats` from the storage engine, enriched
    /// with metadata from the collection config (dimension, index type).
    /// Used by the query planner's CostModel.
    pub async fn get_collection_stats(
        &self,
        collection_name: &str,
        storage_engine: Option<&std::sync::Arc<dyn crate::storage::traits::UnifiedStorageEngine>>,
    ) -> Result<crate::storage::traits::CollectionStats> {
        // If a storage engine is provided, delegate to it for real stats
        if let Some(engine) = storage_engine {
            let mut stats = engine.collection_stats(collection_name).await?;

            // Enrich with metadata from collection config
            if let Some(collection) = self.collection(collection_name).await?
                && let Some(config) = &collection.config
            {
                stats.dimension = Some(config.dimension);
            }

            return Ok(stats);
        }

        // Fallback: return stats from proto collection metadata
        if let Some(collection) = self.collection(collection_name).await? {
            let mut stats = crate::storage::traits::CollectionStats::default();
            if let Some(proto_stats) = collection.stats {
                stats.row_count = proto_stats.vector_count as u64;
                stats.total_bytes = proto_stats.data_size_bytes as u64;
            }
            if let Some(config) = &collection.config {
                stats.dimension = Some(config.dimension);
            }
            return Ok(stats);
        }

        Ok(crate::storage::traits::CollectionStats::default())
    }

    /// Get collection UUID by name or UUID
    pub async fn uuid(&self, collection_id: &str) -> Result<Option<String>> {
        debug!("🔍 Getting UUID for collection: {}", collection_id);

        // First check if it's already a UUID
        if proximadb_kernel::uuid::Uuid::parse(collection_id).is_ok() {
            // Verify it exists
            if let Some(collection) = self.collection(collection_id).await? {
                return Ok(Some(collection.id));
            }
            return Ok(None);
        }

        // Otherwise look up by name
        if let Some(collection) = self.collection(collection_id).await? {
            Ok(Some(collection.id))
        } else {
            Ok(None)
        }
    }

    /// Update collection - type-safe method with native parameters
    pub async fn update_collection(
        &self,
        identifier: &str,
        config_update: Option<CollectionConfig>, // Use native proto type!
    ) -> Result<CollectionServiceResponse> {
        info!("📝 Updating collection: {}", identifier);
        let start_time = std::time::Instant::now();

        // Get current record (supports both names and UUIDs) through xCatalog first.
        let mut record = match self.collection(identifier).await? {
            Some(record) => record,
            None => {
                return Ok(CollectionServiceResponse {
                    success: false,
                    collection: None,
                    storage_path: None,
                    error_code: Some("COLLECTION_NOT_FOUND".to_string()),
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                });
            }
        };
        let previous_record = record.clone();

        // Apply updates using native proto types
        if let Some(new_config) = config_update {
            // Merge the new config with existing one to preserve unchanged fields
            if let Some(existing_config) = record.config.as_mut() {
                // Only update fields that are provided in new_config
                if !new_config.name.is_empty() {
                    existing_config.name = new_config.name;
                }
                if new_config.dimension > 0 {
                    existing_config.dimension = new_config.dimension;
                }
                if new_config.distance_metric.unwrap_or(0) != 0 {
                    existing_config.distance_metric = new_config.distance_metric;
                }
                if new_config.storage_engine.unwrap_or(0) != 0 {
                    existing_config.storage_engine = new_config.storage_engine;
                }
                if new_config.description.is_some() {
                    existing_config.description = new_config.description;
                }
                if !new_config.tags.is_empty() {
                    existing_config.tags = new_config.tags;
                }
                if new_config.owner.is_some() {
                    existing_config.owner = new_config.owner;
                }
                if !new_config.filterable_columns.is_empty() {
                    existing_config.filterable_columns = new_config.filterable_columns;
                }
                // Add other fields as needed
            } else {
                // No existing config, use the new one
                record.config = Some(new_config);
            }
        }

        // Update timestamp
        record.updated_at = chrono::Utc::now().timestamp_millis();

        if previous_record
            .config
            .as_ref()
            .zip(record.config.as_ref())
            .is_some_and(|(previous, current)| previous.name != current.name)
        {
            self.drop_collection_catalog_asset(&previous_record)
                .await
                .context("Failed to remove previous collection catalog asset")?;
        }

        self.upsert_collection_catalog_asset(&record)
            .await
            .context("Failed to update collection catalog metadata")?;

        // Store updated record
        self.metadata_backend
            .upsert_collection_proto(&record)
            .await
            .context("Failed to update collection metadata_info")?;

        info!(
            "✅ Collection updated: {} in {}μs",
            identifier,
            start_time.elapsed().as_micros()
        );

        // Record is already a proto Collection, no conversion needed
        let collection = record;

        Ok(CollectionServiceResponse {
            success: true,
            collection: Some(collection),
            storage_path: None,
            // error_message removed -  None,
            error_code: None,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    /// Get access to the metadata backend for recovery operations
    pub fn metadata_backend(&self) -> &Arc<dyn InternalCollectionProvider> {
        &self.metadata_backend
    }

    /// The catalog this service reads/writes collection assets through, if wired.
    /// Used to make the catalog the WAL/recovery collection-resolution authority.
    pub fn catalog_manager(&self) -> Option<Arc<CatalogManager>> {
        self.catalog_manager.clone()
    }

    /// Resolve compression configuration based on SDK request and server defaults
    fn resolve_compression_config(
        &self,
        requested: Option<&crate::proto::proximadb_v1::CompressionConfig>,
        _storage_engine: i32,
    ) -> Option<crate::proto::proximadb_v1::CompressionConfig> {
        use crate::proto::proximadb_v1::{CompressionAlgorithm, CompressionConfig};

        // If compression explicitly requested, validate and use it
        if let Some(config) = requested {
            // Validate compression level if specified
            if let Some(level) = config.level
                && let Ok(CompressionAlgorithm::CompressionZstd) =
                    CompressionAlgorithm::try_from(config.algorithm)
                && !(1..=22).contains(&level)
            {
                warn!("Invalid ZSTD compression level {}, using default 3", level);
                return Some(CompressionConfig {
                    algorithm: config.algorithm,
                    level: Some(3),
                    adaptive: config.adaptive,
                    min_ratio: config.min_ratio,
                    enable_quantization: config.enable_quantization,
                    quantization_type: config.quantization_type.clone(),
                    normalization_method: config.normalization_method.clone(),
                    block_size_kb: config.block_size_kb,
                    dynamic_block_sizing: config.dynamic_block_sizing,
                });
            }
            return Some(config.clone());
        }

        // SDK-DRIVEN COMPRESSION (2025-08-06): No server defaults!
        // Compression must be specified by the SDK/client
        // Return None to indicate no compression if not specified by SDK
        None

        // SDK-DRIVEN: All compression config removed from server.
        // Compression is 100% controlled by SDK/client through collection metadata.
    }

    /// Update collection compression configuration
    pub async fn update_collection_compression(
        &self,
        identifier: &str,
        compression: &crate::proto::proximadb_v1::CompressionConfig,
    ) -> Result<CollectionServiceResponse> {
        let start_time = std::time::Instant::now();

        // Get existing collection
        let collection = match self.collection(identifier).await? {
            Some(c) => c,
            None => {
                return Ok(CollectionServiceResponse::error(
                    format!(
                        "COLLECTION_NOT_FOUND: Collection '{}' not found",
                        identifier
                    ),
                    start_time.elapsed().as_micros() as i64,
                ));
            }
        };

        // Update compression config (now in storage_config)
        let mut updated_collection = collection.clone();
        if let Some(ref mut config) = updated_collection.config {
            // Ensure storage_config exists
            if config.storage_config.is_none() {
                config.storage_config = Some(crate::proto::proximadb_v1::StorageConfig::default());
            }
            // Set compression in storage_config
            if let Some(ref mut storage_config) = config.storage_config {
                storage_config.compression = Some(compression.algorithm);
            }
        }

        // Store the updated collection. The catalog is the read authority, so the
        // catalog asset write is what makes the compression change visible; the
        // legacy backend write stays (dual-write) until it is removed wholesale.
        self.upsert_collection_catalog_asset(&updated_collection)
            .await
            .context("Failed to update collection compression in catalog")?;
        self.metadata_backend
            .upsert_collection_proto(&updated_collection)
            .await
            .context("Failed to update collection metadata_info")?;

        info!(
            "✅ Updated compression for collection {}: algorithm={}, level={:?}",
            identifier, compression.algorithm, compression.level
        );

        Ok(CollectionServiceResponse {
            success: true,
            collection: Some(updated_collection),
            storage_path: None,
            // error_message removed -  None,
            error_code: None,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    /// Validate collection configuration
    #[allow(dead_code)]
    fn validate_collection_config(&self, config: &CollectionConfig) -> Result<()> {
        if config.name.is_empty() {
            return Err(anyhow::anyhow!("Collection name cannot be empty"));
        }

        if config.name.len() > 255 {
            return Err(anyhow::anyhow!(
                "Collection name too long (max 255 characters)"
            ));
        }

        if config.dimension == 0 {
            return Err(anyhow::anyhow!("Dimension must be positive"));
        }

        if config.dimension > 65536 {
            return Err(anyhow::anyhow!("Dimension too large (max 65536)"));
        }

        // Validate name contains only allowed characters
        if !config
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return Err(anyhow::anyhow!(
                "Collection name contains invalid characters (only alphanumeric, _, -, . allowed)"
            ));
        }

        Ok(())
    }

    /// Create storage directories for a new collection
    ///
    /// For multi-tenant deployments, paths are isolated under `{base}/tenants/{tenant_id}/`.
    async fn create_storage_directories(
        &self,
        base_location: &str,
        collection_name: &str,
        collection_uuid: &str,
        tenant_id: Option<&str>,
    ) -> Result<Vec<StorageComponentType>> {
        let tenant_info = tenant_id.unwrap_or("(default)");
        info!(
            "🏗️ Creating storage directories for collection {} (UUID: {}, tenant: {}) at base: {}",
            collection_name, collection_uuid, tenant_info, base_location
        );

        let mut created_components = Vec::new();

        // Build paths under base location using StoragePath utility (tenant-aware)
        let write_buffer_dir =
            StoragePath::collection_wal_path_with_tenant(base_location, tenant_id, collection_uuid);
        let data_dir = StoragePath::collection_data_path_with_tenant(
            base_location,
            tenant_id,
            collection_uuid,
        );
        let indexes_dir = StoragePath::collection_index_path_with_tenant(
            base_location,
            tenant_id,
            collection_uuid,
        );

        // Create directories
        if let Ok(filesystem) = self.filesystem_factory.get_filesystem(base_location) {
            // Create WAL directory
            if let Err(e) = filesystem.create_dir_all(&write_buffer_dir).await {
                warn!(
                    "⚠️ Failed to create WAL directory {}: {}",
                    write_buffer_dir, e
                );
            } else {
                debug!("Created WAL storage directory: {}", write_buffer_dir);
                created_components.push(StorageComponentType::Wal);
            }

            // Create data directory (flat structure for unified compaction framework)
            if let Err(e) = filesystem.create_dir_all(&data_dir).await {
                warn!("⚠️ Failed to create data directory {}: {}", data_dir, e);
            } else {
                debug!("Created data storage directory: {}", data_dir);
                created_components.push(StorageComponentType::Storage);
            }

            // Create index directory
            if let Err(e) = filesystem.create_dir_all(&indexes_dir).await {
                warn!("⚠️ Failed to create index directory {}: {}", indexes_dir, e);
            } else {
                debug!("Created index storage directory: {}", indexes_dir);
                created_components.push(StorageComponentType::Index);
            }
        } else {
            return Err(anyhow::anyhow!(
                "Failed to get filesystem for location: {}",
                base_location
            ));
        }

        info!(
            "🏗️ Created {} storage components for collection {} at location {}",
            created_components.len(),
            collection_name,
            base_location
        );
        Ok(created_components)
    }

    /// Clean up storage directories for a deleted collection
    async fn cleanup_storage_directories(
        &self,
        collection_name: &str,
        collection_uuid: &str,
    ) -> Result<usize> {
        info!(
            "🧹 Cleaning up storage directories for collection {} (UUID: {})",
            collection_name, collection_uuid
        );

        let mut cleaned_components = 0;

        // Get collection to find storage assignment
        let collection = match self
            .metadata_backend
            .get_collection(collection_uuid)
            .await?
        {
            Some(col) => col,
            None => {
                warn!("Collection {} not found in metadata_info", collection_uuid);
                return Ok(0);
            }
        };

        if let Some(ref assignment) = collection.storage_assignment {
            let base_location = &assignment.base_location;

            // Delete the entire collection directory (includes write_buffer/, data/, indexes/)
            let collection_dir = format!(
                "{}/{}",
                base_location.trim_end_matches('/'),
                collection_uuid
            );

            match self.filesystem_factory.get_filesystem(base_location) {
                Ok(filesystem) => {
                    // Check if directory exists before attempting to delete
                    match filesystem.exists(&collection_dir).await {
                        Ok(true) => {
                            // Recursively delete the entire collection directory
                            match filesystem.remove_dir_all(&collection_dir).await {
                                Ok(_) => {
                                    info!(
                                        "✅ Deleted entire collection directory: {}",
                                        collection_dir
                                    );
                                    cleaned_components = 3; // All components deleted
                                }
                                Err(e) => {
                                    error!(
                                        "❌ Failed to delete collection directory {}: {}",
                                        collection_dir, e
                                    );
                                    return Err(anyhow::anyhow!(
                                        "Failed to delete collection directory: {}",
                                        e
                                    ));
                                }
                            }
                        }
                        Ok(false) => {
                            debug!(
                                "📂 Collection directory {} does not exist (already cleaned up)",
                                collection_dir
                            );
                            cleaned_components = 3; // Count as all cleaned
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ Failed to check existence of collection directory {}: {}",
                                collection_dir, e
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!("⚠️ Failed to get filesystem for {}: {}", base_location, e);
                }
            }
        } else {
            debug!(
                "📂 No storage assignment found for collection {} (may not have been created)",
                collection_name
            );
        }

        info!(
            "🧹 Cleaned up {} storage components for collection {}",
            cleaned_components, collection_name
        );
        Ok(cleaned_components)
    }

    async fn upsert_collection_catalog_asset(&self, collection: &Collection) -> Result<()> {
        let Some(catalog_manager) = &self.catalog_manager else {
            return Ok(());
        };

        let Some(config) = collection.config.as_ref() else {
            return Ok(());
        };

        let catalog = catalog_manager.default_catalog().await?;
        let identifier = Self::collection_table_identifier(config);

        if !catalog.namespace_exists(&identifier.namespace).await? {
            catalog
                .create_namespace(&identifier.namespace, std::collections::HashMap::new())
                .await?;
        }

        let schema = Self::catalog_schema_from_collection(collection)?;
        if catalog.table_exists(&identifier).await? {
            let mut existing = catalog.get_table(&identifier).await?;
            if existing
                .properties
                .get("asset.kind")
                .is_none_or(|kind| kind != "collection")
            {
                existing
                    .properties
                    .insert("asset.capability.vector".to_string(), "true".to_string());
                existing
                    .properties
                    .insert("collection.id".to_string(), collection.id.clone());
                existing
                    .properties
                    .insert("collection.name".to_string(), config.name.clone());
                existing
                    .properties
                    .insert("vector.dimension".to_string(), config.dimension.to_string());
                existing.updated_at_ms = collection.updated_at / 1000;
                if existing.storage_layouts.is_empty() {
                    existing.storage_layouts = schema.storage_layouts.clone();
                }
                if existing.location.is_none() {
                    existing.location = schema.location.clone();
                }

                let _ = catalog.drop_table(&identifier, false).await?;
                catalog.create_table(&identifier, existing).await?;
                return Ok(());
            }

            let _ = catalog.drop_table(&identifier, false).await?;
        }
        catalog.create_table(&identifier, schema).await?;
        Ok(())
    }

    async fn drop_collection_catalog_asset(&self, collection: &Collection) -> Result<()> {
        let Some(catalog_manager) = &self.catalog_manager else {
            return Ok(());
        };

        let Some(config) = collection.config.as_ref() else {
            return Ok(());
        };

        let catalog = catalog_manager.default_catalog().await?;
        let identifier = Self::collection_table_identifier(config);
        if catalog.table_exists(&identifier).await? {
            let _ = catalog.drop_table(&identifier, false).await?;
        }
        Ok(())
    }

    async fn collection_from_catalog_asset(&self, identifier: &str) -> Result<Option<Collection>> {
        let Some(catalog_manager) = &self.catalog_manager else {
            return Ok(None);
        };

        if let Ok((catalog, table_id)) = catalog_manager.resolve_table(identifier).await
            && catalog.table_exists(&table_id).await.unwrap_or(false)
        {
            let schema = catalog.get_table(&table_id).await?;
            if let Some(collection) = Self::collection_from_catalog_schema(&table_id, &schema)? {
                return Ok(Some(collection));
            }
        }

        for collection in self.list_collections_from_catalog().await? {
            if collection.id == identifier {
                return Ok(Some(collection));
            }
            if let Some(config) = &collection.config
                && config.name == identifier
            {
                return Ok(Some(collection));
            }
        }

        Ok(None)
    }

    async fn list_collections_from_catalog(&self) -> Result<Vec<Collection>> {
        let Some(catalog_manager) = &self.catalog_manager else {
            return Ok(Vec::new());
        };

        let catalog = match catalog_manager.default_catalog().await {
            Ok(catalog) => catalog,
            Err(_) => return Ok(Vec::new()),
        };

        let mut namespaces: Vec<Vec<String>> = catalog
            .list_namespaces(None)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|namespace| namespace.levels)
            .collect();
        if !namespaces.iter().any(|namespace| namespace == &["default"]) {
            namespaces.push(vec!["default".to_string()]);
        }

        let mut collections = Vec::new();
        let mut seen_ids = HashSet::new();
        for namespace in namespaces {
            let table_ids = match catalog.list_tables(&namespace).await {
                Ok(table_ids) => table_ids,
                Err(_) => continue,
            };

            for table_id in table_ids {
                let schema = match catalog.get_table(&table_id).await {
                    Ok(schema) => schema,
                    Err(_) => continue,
                };
                let Some(collection) = Self::collection_from_catalog_schema(&table_id, &schema)?
                else {
                    continue;
                };
                if seen_ids.insert(collection.id.clone()) {
                    collections.push(collection);
                }
            }
        }

        Ok(collections)
    }

    pub(crate) fn collection_from_catalog_schema(
        table_id: &TableIdentifier,
        schema: &CatalogTableSchema,
    ) -> Result<Option<Collection>> {
        if schema
            .properties
            .get("asset.kind")
            .is_none_or(|kind| kind != "collection")
        {
            return Ok(None);
        }

        let Some(id) = schema.properties.get("collection.id").cloned() else {
            return Ok(None);
        };

        let name = schema
            .properties
            .get("collection.name")
            .cloned()
            .unwrap_or_else(|| table_id.to_fqn());
        let dimension = schema
            .properties
            .get("vector.dimension")
            .and_then(|dimension| dimension.parse::<u32>().ok())
            .or_else(|| {
                schema
                    .columns
                    .iter()
                    .find(|column| column.name == "embedding")
                    .and_then(|column| column.properties.get("dimension"))
                    .and_then(|dimension| dimension.parse::<u32>().ok())
            })
            .unwrap_or_default();

        let storage_engine = schema
            .storage_layouts
            .first()
            .and_then(|layout| layout.properties.get("storage_engine"))
            .map(|engine| Self::storage_engine_from_catalog(engine))
            .unwrap_or(StorageEngine::Sst as i32);

        // Round-trip canonical_embedding_precision from the catalog
        // schema. Mirror of the forward mapping in
        // `catalog_schema_from_collection`. Unset / Fp32 maps back to
        // None so legacy collections keep their existing serialized
        // shape (no behavior change for fp32 callers).
        let canonical_embedding_precision = {
            use crate::proto::proximadb_v1::EmbeddingPrecision;
            match schema.canonical_embedding_precision {
                proximadb_records::EmbeddingScalarType::Fp32 => None,
                proximadb_records::EmbeddingScalarType::Fp16 => {
                    Some(EmbeddingPrecision::Fp16 as i32)
                }
                proximadb_records::EmbeddingScalarType::Bf16 => {
                    Some(EmbeddingPrecision::Bf16 as i32)
                }
                proximadb_records::EmbeddingScalarType::Int8Scalar => {
                    Some(EmbeddingPrecision::Int8 as i32)
                }
                proximadb_records::EmbeddingScalarType::UInt8Scalar => {
                    Some(EmbeddingPrecision::Uint8 as i32)
                }
            }
        };

        let mut config = CollectionConfig {
            name,
            dimension,
            storage_engine: Some(storage_engine),
            owner: schema.properties.get("owner").cloned(),
            tags: schema
                .properties
                .get("tags")
                .map(|tags| {
                    tags.split(',')
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            canonical_embedding_precision,
            ..Default::default()
        };

        config.filterable_columns = schema
            .columns
            .iter()
            .filter(|column| column.id >= 100)
            .map(|column| {
                let indexed = schema
                    .indexes
                    .iter()
                    .any(|index| index.columns.iter().any(|name| name == &column.name));
                let supports_range = schema.indexes.iter().any(|index| {
                    index.columns.iter().any(|name| name == &column.name)
                        && index.index_type == CatalogIndexType::BTree
                });
                FilterableColumnSpec {
                    name: column.name.clone(),
                    data_type: Self::filterable_data_type(&column.data_type),
                    indexed,
                    supports_range,
                    estimated_cardinality: None,
                }
            })
            .collect();

        config.distance_metric = schema
            .properties
            .get("vector.distance_metric")
            .and_then(|metric| metric.parse::<i32>().ok());

        config.index_configs = schema
            .indexes
            .iter()
            .filter(|index| index.columns.iter().any(|column| column == "embedding"))
            .map(|index| IndexConfig {
                index_name: index.name.clone(),
                algorithm: Self::indexing_algorithm(index.index_type),
                parameters: index.properties.clone(),
                enabled: Some(true),
                ..Default::default()
            })
            .collect();

        // TD-122: prefer the neutral per-index/quant blob when present so the
        // detailed HNSW/IVF params, is_primary, and quantization survive the
        // round-trip. Legacy collections persisted before this lack the blob and
        // keep the coarse reconstruction above (mixed-read-safe).
        if let Some(json) = schema.properties.get("collection.index_config") {
            let restored = crate::storage::metadata::catalog_config::index_configs_from_json(json);
            if !restored.is_empty() {
                config.index_configs = restored;
            }
        }
        if let Some(json) = schema.properties.get("collection.quantization") {
            config.quantization =
                crate::storage::metadata::catalog_config::quantization_from_json(json);
        }
        // TD-122: restore the ProximaRecord schema config (enable flag, enforcement,
        // text columns) so the v2 get surface can reconstruct the schema/flags.
        if let Some(json) = schema.properties.get("collection.record_schema") {
            crate::storage::metadata::catalog_config::apply_record_schema_from_json(
                &mut config,
                json,
            );
        }
        // Lossless round-trip: if the asset carries the full serialized config it
        // is authoritative — it captures every field (including ones not mapped to
        // a typed catalog property), so no collection config is ever silently
        // dropped on read. The per-field properties above remain for pg_catalog
        // introspection.
        if let Some(json) = schema.properties.get("collection.config_json")
            && let Ok(full) = serde_json::from_str::<CollectionConfig>(json)
        {
            config = full;
        }

        let location = schema
            .storage_layouts
            .first()
            .and_then(|layout| layout.location.clone())
            .or_else(|| schema.location.clone())
            .unwrap_or_default();
        let storage_assignment = if location.is_empty() {
            None
        } else {
            Some(StorageAssignment {
                primary_path: location.clone(),
                engine: storage_engine,
                base_location: location,
                ..Default::default()
            })
        };

        Ok(Some(Collection {
            id,
            config: Some(config),
            stats: Some(CollectionStats {
                vector_count: schema
                    .properties
                    .get("stats.row_count")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
                data_size_bytes: schema
                    .properties
                    .get("stats.data_size_bytes")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
                index_size_bytes: schema
                    .properties
                    .get("stats.index_size_bytes")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
            }),
            created_at: schema.created_at_ms * 1000,
            updated_at: schema.updated_at_ms * 1000,
            storage_assignment,
        }))
    }

    fn collection_table_identifier(config: &CollectionConfig) -> TableIdentifier {
        let parsed = TableIdentifier::parse(&config.name);
        if parsed.namespace.is_empty() {
            TableIdentifier::new(vec!["default".to_string()], parsed.name)
        } else {
            parsed
        }
    }

    fn catalog_schema_from_collection(collection: &Collection) -> Result<CatalogTableSchema> {
        let config = collection
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection has no config"))?;
        let identifier = Self::collection_table_identifier(config);
        let mut embedding_column = CatalogColumn::new(
            20,
            "embedding",
            ProximaType::DenseVector {
                element: proximadb_data_model::VectorElement::Float32,
                dim: 0,
            },
        );
        embedding_column
            .properties
            .insert("dimension".to_string(), config.dimension.to_string());

        let mut schema = CatalogTableSchema::new(identifier.name.clone())
            .with_column(CatalogColumn::new(0, "oid", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(1, "tenant_id", ProximaType::String))
            .with_column(CatalogColumn::new(
                2,
                "created_at_ns",
                ProximaType::TimestampTz(proximadb_data_model::TimeUnit::Nanosecond),
            ))
            .with_column(CatalogColumn::new(
                3,
                "updated_at_ns",
                ProximaType::TimestampTz(proximadb_data_model::TimeUnit::Nanosecond),
            ))
            .with_column(CatalogColumn::new(8, "props", ProximaType::Json))
            .with_column(embedding_column)
            .with_primary_key(vec!["oid".to_string()]);

        for (idx, column) in config.filterable_columns.iter().enumerate() {
            if column.name.is_empty() {
                continue;
            }
            schema = schema.with_column(CatalogColumn::new(
                100 + idx as i32,
                column.name.clone(),
                Self::catalog_data_type(column.data_type),
            ));

            if column.indexed {
                let index_type = if column.supports_range {
                    CatalogIndexType::BTree
                } else {
                    CatalogIndexType::Hash
                };
                schema = schema.with_index(CatalogIndex::new(
                    format!("idx_{}_{}", identifier.name, column.name),
                    vec![column.name.clone()],
                    index_type,
                ));
            }
        }

        for index in &config.index_configs {
            let index_type = Self::catalog_index_type(index.algorithm);
            schema = schema.with_index(CatalogIndex::new(
                if index.index_name.is_empty() {
                    format!("idx_{}_embedding", identifier.name)
                } else {
                    index.index_name.clone()
                },
                vec!["embedding".to_string()],
                index_type,
            ));

            let mut projection = CatalogProjection::rebuildable(
                if index.index_name.is_empty() {
                    format!("{}_ann", identifier.name)
                } else {
                    index.index_name.clone()
                },
                CatalogProjectionKind::VectorAnn,
                "primary",
            );
            projection.physical_format = CatalogPhysicalFormat::ProximaBlock;
            projection.freshness = ProjectionFreshness::Lazy;
            schema = schema.with_projection(projection);
        }

        let mut layout = CatalogStorageLayout::internal(
            "primary",
            match config.storage_engine.unwrap_or(StorageEngine::Sst as i32) {
                value if value == StorageEngine::Viper as i32 => CatalogStorageLayoutKind::Columnar,
                value if value == StorageEngine::Nova as i32 => CatalogStorageLayoutKind::Columnar,
                value if value == StorageEngine::Helix as i32 => {
                    CatalogStorageLayoutKind::LsmRecord
                }
                _ => CatalogStorageLayoutKind::RowRecord,
            },
        );
        layout.location = collection
            .storage_assignment
            .as_ref()
            .map(|assignment| assignment.base_location.clone())
            .filter(|location| !location.is_empty());
        layout
            .properties
            .insert("collection_id".to_string(), collection.id.clone());
        layout.properties.insert(
            "storage_engine".to_string(),
            config
                .storage_engine
                .and_then(|engine| StorageEngine::try_from(engine).ok())
                .map(|engine| format!("{:?}", engine))
                .unwrap_or_else(|| "Sst".to_string()),
        );

        schema.storage_layouts = vec![layout];
        schema.location = collection
            .storage_assignment
            .as_ref()
            .map(|assignment| assignment.base_location.clone())
            .filter(|location| !location.is_empty());
        schema.created_at_ms = collection.created_at / 1000;
        schema.updated_at_ms = collection.updated_at / 1000;
        schema
            .properties
            .insert("asset.kind".to_string(), "collection".to_string());
        schema
            .properties
            .insert("asset.capability.vector".to_string(), "true".to_string());
        schema
            .properties
            .insert("collection.id".to_string(), collection.id.clone());
        schema
            .properties
            .insert("collection.name".to_string(), config.name.clone());
        schema
            .properties
            .insert("vector.dimension".to_string(), config.dimension.to_string());
        if let Some(metric) = config.distance_metric {
            schema
                .properties
                .insert("vector.distance_metric".to_string(), metric.to_string());
        }
        if let Some(owner) = &config.owner {
            schema.properties.insert("owner".to_string(), owner.clone());
        }
        if !config.tags.is_empty() {
            schema
                .properties
                .insert("tags".to_string(), config.tags.join(","));
        }
        if let Some(stats) = &collection.stats {
            schema.properties.insert(
                "stats.row_count".to_string(),
                stats.vector_count.to_string(),
            );
            schema.properties.insert(
                "stats.data_size_bytes".to_string(),
                stats.data_size_bytes.to_string(),
            );
            schema.properties.insert(
                "stats.index_size_bytes".to_string(),
                stats.index_size_bytes.to_string(),
            );
        }

        // Map the proto EmbeddingPrecision discriminant to the catalog's
        // EmbeddingScalarType so the canonical_embedding_precision field
        // (read by CanonicalPrecisionResolver) reflects whatever the
        // create-collection request asked for. Unspecified / Fp32 stays
        // on the legacy default.
        if let Some(precision_value) = config.canonical_embedding_precision {
            use crate::proto::proximadb_v1::EmbeddingPrecision;
            schema.canonical_embedding_precision =
                match EmbeddingPrecision::try_from(precision_value) {
                    Ok(EmbeddingPrecision::Fp16) => proximadb_records::EmbeddingScalarType::Fp16,
                    Ok(EmbeddingPrecision::Bf16) => proximadb_records::EmbeddingScalarType::Bf16,
                    Ok(EmbeddingPrecision::Int8) => {
                        proximadb_records::EmbeddingScalarType::Int8Scalar
                    }
                    Ok(EmbeddingPrecision::Uint8) => {
                        proximadb_records::EmbeddingScalarType::UInt8Scalar
                    }
                    // Unspecified / Fp32 / unknown all map to the legacy default
                    _ => proximadb_records::EmbeddingScalarType::Fp32,
                };
        }

        // TD-122: persist the detailed per-index (HNSW m/ef, IVF n_lists/n_probe,
        // is_primary) and quantization (enabled, strategy) config in a neutral,
        // wire-independent form so GetCollection echoes back what CreateCollection
        // set. The CatalogIndex entries above only carry the index identity/type;
        // these JSON blobs carry the tuning knobs the catalog schema can't model.
        if let Some(json) = crate::storage::metadata::catalog_config::index_configs_to_json(config)?
        {
            schema
                .properties
                .insert("collection.index_config".to_string(), json);
        }
        if let Some(json) = crate::storage::metadata::catalog_config::quantization_to_json(config)?
        {
            schema
                .properties
                .insert("collection.quantization".to_string(), json);
        }
        // TD-122: persist the ProximaRecord schema config (enable flag, enforcement,
        // text columns) neutrally so get_collection_v2 echoes the schema/flags set
        // at create time.
        if let Some(json) = crate::storage::metadata::catalog_config::record_schema_to_json(config)?
        {
            schema
                .properties
                .insert("collection.record_schema".to_string(), json);
        }
        // Lossless round-trip: store the full serialized config so the catalog
        // asset never drops any collection field (the typed properties above stay
        // for pg_catalog introspection). This makes the catalog a complete,
        // sole-authority store for collection metadata.
        schema.properties.insert(
            "collection.config_json".to_string(),
            serde_json::to_string(config)
                .context("serializing collection config for catalog asset")?,
        );

        Ok(schema)
    }

    fn catalog_data_type(data_type: i32) -> ProximaType {
        use proximadb_data_model::TimeUnit;
        match FilterableDataType::try_from(data_type).ok() {
            Some(FilterableDataType::FilterableInteger) => ProximaType::Int64,
            Some(FilterableDataType::FilterableFloat) => ProximaType::Float64,
            Some(FilterableDataType::FilterableBoolean) => ProximaType::Boolean,
            Some(FilterableDataType::FilterableDatetime) => {
                ProximaType::Timestamp(TimeUnit::Nanosecond)
            }
            Some(FilterableDataType::FilterableDecimal) => ProximaType::Decimal {
                precision: 38,
                scale: 10,
            },
            Some(FilterableDataType::FilterableTimestampTz) => {
                ProximaType::TimestampTz(TimeUnit::Nanosecond)
            }
            Some(FilterableDataType::FilterableDate) => ProximaType::Date,
            Some(FilterableDataType::FilterableTime) => ProximaType::Time(TimeUnit::Nanosecond),
            Some(FilterableDataType::FilterableUuid) => ProximaType::Uuid,
            Some(FilterableDataType::FilterableBinary) => ProximaType::Binary,
            Some(FilterableDataType::FilterableJson)
            | Some(FilterableDataType::FilterableMapStringAny) => ProximaType::Json,
            _ => ProximaType::String,
        }
    }

    fn catalog_index_type(algorithm: i32) -> CatalogIndexType {
        match IndexingAlgorithm::try_from(algorithm).ok() {
            Some(IndexingAlgorithm::Hnsw) => CatalogIndexType::Hnsw,
            Some(IndexingAlgorithm::Ivf) => CatalogIndexType::Ivf,
            Some(IndexingAlgorithm::Pq) => CatalogIndexType::Pq,
            _ => CatalogIndexType::Hnsw,
        }
    }

    fn filterable_data_type(data_type: &ProximaType) -> i32 {
        match data_type {
            ProximaType::Int8 | ProximaType::Int16 | ProximaType::Int32 | ProximaType::Int64 => {
                FilterableDataType::FilterableInteger as i32
            }
            ProximaType::Float32 | ProximaType::Float64 => {
                FilterableDataType::FilterableFloat as i32
            }
            ProximaType::Boolean => FilterableDataType::FilterableBoolean as i32,
            ProximaType::Timestamp(_) => FilterableDataType::FilterableDatetime as i32,
            ProximaType::TimestampTz(_) => FilterableDataType::FilterableTimestampTz as i32,
            ProximaType::Decimal { .. } => FilterableDataType::FilterableDecimal as i32,
            ProximaType::Date => FilterableDataType::FilterableDate as i32,
            ProximaType::Time(_) => FilterableDataType::FilterableTime as i32,
            ProximaType::Uuid => FilterableDataType::FilterableUuid as i32,
            ProximaType::Binary => FilterableDataType::FilterableBinary as i32,
            ProximaType::Json => FilterableDataType::FilterableJson as i32,
            _ => FilterableDataType::FilterableString as i32,
        }
    }

    fn indexing_algorithm(index_type: CatalogIndexType) -> i32 {
        match index_type {
            CatalogIndexType::Ivf => IndexingAlgorithm::Ivf as i32,
            CatalogIndexType::Pq => IndexingAlgorithm::Pq as i32,
            CatalogIndexType::Hnsw => IndexingAlgorithm::Hnsw as i32,
            _ => IndexingAlgorithm::Hnsw as i32,
        }
    }

    fn storage_engine_from_catalog(engine: &str) -> i32 {
        match engine.to_ascii_uppercase().as_str() {
            "VIPER" => StorageEngine::Viper as i32,
            "NOVA" => StorageEngine::Nova as i32,
            "HELIX" => StorageEngine::Helix as i32,
            "SWIFT" => StorageEngine::Swift as i32,
            "RAPTOR" => StorageEngine::Raptor as i32,
            "MMAP" => StorageEngine::Mmap as i32,
            "HYBRID" => StorageEngine::Hybrid as i32,
            "TST" => StorageEngine::Tst as i32,
            "CEDAR" => StorageEngine::Cedar as i32,
            "TITAN" => StorageEngine::Titan as i32,
            "CHRONO" => StorageEngine::Chrono as i32,
            _ => StorageEngine::Sst as i32,
        }
    }

    fn is_not_found_error(error: &anyhow::Error) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        message.contains("not found") || message.contains("does not exist")
    }

    /// Generate unique collection ID using UUIDs.
    ///
    /// Base62 timestamp IDs are still accepted as legacy identifiers by lookup paths, but new
    /// catalog assets use UUID strings so identity is opaque, non-time-leaking, and compatible
    /// with catalog/schema UUID fields across SDKs and embedded mode.
    async fn generate_unique_collection_id(&self) -> Result<String> {
        for _ in 0..8 {
            let id = uuid::Uuid::new_v4().to_string();
            if self.collection_from_catalog_asset(&id).await?.is_none() {
                return Ok(id);
            }
        }

        Err(anyhow::anyhow!(
            "Unable to generate a unique UUID for collection"
        ))
    }
}

impl std::fmt::Debug for CollectionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectionService")
            .field("metadata_backend", &"UniversalMetadataBackend")
            .field("assignment_service", &"AssignmentService")
            .field("filesystem_factory", &"FilesystemFactory")
            .field("index_config_cache_info", &"HashMap<String, IndexConfig>")
            .finish()
    }
}

/// Response for collection operations - includes the full collection data
#[derive(Debug, Clone)]
pub struct CollectionServiceResponse {
    /// Whether the operation completed successfully.
    pub success: bool,
    /// The collection affected by the operation, if applicable (proto-first architecture).
    pub collection: Option<Collection>,
    /// Filesystem path where the collection's data is stored.
    pub storage_path: Option<String>,
    /// Machine-readable error code when the operation fails.
    pub error_code: Option<String>,
    /// Wall-clock time taken to process the request, in microseconds.
    pub processing_time_us: i64,
}

impl CollectionServiceResponse {
    /// Create success response
    pub fn success(
        _collection_uuid: String,
        storage_path: String,
        processing_time_us: i64,
    ) -> Self {
        Self {
            success: true,
            collection: None, // Collection should be passed in if needed
            storage_path: Some(storage_path),
            error_code: None,
            processing_time_us,
        }
    }

    /// Create success response with collection
    pub fn success_with_collection(
        collection: Collection,
        storage_path: String,
        processing_time_us: i64,
    ) -> Self {
        Self {
            success: true,
            collection: Some(collection),
            storage_path: Some(storage_path),
            error_code: None,
            processing_time_us,
        }
    }

    /// Create error response
    pub fn error(error_code: String, processing_time_us: i64) -> Self {
        Self {
            success: false,
            collection: None,
            storage_path: None,
            error_code: Some(error_code),
            processing_time_us,
        }
    }
}

/// Builder for collection service with dependencies
pub struct CollectionServiceBuilder {
    /// Optional metadata backend to set during construction
    metadata_backend: Option<Arc<dyn InternalCollectionProvider>>,
    /// Optional storage configuration to set during construction
    storage_config: Option<StorageConfig>,
}

impl CollectionServiceBuilder {
    /// Create a new builder with no dependencies configured.
    pub fn new() -> Self {
        Self {
            metadata_backend: None,
            storage_config: None,
        }
    }

    /// Set the metadata backend used for collection persistence.
    pub fn with_metadata_backend(mut self, backend: Arc<dyn InternalCollectionProvider>) -> Self {
        self.metadata_backend = Some(backend);
        self
    }

    /// Set the storage configuration (data paths, engine settings, etc.).
    pub fn with_storage_config(mut self, config: StorageConfig) -> Self {
        self.storage_config = Some(config);
        self
    }

    /// Consume the builder and construct a [`CollectionService`].
    ///
    /// Returns an error if the required metadata backend has not been provided.
    pub async fn build(self) -> Result<CollectionService> {
        let metadata_backend = self
            .metadata_backend
            .ok_or_else(|| anyhow::anyhow!("Metadata backend is required"))?;

        let storage_config = self.storage_config.unwrap_or_default();

        CollectionService::new(metadata_backend, storage_config).await
    }
}

impl Default for CollectionServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::CollectionConfig;

    #[tokio::test]
    async fn test_collection_validation() -> Result<()> {
        // Use filestore backend with temporary directory for testing
        use crate::storage::metadata::backends::universal_backend::{
            UniversalMetadataBackend, UniversalMetadataConfig,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().context("Failed to create temporary directory for test")?;
        let temp_path = format!("file://{}", temp_dir.path().display());

        let filestore_config = UniversalMetadataConfig {
            storage_url: temp_path.clone(),
            compression: false,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::create(filesystem_config)
                .await
                .context("Failed to create filesystem factory for test")?,
        );

        let backend = Arc::new(
            UniversalMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .context("Failed to create metadata backend for test")?,
        );

        let storage_config = StorageConfig::default();
        let service = CollectionService::new(backend, storage_config)
            .await
            .context("Failed to create collection service for test")?;

        // Valid config
        let valid_config = CollectionConfig {
            name: "valid_collection".to_string(),
            dimension: 128,
            distance_metric: Some(1),
            storage_engine: Some(1),
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            primary_index: Some("default".to_string()),
            auto_index_selection: Some(false),
            storage_config: None,
            description: Some("Test collection".to_string()),
            tags: vec![],
            owner: Some("test".to_string()),
            embedding_models: vec![],
            record_schema: None,
            enable_proxima_record: None,
            text_columns: vec![],
            text_storage_configs: vec![],
            enable_dual_use_embeddings: None,
            canonical_embedding_precision: None,
        };

        // Test create with valid config
        let result = service
            .create_collection(&valid_config)
            .await
            .context("Failed to create valid collection")?;
        assert!(result.success);

        // Test empty name
        let empty_name = CollectionConfig {
            name: "".to_string(),
            ..valid_config.clone()
        };
        let result = service
            .create_collection(&empty_name)
            .await
            .context("Failed to create collection with empty name")?;
        assert!(!result.success);
        assert!(
            result
                .error_code
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Error code missing"))?
                .contains("INVALID_NAME"),
            "Error code should contain INVALID_NAME, got: {:?}",
            result.error_code
        );

        // Short names are valid SQL/ANSI identifiers — the vestigial 8-char floor was
        // removed (TPC-H `part`/`orders` etc. need short table names). "short" now succeeds.
        let short_name = CollectionConfig {
            name: "short".to_string(),
            ..valid_config.clone()
        };
        let result = service
            .create_collection(&short_name)
            .await
            .context("Failed to create collection with short name")?;
        assert!(
            result.success,
            "short names are now valid; got error: {:?}",
            result.error_code
        );

        // Test exactly 8 characters (should pass)
        let eight_chars = CollectionConfig {
            name: "exactly8".to_string(),
            ..valid_config.clone()
        };
        let result = service
            .create_collection(&eight_chars)
            .await
            .context("Failed to create collection with 8-character name")?;
        assert!(result.success);

        // Test invalid dimension
        let invalid_dimension = CollectionConfig {
            name: "valid_dimension_test".to_string(),
            dimension: 0,
            ..valid_config.clone()
        };
        let result = service
            .create_collection(&invalid_dimension)
            .await
            .context("Failed to create collection with invalid dimension")?;
        assert!(!result.success);
        assert!(
            result
                .error_code
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Error code missing"))?
                .contains("INVALID_DIMENSION"),
            "Error code should contain INVALID_DIMENSION, got: {:?}",
            result.error_code
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_name_length_validation() -> Result<()> {
        // Create a minimal test setup
        use crate::storage::metadata::backends::universal_backend::{
            UniversalMetadataBackend, UniversalMetadataConfig,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().context("Failed to create temporary directory for test")?;
        let temp_path = format!("file://{}", temp_dir.path().display());

        let filestore_config = UniversalMetadataConfig {
            storage_url: temp_path.clone(),
            compression: false,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::create(filesystem_config)
                .await
                .context("Failed to create filesystem factory for test")?,
        );

        let backend = Arc::new(
            UniversalMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .context("Failed to create metadata backend for test")?,
        );

        let storage_config = StorageConfig::default();
        let service = CollectionService::new(backend, storage_config)
            .await
            .context("Failed to create collection service for test")?;

        // Test cases for collection name length
        let test_cases = vec![
            ("", false, "INVALID_NAME"), // Empty name
            // Short names are valid SQL/ANSI identifiers (no artificial 8-char floor);
            // required for relational tables over pgwire (TPC-H `part`, `orders`, ...).
            ("a", true, ""),                              // 1 char
            ("abc", true, ""),                            // 3 chars
            ("seven77", true, ""),                        // 7 chars
            ("exactly8", true, ""),                       // 8 chars (valid)
            ("ninechars", true, ""),                      // 9 chars (valid)
            ("this_is_a_long_collection_name", true, ""), // Long name (valid)
        ];

        for (name, should_succeed, expected_error_code) in test_cases {
            let config = CollectionConfig {
                name: name.to_string(),
                dimension: 128,
                distance_metric: Some(1),
                storage_engine: Some(1),
                filterable_columns: vec![],
                index_configs: vec![],
                quantization: None,
                primary_index: Some("default".to_string()),
                auto_index_selection: Some(false),
                description: Some("Test collection".to_string()),
                tags: vec![],
                owner: Some("test".to_string()),
                embedding_models: vec![],
                storage_config: None,
                record_schema: None,
                enable_proxima_record: None,
                text_columns: vec![],
                text_storage_configs: vec![],
                enable_dual_use_embeddings: None,
                canonical_embedding_precision: None,
            };

            let result = service
                .create_collection(&config)
                .await
                .context(format!("Failed to create collection with name '{}'", name))?;

            assert_eq!(
                result.success, should_succeed,
                "Name '{}' validation failed: expected success={}, got success={}",
                name, should_succeed, result.success
            );

            if !should_succeed {
                assert!(
                    result
                        .error_code
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Error code missing for name '{}'", name))?
                        .contains(expected_error_code),
                    "Name '{}' error code mismatch: expected to contain '{}', got '{:?}'",
                    name,
                    expected_error_code,
                    result.error_code
                );
            }
        }

        Ok(())
    }

    /// TDD: short SQL/ANSI table names (e.g. TPC-H `part`) create successfully,
    /// resolve to a DISTINCT opaque UUID id (not the name), and round-trip
    /// name<->id. Proves the redesign: name and id are separate, unambiguous
    /// namespaces with NO length dependence (the old 8-char floor is gone), and
    /// resolution is name-authoritative.
    #[tokio::test]
    async fn test_short_name_resolves_name_authoritative() -> Result<()> {
        use crate::storage::metadata::backends::universal_backend::{
            UniversalMetadataBackend, UniversalMetadataConfig,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().context("temp dir")?;
        let temp_path = format!("file://{}", temp_dir.path().display());
        let filestore_config = UniversalMetadataConfig {
            storage_url: temp_path.clone(),
            compression: false,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };
        let filesystem_factory = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .context("fs factory")?,
        );
        let backend = Arc::new(
            UniversalMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .context("metadata backend")?,
        );
        let catalog_manager = Arc::new(CatalogManager::new());
        catalog_manager
            .create_native_catalog("default", &temp_path)
            .await
            .context("Failed to create test xCatalog")?;
        let service = CollectionService::new(backend, StorageConfig::default())
            .await
            .context("collection service")?
            .with_catalog_manager(catalog_manager.clone());

        // A short, standard SQL identifier (4 chars) — would have been rejected by
        // the old 8-char floor.
        let name = "part";
        let config = CollectionConfig {
            name: name.to_string(),
            dimension: 16,
            distance_metric: Some(1),
            storage_engine: Some(1),
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            primary_index: Some("default".to_string()),
            auto_index_selection: Some(false),
            description: None,
            tags: vec![],
            owner: None,
            embedding_models: vec![],
            storage_config: None,
            record_schema: None,
            enable_proxima_record: None,
            text_columns: vec![],
            text_storage_configs: vec![],
            enable_dual_use_embeddings: None,
            canonical_embedding_precision: None,
        };
        let created = service.create_collection(&config).await.context("create")?;
        assert!(
            created.success,
            "short name should create: {:?}",
            created.error_code
        );

        // Resolve name -> id: must yield an opaque id DISTINCT from the name.
        let id = service
            .resolve_collection_id(name)
            .await
            .context("resolve id")?
            .ok_or_else(|| anyhow::anyhow!("name did not resolve to an id"))?;
        assert_ne!(id, name, "id must be opaque, not the name");

        // Round-trip id -> name.
        let back = service
            .resolve_collection_name(&id)
            .await
            .context("resolve name")?
            .ok_or_else(|| anyhow::anyhow!("id did not resolve back to a name"))?;
        assert_eq!(back, name, "id must round-trip to the original name");

        Ok(())
    }

    #[tokio::test]
    async fn test_create_collection_persists_explicit_cosine_metric() -> Result<()> {
        use crate::storage::metadata::backends::universal_backend::{
            UniversalMetadataBackend, UniversalMetadataConfig,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().context("Failed to create temporary directory for test")?;
        let temp_path = format!("file://{}", temp_dir.path().display());

        let filestore_config = UniversalMetadataConfig {
            storage_url: temp_path.clone(),
            compression: false,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .context("Failed to create filesystem factory for test")?,
        );
        let backend = Arc::new(
            UniversalMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .context("Failed to create metadata backend for test")?,
        );
        let catalog_manager = Arc::new(CatalogManager::new());
        catalog_manager
            .create_native_catalog("default", &temp_path)
            .await
            .context("Failed to create test xCatalog")?;
        let service = CollectionService::new(backend, StorageConfig::default())
            .await
            .context("Failed to create collection service for test")?
            .with_catalog_manager(catalog_manager.clone());

        let config = CollectionConfig {
            name: "metric_default_test".to_string(),
            dimension: 128,
            distance_metric: None,
            storage_engine: Some(StorageEngine::Viper as i32),
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            primary_index: None,
            auto_index_selection: Some(false),
            storage_config: None,
            description: Some("Test collection".to_string()),
            tags: vec![],
            owner: Some("test".to_string()),
            embedding_models: vec![],
            record_schema: None,
            enable_proxima_record: None,
            text_columns: vec![],
            text_storage_configs: vec![],
            enable_dual_use_embeddings: None,
            canonical_embedding_precision: None,
        };

        let result = service.create_collection(&config).await?;
        assert!(
            result.success,
            "create failed with error_code={:?}",
            result.error_code
        );

        let stored = service
            .collection("metric_default_test")
            .await?
            .expect("collection should exist");
        assert_eq!(
            stored.config.as_ref().and_then(|cfg| cfg.distance_metric),
            Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32)
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_create_collection_preserves_exact_default_without_indexes() -> Result<()> {
        use crate::storage::metadata::backends::universal_backend::{
            UniversalMetadataBackend, UniversalMetadataConfig,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().context("Failed to create temporary directory for test")?;
        let temp_path = format!("file://{}", temp_dir.path().display());

        let filestore_config = UniversalMetadataConfig {
            storage_url: temp_path.clone(),
            compression: false,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .context("Failed to create filesystem factory for test")?,
        );
        let backend = Arc::new(
            UniversalMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .context("Failed to create metadata backend for test")?,
        );
        let catalog_manager = Arc::new(CatalogManager::new());
        catalog_manager
            .create_native_catalog("default", &temp_path)
            .await
            .context("Failed to create test xCatalog")?;
        let service = CollectionService::new(backend, StorageConfig::default())
            .await
            .context("Failed to create collection service for test")?
            .with_catalog_manager(catalog_manager.clone());

        let config = CollectionConfig {
            name: "exact_default_case".to_string(),
            dimension: 384,
            storage_engine: Some(StorageEngine::Sst as i32),
            index_configs: vec![],
            auto_index_selection: Some(false),
            ..Default::default()
        };

        let result = service.create_collection(&config).await?;
        assert!(
            result.success,
            "create failed with error_code={:?}",
            result.error_code
        );

        let stored = service
            .collection("exact_default_case")
            .await?
            .expect("collection should exist");
        assert!(
            stored
                .config
                .as_ref()
                .is_some_and(|cfg| cfg.index_configs.is_empty())
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_lifecycle_mirrors_to_xcatalog_with_uuid_id() -> Result<()> {
        use crate::storage::metadata::backends::universal_backend::{
            UniversalMetadataBackend, UniversalMetadataConfig,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().context("Failed to create temporary directory for test")?;
        let temp_path = format!("file://{}", temp_dir.path().display());

        let filestore_config = UniversalMetadataConfig {
            storage_url: temp_path.clone(),
            compression: false,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .context("Failed to create filesystem factory for test")?,
        );
        let backend = Arc::new(
            UniversalMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .context("Failed to create metadata backend for test")?,
        );

        let catalog_manager = Arc::new(CatalogManager::new());
        catalog_manager
            .create_native_catalog("default", &temp_path)
            .await
            .context("Failed to create test xCatalog")?;

        let service = CollectionService::new(backend, StorageConfig::default())
            .await
            .context("Failed to create collection service for test")?
            .with_catalog_manager(catalog_manager.clone());

        let config = CollectionConfig {
            name: "catalog_vector_assets".to_string(),
            dimension: 384,
            storage_engine: Some(StorageEngine::Sst as i32),
            filterable_columns: vec![crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "category".to_string(),
                data_type: FilterableDataType::FilterableString as i32,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(32),
            }],
            index_configs: vec![crate::proto::proximadb_v1::IndexConfig {
                index_name: "catalog_vector_assets_hnsw".to_string(),
                algorithm: IndexingAlgorithm::Hnsw as i32,
                enabled: Some(true),
                ..Default::default()
            }],
            auto_index_selection: Some(false),
            ..Default::default()
        };

        let result = service.create_collection(&config).await?;
        assert!(
            result.success,
            "create failed with error_code={:?}",
            result.error_code
        );
        let collection = result.collection.expect("collection should be returned");
        assert!(uuid::Uuid::parse_str(&collection.id).is_ok());

        let catalog = catalog_manager.default_catalog().await?;
        let table_id = TableIdentifier::new(
            vec!["default".to_string()],
            "catalog_vector_assets".to_string(),
        );
        let schema = catalog.get_table(&table_id).await?;
        assert_eq!(schema.properties.get("collection.id"), Some(&collection.id));
        assert_eq!(
            schema.properties.get("asset.capability.vector"),
            Some(&"true".to_string())
        );
        assert!(
            schema
                .columns
                .iter()
                .any(|column| column.name == "category")
        );
        assert_eq!(schema.projections.len(), 1);

        service
            .metadata_backend()
            .delete_collection("catalog_vector_assets")
            .await?;

        let catalog_backed_by_name = service
            .collection("catalog_vector_assets")
            .await?
            .expect("collection should be reconstructed from xCatalog by name");
        assert_eq!(catalog_backed_by_name.id, collection.id);
        let catalog_backed_by_id = service
            .collection(&collection.id)
            .await?
            .expect("collection should be reconstructed from xCatalog by UUID");
        assert_eq!(
            catalog_backed_by_id
                .config
                .as_ref()
                .map(|config| config.dimension),
            Some(384)
        );
        assert!(catalog_backed_by_id.config.as_ref().is_some_and(|config| {
            config.filterable_columns.iter().any(|column| {
                column.name == "category"
                    && column.data_type == FilterableDataType::FilterableString as i32
            })
        }));
        assert!(
            service
                .list_collections()
                .await?
                .iter()
                .any(|listed| listed.id == collection.id)
        );

        let duplicate = service.create_collection(&config).await?;
        assert!(!duplicate.success);
        assert_eq!(duplicate.error_code.as_deref(), Some("COLLECTION_EXISTS"));

        let delete = service.delete_collection("catalog_vector_assets").await?;
        assert!(delete.success);
        assert!(!catalog.table_exists(&table_id).await?);

        Ok(())
    }

    #[test]
    fn test_response_conversion() {
        let response = CollectionServiceResponse::success(
            "test-uuid".to_string(),
            "/path/to/storage".to_string(),
            1000,
        );

        assert!(response.success);
        assert_eq!(response.processing_time_us, 1000);
    }

    /// TD-122: the detailed per-index (HNSW m/ef, IVF n_lists/n_probe,
    /// is_primary) and quantization (enabled, strategy) config must survive a
    /// round-trip through the read-authoritative xCatalog table asset. Before
    /// the fix this reconstruction returned `m=0`, `is_primary=false`, and
    /// quantization disabled.
    #[test]
    fn catalog_asset_round_trips_detailed_index_and_quant_config() {
        use crate::proto::proximadb_v1::{
            Collection, HnswConfig, IndexConfig, IvfConfig, QuantizationConfig, StorageAssignment,
            quantization_config::Strategy,
        };

        let collection = Collection {
            id: "col-td122".to_string(),
            config: Some(CollectionConfig {
                name: "td122_round_trip".to_string(),
                dimension: 128,
                index_configs: vec![IndexConfig {
                    index_name: "primary_hnsw".to_string(),
                    hnsw_config: Some(HnswConfig {
                        m: Some(24),
                        ef_construction: Some(150),
                        ef_search: Some(64),
                        ..Default::default()
                    }),
                    ivf_config: Some(IvfConfig {
                        n_lists: Some(256),
                        n_probe: Some(16),
                        ..Default::default()
                    }),
                    is_primary: Some(true),
                    ..Default::default()
                }],
                quantization: Some(QuantizationConfig {
                    enabled: Some(true),
                    strategy: Some(Strategy::Aggressive as i32),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                primary_path: "file:///tmp/td122".to_string(),
                base_location: "file:///tmp/td122".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let schema = CollectionService::catalog_schema_from_collection(&collection)
            .expect("schema from collection");
        let identifier = CollectionService::collection_table_identifier(
            collection.config.as_ref().expect("config"),
        );
        let restored = CollectionService::collection_from_catalog_schema(&identifier, &schema)
            .expect("collection from schema")
            .expect("collection present");

        let config = restored.config.expect("restored config");
        assert_eq!(config.index_configs.len(), 1, "one ANN index retained");
        let ic = &config.index_configs[0];
        assert_eq!(ic.index_name, "primary_hnsw");
        assert_eq!(ic.is_primary, Some(true));
        let hnsw = ic.hnsw_config.as_ref().expect("hnsw config restored");
        assert_eq!(hnsw.m, Some(24));
        assert_eq!(hnsw.ef_construction, Some(150));
        assert_eq!(hnsw.ef_search, Some(64));
        let ivf = ic.ivf_config.as_ref().expect("ivf config restored");
        assert_eq!(ivf.n_lists, Some(256));
        assert_eq!(ivf.n_probe, Some(16));
        let quant = config.quantization.as_ref().expect("quantization restored");
        assert_eq!(quant.enabled, Some(true));
        assert_eq!(quant.strategy, Some(Strategy::Aggressive as i32));
    }
}

// CollectionService does NOT implement MetadataProvider - it USES a MetadataProvider backend!
// The backend (LocalRocksDbBackend or UniversalMetadataBackend) implements MetadataProvider.
// CollectionService can implement InternalCollectionProvider if needed for backward compatibility,
// but it delegates to its metadata_backend which is the actual MetadataProvider.

// ── CollectionPort impl ───────────────────────────────────────────────────────

#[async_trait::async_trait]
impl proximadb_runtime::CollectionPort for CollectionService {
    async fn get_collection(
        &self,
        identifier: &str,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<Option<crate::proto::proximadb_v1::Collection>> {
        let ctx = self.load_tenant_context(tenant_id)?;
        self.get_collection_with_tenant_context(identifier, ctx.as_ref())
            .await
    }

    async fn create_collection(
        &self,
        config: crate::proto::proximadb_v1::CollectionConfig,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<crate::proto::proximadb_v1::Collection> {
        let ctx = self.load_tenant_context(tenant_id)?;
        let resp = self
            .create_collection_with_tenant_context(&config, ctx.as_ref())
            .await?;
        resp.collection.ok_or_else(|| {
            anyhow::anyhow!(
                "create_collection returned no collection: error_code={:?}",
                resp.error_code
            )
        })
    }

    async fn update_collection(
        &self,
        id: &str,
        config: crate::proto::proximadb_v1::CollectionConfig,
        _tenant_id: Option<&str>,
    ) -> anyhow::Result<crate::proto::proximadb_v1::Collection> {
        let resp = CollectionService::update_collection(self, id, Some(config)).await?;
        resp.collection.ok_or_else(|| {
            anyhow::anyhow!(
                "update_collection returned no collection: error_code={:?}",
                resp.error_code
            )
        })
    }

    async fn delete_collection(&self, id: &str, tenant_id: Option<&str>) -> anyhow::Result<bool> {
        let ctx = self.load_tenant_context(tenant_id)?;
        let resp = self
            .delete_collection_with_tenant_context(id, ctx.as_ref())
            .await?;
        Ok(resp.success)
    }

    async fn list_collections(
        &self,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::proto::proximadb_v1::Collection>> {
        let ctx = self.load_tenant_context(tenant_id)?;
        self.list_collections_with_tenant_context(ctx.as_ref())
            .await
    }

    async fn resolve_collection_id(&self, identifier: &str) -> anyhow::Result<Option<String>> {
        CollectionService::resolve_collection_id(self, identifier).await
    }
}
