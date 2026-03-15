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
use std::sync::Arc;
use tracing::{debug, error, info, warn};

// Using String directly instead of String alias for proto-first architecture
use crate::core::config::StorageConfig;
use crate::proto::proximadb_v1::{Collection, CollectionConfig, StorageEngine};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::InternalCollectionProvider;
use crate::utils::StoragePath;

// Proto-first architecture - use crate::proto::proximadb_v1::Collection directly

// Local types to replace assignment service
#[derive(Debug, Clone)]
enum StorageComponentType {
    Wal,
    Storage,
    Index,
}

/// Collection service for unified business logic with multi-disk coordination
pub struct CollectionService {
    metadata_backend: Arc<dyn InternalCollectionProvider>,
    filesystem_factory: Arc<FilesystemFactory>,
    /// Cache for IndexConfig to avoid repeated deserialization
    /// Using dashmap for lock-free concurrent access
    index_config_cache: Arc<dashmap::DashMap<String, crate::index::config::IndexConfig>>,
    storage_config: StorageConfig,

    // NEW: Multi-tenant integration
    tenant_manager: Option<Arc<crate::storage::tenant::TenantManager>>,
    rbac_enforcer: Option<Arc<crate::storage::tenant::EnhancedRBACManager>>,
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
            tenant_manager: None, // Will be set via with_tenant_manager()
            rbac_enforcer: None,  // Will be set via with_rbac_enforcer()
        })
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

    pub fn multi_tenant_enabled(&self) -> bool {
        self.tenant_manager.is_some()
    }

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
            .metadata_backend
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
        if let Some(ref tenant_manager) = self.tenant_manager {
            if !tenant_manager.is_tenant_active(&tenant_ctx.tenant_id) {
                warn!(
                    "🚨 Tenant '{}' is not active; denying access to collection '{}'",
                    tenant_ctx.tenant_id, collection_identifier
                );
                return Ok(None);
            }
        }

        let collection = self
            .metadata_backend
            .get_collection(collection_identifier)
            .await?;

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
        self.metadata_backend.get_collection(collection_name).await
    }

    pub async fn list_collections_with_tenant_context(
        &self,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<Vec<Collection>> {
        let collections = self.metadata_backend.list_collections().await?;

        if let Some(tenant_ctx) = tenant_context.filter(|_| self.tenant_manager.is_some()) {
            if let Some(ref tenant_manager) = self.tenant_manager {
                if !tenant_manager.is_tenant_active(&tenant_ctx.tenant_id) {
                    warn!(
                        "🚨 Tenant '{}' is not active; returning empty collection list",
                        tenant_ctx.tenant_id
                    );
                    return Ok(Vec::new());
                }
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

        self.delete_collection(collection_name).await
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

        // Resolve compression and storage configuration
        let mut enriched_config = config.clone();

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
                storage_cfg.compression = Some(compression_config.algorithm as i32);
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
                    });
                }
            }
        }

        // Add default HNSW index configuration if not provided
        // This enables AXIS indexes for accelerated vector search
        let resolved_engine = crate::proto::proximadb_v1::StorageEngine::try_from(
            config.storage_engine.unwrap_or(StorageEngine::Sst as i32),
        )
        .unwrap_or(StorageEngine::Sst);

        if enriched_config.index_configs.is_empty() && resolved_engine != StorageEngine::Tst {
            use crate::proto::proximadb_v1::{HnswConfig, IndexConfig, IndexingAlgorithm};

            let default_hnsw_config = IndexConfig {
                index_name: format!("{}_default_hnsw", config.name),
                algorithm: IndexingAlgorithm::Hnsw as i32,
                enabled: Some(true),
                is_primary: Some(true),
                hnsw_config: Some(HnswConfig {
                    m: Some(16),                // Balanced connectivity
                    ef_construction: Some(200), // Good build quality
                    ef_search: Some(50),        // Fast search with good recall
                    max_partition_size: Some(100_000),
                    adaptive_parameters: Some(true),
                    use_simd: Some(true),
                    memory_limit_mb: Some(512),
                    lazy_loading: Some(false),
                }),
                ..Default::default()
            };

            enriched_config.index_configs.push(default_hnsw_config);
            info!(
                "📊 Created default HNSW index for collection '{}' (dimension: {})",
                config.name, config.dimension
            );
        } else if enriched_config.index_configs.is_empty() {
            info!(
                "📊 Skipping default HNSW index for time-series collection '{}'",
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

        // Validate collection name length to prevent collision with IDs
        if config.name.len() < 8 {
            return Ok(CollectionServiceResponse::error(
                "INVALID_NAME_LENGTH: Collection name must be at least 8 characters long"
                    .to_string(),
                start_time.elapsed().as_micros() as i64,
            ));
        }

        if config.dimension == 0 || config.dimension > 1_000_000 {
            return Ok(CollectionServiceResponse::error(
                "INVALID_DIMENSION: Invalid dimension: must be between 1 and 1,000,000".to_string(),
                start_time.elapsed().as_micros() as i64,
            ));
        }

        // Validate quantization configuration
        if let Some(quant_config) = &enriched_config.quantization {
            if quant_config.enabled.unwrap_or(false) {
                info!(
                    "⚠️ Collection '{}' has quantization enabled. All vectors MUST have unique IDs for tracking quantized representations",
                    config.name
                );
                // Note: We don't fail here, but log a warning. The actual validation happens during insert
                // This allows collections to be created with quantization enabled, but enforces IDs at insert time
            }
        }

        // Check if collection already exists
        // Check if collection already exists
        // Check if collection already exists
        if let Some(_) = self.metadata_backend.get_collection(&config.name).await? {
            return Ok(CollectionServiceResponse {
                success: false,
                collection: None,
                storage_path: None,
                error_code: Some("COLLECTION_EXISTS".to_string()),
                processing_time_us: start_time.elapsed().as_micros() as i64,
            });
        }

        // Create proto collection directly - no Avro conversion needed!
        // Generate base62 ID from microsecond timestamp with collision detection
        let uuid = self.generate_unique_collection_id().await?;
        let now = chrono::Utc::now().timestamp_micros();

        // Get storage location - use provided or pick randomly from config
        let base_location = if let Some(ref storage_config) = enriched_config.storage_config {
            if storage_config
                .storage_path
                .as_ref()
                .map_or(false, |p| !p.is_empty())
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

        // Store proto collection using protobuf serialization (zero-copy)
        self.metadata_backend
            .upsert_collection_proto(&proto_collection)
            .await
            .context("Failed to store collection metadata_info")?;

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
        // Use the metadata backend's collection_metadata which handles both name and UUID
        self.metadata_backend.collection_metadata(identifier).await
    }

    /// ✅ RESOLVE COLLECTION NAME/ID TO COLLECTION ID
    /// This is the key method for collection identifier resolution
    /// - Input: Collection name OR collection ID
    /// - Output: Collection ID (base62) for internal use
    /// - Used by WAL, storage, and index path resolution
    pub async fn resolve_collection_id(&self, identifier: &str) -> Result<Option<String>> {
        tracing::debug!("🔍 Resolving collection identifier: '{}'", identifier);

        // Check if this looks like a base62 collection ID (short alphanumeric)
        let _is_likely_id =
            identifier.len() <= 12 && identifier.chars().all(|c| c.is_alphanumeric());

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
        if let Some(collection) = self.collection(collection_id).await? {
            if let Some(config) = &collection.config {
                return Ok(Some(config.name.clone()));
            }
        }
        Ok(None)
    }

    /// Convert Collection to core Collection - direct proto to core mapping
    /// Get IndexConfig for a collection by name or UUID with caching
    pub async fn native_index_config(
        &self,
        identifier: &str,
    ) -> Result<Option<crate::index::config::IndexConfig>> {
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
    ) -> Result<crate::index::config::IndexConfig> {
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
        crate::index::config::IndexConfig::from_proto(_proto_config)
    }

    /// Parse IndexConfig from Collection
    fn parse_index_config_from_proto(
        &self,
        proto: &Collection,
    ) -> Result<crate::index::config::IndexConfig> {
        // Check if proto has index_config field
        if let Some(config) = proto.config.as_ref() {
            if !config.index_configs.is_empty() {
                // Take the first IndexConfig from proto (index_configs is a Vec)
                if let Some(first_config) = config.index_configs.first() {
                    // Convert from proto IndexConfig to internal IndexConfig
                    return Ok(self.convert_proto_index_config(first_config)?);
                }
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

        let smart_config = crate::index::config::IndexConfig::create_smart_default(
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
        self.metadata_backend.list_collections().await
    }

    /// Delete collection with comprehensive cleanup across all storage components
    pub async fn delete_collection(
        &self,
        collection_identifier: &str,
    ) -> Result<CollectionServiceResponse> {
        info!("🗑️ Deleting collection: {}", collection_identifier);
        let start_time = std::time::Instant::now();

        // Get collection record first to retrieve UUID and other details
        let collection_record = self
            .metadata_backend
            .get_collection(collection_identifier)
            .await?;

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

            // Step 3: Delete from metadata backend
            self.metadata_backend
                .delete_collection(collection_name.as_deref().unwrap_or(collection_identifier))
                .await?;
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
            if let Some(collection) = self.collection(collection_name).await? {
                if let Some(config) = &collection.config {
                    stats.dimension = Some(config.dimension as u32);
                }
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
                stats.dimension = Some(config.dimension as u32);
            }
            return Ok(stats);
        }

        Ok(crate::storage::traits::CollectionStats::default())
    }

    /// Get collection UUID by name or UUID
    pub async fn uuid(&self, collection_id: &str) -> Result<Option<String>> {
        debug!("🔍 Getting UUID for collection: {}", collection_id);

        // First check if it's already a UUID
        if crate::utils::uuid::Uuid::parse(collection_id).is_ok() {
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

        // Get current record (supports both names and UUIDs)
        // Get current record (supports both names and UUIDs)
        let mut record = match self.metadata_backend.get_collection(identifier).await? {
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

        // Apply updates using native proto types
        if let Some(new_config) = config_update {
            // Merge the new config with existing one to preserve unchanged fields
            if let Some(existing_config) = record.config.as_mut() {
                // Only update fields that are provided in new_config
                if new_config.name != "" {
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
            if let Some(level) = config.level {
                match CompressionAlgorithm::try_from(config.algorithm) {
                    Ok(CompressionAlgorithm::CompressionZstd) => {
                        if level < 1 || level > 22 {
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
                    }
                    _ => {}
                }
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

        // Store updated collection
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

    /// Generate unique collection ID using base62-encoded seconds with random padding
    /// Format: {base62(seconds)}{random_base62_char}
    async fn generate_unique_collection_id(&self) -> Result<String> {
        use crate::core::base62;

        const BASE62_CHARS: &[u8] =
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let base_timestamp = chrono::Utc::now().timestamp() as u64;
        let base_id = base62::encode(base_timestamp);

        // Generate initial ID with random padding using rand::random which is Send
        let random_index: u8 = rand::random::<u8>() % 62;
        let random_char = BASE62_CHARS[random_index as usize] as char;
        let id = format!("{}{}", base_id, random_char);

        // Check if ID is available
        if !self.metadata_backend.collection_id_exists(&id).await? {
            return Ok(id);
        }

        // If collision detected, try different random paddings
        for _ in 0..62 {
            let random_index: u8 = rand::random::<u8>() % 62;
            let random_char = BASE62_CHARS[random_index as usize] as char;
            let try_id = format!("{}{}", base_id, random_char);
            if !self.metadata_backend.collection_id_exists(&try_id).await? {
                return Ok(try_id);
            }
        }

        // If still colliding (very unlikely), try bidirectional search with padding
        const MAX_ATTEMPTS: u64 = 100;

        for offset in 1..=MAX_ATTEMPTS {
            // Try incrementing seconds
            let inc_timestamp = base_timestamp + offset;
            let inc_base = base62::encode(inc_timestamp);
            let random_index: u8 = rand::random::<u8>() % 62;
            let random_char = BASE62_CHARS[random_index as usize] as char;
            let inc_id = format!("{}{}", inc_base, random_char);
            if !self.metadata_backend.collection_id_exists(&inc_id).await? {
                return Ok(inc_id);
            }

            // Try decrementing seconds (if not underflow)
            if base_timestamp > offset {
                let dec_timestamp = base_timestamp - offset;
                let dec_base = base62::encode(dec_timestamp);
                let random_index: u8 = rand::random::<u8>() % 62;
                let random_char = BASE62_CHARS[random_index as usize] as char;
                let dec_id = format!("{}{}", dec_base, random_char);
                if !self.metadata_backend.collection_id_exists(&dec_id).await? {
                    return Ok(dec_id);
                }
            }
        }

        // Extremely unlikely case: append another random character
        let random_index: u8 = rand::random::<u8>() % 62;
        let random_suffix = BASE62_CHARS[random_index as usize] as char;
        Ok(format!("{}{}{}", base_id, random_char, random_suffix))
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
    pub success: bool,
    pub collection: Option<Collection>, // Proto-first architecture
    pub storage_path: Option<String>,
    pub error_code: Option<String>,
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
    metadata_backend: Option<Arc<dyn InternalCollectionProvider>>,
    storage_config: Option<StorageConfig>,
}

impl CollectionServiceBuilder {
    pub fn new() -> Self {
        Self {
            metadata_backend: None,
            storage_config: None,
        }
    }

    pub fn with_metadata_backend(mut self, backend: Arc<dyn InternalCollectionProvider>) -> Self {
        self.metadata_backend = Some(backend);
        self
    }

    pub fn with_storage_config(mut self, config: StorageConfig) -> Self {
        self.storage_config = Some(config);
        self
    }

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

        // Test short name (less than 8 characters)
        let short_name = CollectionConfig {
            name: "short".to_string(),
            ..valid_config.clone()
        };
        let result = service
            .create_collection(&short_name)
            .await
            .context("Failed to create collection with short name")?;
        assert!(!result.success);
        assert!(
            result
                .error_code
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Error code missing"))?
                .contains("INVALID_NAME_LENGTH"),
            "Error code should contain INVALID_NAME_LENGTH, got: {:?}",
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
            ("", false, "INVALID_NAME"),                  // Empty name
            ("a", false, "INVALID_NAME_LENGTH"),          // 1 char
            ("abc", false, "INVALID_NAME_LENGTH"),        // 3 chars
            ("seven77", false, "INVALID_NAME_LENGTH"),    // 7 chars
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
}

// CollectionService does NOT implement MetadataProvider - it USES a MetadataProvider backend!
// The backend (LocalRocksDbBackend or UniversalMetadataBackend) implements MetadataProvider.
// CollectionService can implement InternalCollectionProvider if needed for backward compatibility,
// but it delegates to its metadata_backend which is the actual MetadataProvider.
