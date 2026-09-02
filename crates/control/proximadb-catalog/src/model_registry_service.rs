//! Tenant-scoped application service for embedding-model registry lifecycle.
//!
//! Transport adapters (native REST/gRPC, MLflow compatibility, SDKs) lower to
//! this service instead of manipulating whole catalog documents. The service
//! owns tenant scoping, deterministic listing, command-shaped mutation, and
//! alias-to-immutable binding resolution; authorization and audit identity stay
//! at the transport/control-plane boundary.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::mlops::{
    CatalogEmbeddingModelRegistry, CatalogMlopsAsset, CatalogModelContractError,
    CatalogModelRegistryMutation, CatalogModelUsePolicy, CatalogResolvedEmbeddingModel,
};
use crate::{
    Catalog, CatalogEmbeddingConfig, CatalogManager, CatalogMlopsAssetExt, CatalogTableSchema,
    TableIdentifier,
};

const MLOPS_NAMESPACE: &str = "mlops";

#[derive(Debug, Error)]
pub enum ModelRegistryServiceError {
    #[error("invalid tenant id: {reason}")]
    InvalidTenant { reason: String },
    #[error("invalid model registry name: {reason}")]
    InvalidName { reason: String },
    #[error("embedding model registry '{name}' already exists for tenant '{tenant}'")]
    AlreadyExists { tenant: String, name: String },
    #[error("embedding model registry '{name}' was not found for tenant '{tenant}'")]
    NotFound { tenant: String, name: String },
    #[error("catalog backend did not issue a stable tenant id for '{tenant}'")]
    TenantIdentityUnavailable { tenant: String },
    #[error("catalog object '{name}' for tenant '{tenant}' has no stable asset id")]
    MissingAssetId { tenant: String, name: String },
    #[error("catalog object '{name}' for tenant '{tenant}' is not a model registry")]
    NotModelRegistry { tenant: String, name: String },
    #[error(transparent)]
    Contract(#[from] CatalogModelContractError),
    #[error(transparent)]
    Catalog(#[from] anyhow::Error),
}

/// Stable lifecycle response shared by transport adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CatalogModelRegistryRecord {
    pub tenant_id: String,
    pub asset_id: u64,
    pub registry: CatalogEmbeddingModelRegistry,
}

/// Alias resolution returns both the persistable collection binding and the
/// immutable snapshot a worker may cache by contract digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogResolvedModelBinding {
    pub binding: CatalogEmbeddingConfig,
    pub snapshot: CatalogResolvedEmbeddingModel,
}

/// Catalog-neutral model-registry lifecycle service.
#[derive(Clone)]
pub struct CatalogModelRegistryService {
    manager: Arc<CatalogManager>,
    /// Serializes the check-and-create sequence across every clone shared by
    /// transport adapters. Registry creation is cold control-plane work; a
    /// single lock keeps duplicate create responses deterministic even for
    /// backends whose generic table-create seam predates atomic create-if-absent.
    create_lock: Arc<tokio::sync::Mutex<()>>,
}

impl CatalogModelRegistryService {
    pub fn new(manager: Arc<CatalogManager>) -> Self {
        Self {
            manager,
            create_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn registry_identifier(
        &self,
        tenant: &str,
        name: &str,
    ) -> Result<TableIdentifier, ModelRegistryServiceError> {
        let tenant = tenant.trim();
        proximadb_tenant::validate_request_tenant(tenant).map_err(|error| {
            ModelRegistryServiceError::InvalidTenant {
                reason: error.to_string(),
            }
        })?;
        let name = name.trim();
        if name.is_empty() {
            return Err(ModelRegistryServiceError::InvalidName {
                reason: "name must not be empty".to_string(),
            });
        }
        if name.contains("..")
            || name
                .chars()
                .any(|character| matches!(character, '/' | '\\' | '\0') || character.is_control())
        {
            return Err(ModelRegistryServiceError::InvalidName {
                reason: "name must be one traversal-free catalog path segment".to_string(),
            });
        }
        Ok(TableIdentifier::new(
            vec![tenant.to_string(), MLOPS_NAMESPACE.to_string()],
            name.to_string(),
        ))
    }

    async fn catalog(&self) -> Result<Arc<dyn Catalog>, ModelRegistryServiceError> {
        self.manager
            .default_catalog()
            .await
            .map_err(ModelRegistryServiceError::Catalog)
    }

    /// Preserve model-contract failures across the object-safe catalog seam so
    /// transports can map revision conflicts and validation failures by type,
    /// never by parsing backend error strings.
    fn map_catalog_error(error: anyhow::Error) -> ModelRegistryServiceError {
        match error.downcast::<CatalogModelContractError>() {
            Ok(contract) => ModelRegistryServiceError::Contract(contract),
            Err(error) => ModelRegistryServiceError::Catalog(error),
        }
    }

    async fn ensure_namespace(
        &self,
        catalog: &Arc<dyn Catalog>,
        tenant: &str,
        namespace: &[String],
    ) -> Result<(), ModelRegistryServiceError> {
        if catalog.account_id_u32(tenant).await?.is_none() {
            return Err(ModelRegistryServiceError::TenantIdentityUnavailable {
                tenant: tenant.to_string(),
            });
        }
        if catalog.namespace_exists(namespace).await? {
            return Ok(());
        }
        match catalog
            .create_namespace_for_tenant(namespace, HashMap::new(), Some(tenant))
            .await
        {
            Ok(_) => Ok(()),
            // Concurrent first registration may have created the namespace
            // between the read and create. Re-read authority before deciding.
            Err(_) if catalog.namespace_exists(namespace).await? => Ok(()),
            Err(error) => Err(ModelRegistryServiceError::Catalog(error)),
        }
    }

    fn record_from_schema(
        tenant: &str,
        identifier: &TableIdentifier,
        schema: CatalogTableSchema,
    ) -> Result<CatalogModelRegistryRecord, ModelRegistryServiceError> {
        let asset_id =
            schema
                .object_id
                .ok_or_else(|| ModelRegistryServiceError::MissingAssetId {
                    tenant: tenant.to_string(),
                    name: identifier.name.clone(),
                })?;
        let asset = schema.mlops_asset_as_typed()?.ok_or_else(|| {
            ModelRegistryServiceError::NotModelRegistry {
                tenant: tenant.to_string(),
                name: identifier.name.clone(),
            }
        })?;
        let CatalogMlopsAsset::EmbeddingModel(registry) = asset;
        if registry.name != identifier.name {
            return Err(ModelRegistryServiceError::NotModelRegistry {
                tenant: tenant.to_string(),
                name: identifier.name.clone(),
            });
        }
        Ok(CatalogModelRegistryRecord {
            tenant_id: tenant.to_string(),
            asset_id,
            registry,
        })
    }

    async fn get_from_catalog(
        &self,
        catalog: &Arc<dyn Catalog>,
        tenant: &str,
        name: &str,
    ) -> Result<CatalogModelRegistryRecord, ModelRegistryServiceError> {
        let identifier = self.registry_identifier(tenant, name)?;
        if !catalog.table_exists(&identifier).await? {
            return Err(ModelRegistryServiceError::NotFound {
                tenant: tenant.trim().to_string(),
                name: name.trim().to_string(),
            });
        }
        let schema = catalog.get_table(&identifier).await?;
        Self::record_from_schema(tenant.trim(), &identifier, schema)
    }

    pub async fn create_registry(
        &self,
        tenant: &str,
        name: &str,
    ) -> Result<CatalogModelRegistryRecord, ModelRegistryServiceError> {
        let identifier = self.registry_identifier(tenant, name)?;
        let tenant = tenant.trim();
        let catalog = self.catalog().await?;
        let _guard = self.create_lock.lock().await;
        self.ensure_namespace(&catalog, tenant, &identifier.namespace)
            .await?;
        if catalog.table_exists(&identifier).await? {
            return Err(ModelRegistryServiceError::AlreadyExists {
                tenant: tenant.to_string(),
                name: identifier.name,
            });
        }

        let registry = CatalogEmbeddingModelRegistry::new(identifier.name.clone())?;
        let schema = CatalogTableSchema::new(identifier.name.clone())
            .with_mlops_asset(CatalogMlopsAsset::EmbeddingModel(registry))?;
        match catalog.create_table(&identifier, schema).await {
            Ok(created) => Self::record_from_schema(tenant, &identifier, created),
            Err(_) if catalog.table_exists(&identifier).await? => {
                Err(ModelRegistryServiceError::AlreadyExists {
                    tenant: tenant.to_string(),
                    name: identifier.name,
                })
            }
            Err(error) => Err(ModelRegistryServiceError::Catalog(error)),
        }
    }

    pub async fn get_registry(
        &self,
        tenant: &str,
        name: &str,
    ) -> Result<CatalogModelRegistryRecord, ModelRegistryServiceError> {
        let catalog = self.catalog().await?;
        self.get_from_catalog(&catalog, tenant, name).await
    }

    pub async fn list_registries(
        &self,
        tenant: &str,
    ) -> Result<Vec<CatalogModelRegistryRecord>, ModelRegistryServiceError> {
        let probe = self.registry_identifier(tenant, "_")?;
        let tenant = tenant.trim();
        let catalog = self.catalog().await?;
        if !catalog.namespace_exists(&probe.namespace).await? {
            return Ok(Vec::new());
        }
        let mut identifiers = catalog.list_tables(&probe.namespace).await?;
        identifiers.sort_by(|left, right| left.name.cmp(&right.name));
        let mut records = Vec::with_capacity(identifiers.len());
        for identifier in identifiers {
            let schema = catalog.get_table(&identifier).await?;
            records.push(Self::record_from_schema(tenant, &identifier, schema)?);
        }
        Ok(records)
    }

    pub async fn apply_mutation(
        &self,
        tenant: &str,
        name: &str,
        expected_revision: u64,
        mutation: CatalogModelRegistryMutation,
    ) -> Result<CatalogModelRegistryRecord, ModelRegistryServiceError> {
        let identifier = self.registry_identifier(tenant, name)?;
        let catalog = self.catalog().await?;
        if !catalog.table_exists(&identifier).await? {
            return Err(ModelRegistryServiceError::NotFound {
                tenant: tenant.trim().to_string(),
                name: identifier.name,
            });
        }
        let schema = catalog
            .apply_model_registry_mutation(&identifier, expected_revision, mutation)
            .await
            .map_err(Self::map_catalog_error)?;
        Self::record_from_schema(tenant.trim(), &identifier, schema)
    }

    pub async fn resolve_alias_binding(
        &self,
        tenant: &str,
        name: &str,
        alias: &str,
        dimension: u32,
        policy: &CatalogModelUsePolicy,
    ) -> Result<CatalogResolvedModelBinding, ModelRegistryServiceError> {
        let catalog = self.catalog().await?;
        let record = self.get_from_catalog(&catalog, tenant, name).await?;
        let model = record.registry.resolve_alias(alias)?;
        let binding = CatalogEmbeddingConfig::pinned(
            record.registry.name.clone(),
            dimension,
            record.asset_id,
            model.version,
            model.contract_sha256()?,
        )?;
        let snapshot = catalog
            .resolve_embedding_model_binding(&binding, policy)
            .await
            .map_err(Self::map_catalog_error)?;
        Ok(CatalogResolvedModelBinding { binding, snapshot })
    }
}
