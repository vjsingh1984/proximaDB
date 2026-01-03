//! # Federated Catalog Implementation
//!
//! Provides a unified catalog view across internal and external sources.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use arrow_schema::Schema as ArrowSchema;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::catalog::internal::InternalSchemaRegistry;
use crate::catalog::traits::Catalog;
use crate::catalog::{CatalogManager, TableIdentifier};

use super::external::{ExternalCatalog, ExternalCatalogType};

// ============================================================================
// Constraint Support
// ============================================================================

/// Constraint support levels by catalog/format type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintSupport {
    /// Primary key support
    pub primary_key: ConstraintLevel,
    /// Foreign key support
    pub foreign_key: ConstraintLevel,
    /// Unique constraint support
    pub unique: ConstraintLevel,
    /// Check constraint support
    pub check: ConstraintLevel,
    /// Not null constraint support
    pub not_null: ConstraintLevel,
    /// Default value support
    pub default_value: ConstraintLevel,
}

impl ConstraintSupport {
    /// Full constraint support (internal formats)
    pub fn full() -> Self {
        Self {
            primary_key: ConstraintLevel::Full,
            foreign_key: ConstraintLevel::Full,
            unique: ConstraintLevel::Full,
            check: ConstraintLevel::Full,
            not_null: ConstraintLevel::Full,
            default_value: ConstraintLevel::Full,
        }
    }

    /// Partial constraint support (Delta Lake)
    pub fn delta_lake() -> Self {
        Self {
            primary_key: ConstraintLevel::Partial, // Liquid clustering keys
            foreign_key: ConstraintLevel::None,
            unique: ConstraintLevel::None,
            check: ConstraintLevel::None, // CHECK constraints in Delta 2.0+
            not_null: ConstraintLevel::Full,
            default_value: ConstraintLevel::Partial,
        }
    }

    /// Partial constraint support (Apache Iceberg)
    pub fn iceberg() -> Self {
        Self {
            primary_key: ConstraintLevel::Partial, // Identifier columns
            foreign_key: ConstraintLevel::None,
            unique: ConstraintLevel::None,
            check: ConstraintLevel::None,
            not_null: ConstraintLevel::Full,
            default_value: ConstraintLevel::Partial,
        }
    }

    /// Minimal constraint support (Parquet)
    pub fn parquet() -> Self {
        Self {
            primary_key: ConstraintLevel::None,
            foreign_key: ConstraintLevel::None,
            unique: ConstraintLevel::None,
            check: ConstraintLevel::None,
            not_null: ConstraintLevel::None, // Parquet has optional fields, not NOT NULL
            default_value: ConstraintLevel::None,
        }
    }

    /// LanceDB constraint support
    pub fn lancedb() -> Self {
        Self {
            primary_key: ConstraintLevel::Partial, // Vector ID
            foreign_key: ConstraintLevel::None,
            unique: ConstraintLevel::Partial, // ID uniqueness
            check: ConstraintLevel::None,
            not_null: ConstraintLevel::Full,
            default_value: ConstraintLevel::None,
        }
    }

    /// Get support level for constraint type
    pub fn get_level(&self, constraint_type: &str) -> ConstraintLevel {
        match constraint_type.to_lowercase().as_str() {
            "primary_key" | "pk" => self.primary_key,
            "foreign_key" | "fk" => self.foreign_key,
            "unique" => self.unique,
            "check" => self.check,
            "not_null" | "notnull" => self.not_null,
            "default" | "default_value" => self.default_value,
            _ => ConstraintLevel::None,
        }
    }

    /// Check if all specified constraints are supported
    pub fn supports_all(&self, constraints: &[&str]) -> bool {
        constraints
            .iter()
            .all(|c| self.get_level(c) != ConstraintLevel::None)
    }
}

/// Constraint support level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintLevel {
    /// Full support with enforcement
    Full,
    /// Partial/logical support (metadata only, no enforcement)
    Partial,
    /// No support
    None,
}

// ============================================================================
// Federated Table Info
// ============================================================================

/// Extended table information with federation metadata
#[derive(Debug, Clone)]
pub struct FederatedTableInfo {
    /// Catalog name
    pub catalog: String,
    /// Namespace path
    pub namespace: Vec<String>,
    /// Table name
    pub name: String,
    /// Arrow schema
    pub schema: ArrowSchema,
    /// Whether this is an internal or external table
    pub is_internal: bool,
    /// External catalog type (if external)
    pub external_type: Option<ExternalCatalogType>,
    /// Constraint support for this table's format
    pub constraint_support: ConstraintSupport,
    /// Table properties/metadata
    pub properties: HashMap<String, String>,
    /// Storage location (if external)
    pub location: Option<String>,
}

impl FederatedTableInfo {
    /// Get fully qualified name
    pub fn fqn(&self) -> String {
        if self.namespace.is_empty() {
            format!("{}.{}", self.catalog, self.name)
        } else {
            format!(
                "{}.{}.{}",
                self.catalog,
                self.namespace.join("."),
                self.name
            )
        }
    }
}

// ============================================================================
// Federated Catalog Configuration
// ============================================================================

/// Configuration for federated catalog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedCatalogConfig {
    /// Enable cross-catalog query resolution
    pub enable_cross_catalog: bool,
    /// Default catalog for unqualified names
    pub default_catalog: Option<String>,
    /// Enable caching of external catalog metadata
    pub enable_metadata_cache: bool,
    /// Metadata cache TTL in seconds
    pub cache_ttl_seconds: u64,
    /// Maximum cached entries
    pub max_cache_entries: usize,
}

impl Default for FederatedCatalogConfig {
    fn default() -> Self {
        Self {
            enable_cross_catalog: true,
            default_catalog: Some("internal".to_string()),
            enable_metadata_cache: true,
            cache_ttl_seconds: 300, // 5 minutes
            max_cache_entries: 10000,
        }
    }
}

// ============================================================================
// Federated Catalog
// ============================================================================

/// Federated catalog providing unified view across internal and external sources
pub struct FederatedCatalog {
    /// Internal catalog (full constraint support)
    internal: Arc<InternalSchemaRegistry>,

    /// External catalogs by name
    external: RwLock<HashMap<String, Arc<dyn ExternalCatalog>>>,

    /// Catalog manager for standard catalog operations
    catalog_manager: Arc<CatalogManager>,

    /// Configuration
    config: FederatedCatalogConfig,

    /// Table info cache (fqn -> FederatedTableInfo)
    cache: RwLock<HashMap<String, CachedTableInfo>>,
}

#[derive(Debug, Clone)]
struct CachedTableInfo {
    info: FederatedTableInfo,
    cached_at: std::time::Instant,
}

impl FederatedCatalog {
    /// Create a new federated catalog
    pub fn new(
        internal: Arc<InternalSchemaRegistry>,
        catalog_manager: Arc<CatalogManager>,
        config: FederatedCatalogConfig,
    ) -> Self {
        Self {
            internal,
            external: RwLock::new(HashMap::new()),
            catalog_manager,
            config,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Create with default configuration
    pub fn with_defaults(
        internal: Arc<InternalSchemaRegistry>,
        catalog_manager: Arc<CatalogManager>,
    ) -> Self {
        Self::new(internal, catalog_manager, FederatedCatalogConfig::default())
    }

    // ========================================================================
    // External Catalog Management
    // ========================================================================

    /// Register an external catalog
    pub fn register_external(&self, name: &str, catalog: Arc<dyn ExternalCatalog>) -> Result<()> {
        let mut externals = self.external.write();

        if externals.contains_key(name) {
            warn!("Overwriting existing external catalog: {}", name);
        }

        info!(
            "Registering external catalog: {} (type: {:?})",
            name,
            catalog.catalog_type()
        );
        externals.insert(name.to_string(), catalog);

        Ok(())
    }

    /// Unregister an external catalog
    pub fn unregister_external(&self, name: &str) -> Option<Arc<dyn ExternalCatalog>> {
        let mut externals = self.external.write();
        externals.remove(name)
    }

    /// Get an external catalog by name
    pub fn get_external(&self, name: &str) -> Option<Arc<dyn ExternalCatalog>> {
        self.external.read().get(name).cloned()
    }

    /// List all external catalog names
    pub fn list_external_catalogs(&self) -> Vec<String> {
        self.external.read().keys().cloned().collect()
    }

    // ========================================================================
    // Table Resolution
    // ========================================================================

    /// Resolve a table by fully qualified name
    pub async fn resolve_table(&self, fqn: &str) -> Result<FederatedTableInfo> {
        // Check cache first
        if self.config.enable_metadata_cache {
            if let Some(cached) = self.get_cached(fqn) {
                return Ok(cached);
            }
        }

        let parts: Vec<&str> = fqn.split('.').collect();

        let info = match parts.len() {
            1 => {
                // Just table name - check internal first, then default catalog
                self.resolve_unqualified(parts[0]).await?
            }
            2 => {
                // namespace.table or catalog.table
                self.resolve_two_part(parts[0], parts[1]).await?
            }
            _ => {
                // catalog.namespace.table or catalog.ns1.ns2...nsN.table
                self.resolve_fully_qualified(&parts).await?
            }
        };

        // Cache the result
        if self.config.enable_metadata_cache {
            self.cache_table(&info);
        }

        Ok(info)
    }

    /// Resolve unqualified table name
    async fn resolve_unqualified(&self, table: &str) -> Result<FederatedTableInfo> {
        // Try internal catalog first
        if let Ok(info) = self.resolve_internal("internal", &[], table).await {
            return Ok(info);
        }

        // Try default catalog if configured
        if let Some(ref default) = self.config.default_catalog {
            if default != "internal" {
                if let Ok(info) = self.resolve_in_catalog(default, &[], table).await {
                    return Ok(info);
                }
            }
        }

        Err(anyhow!("Table '{}' not found", table))
    }

    /// Resolve two-part name (could be namespace.table or catalog.table)
    async fn resolve_two_part(&self, first: &str, second: &str) -> Result<FederatedTableInfo> {
        // First try as catalog.table
        if let Ok(info) = self.resolve_in_catalog(first, &[], second).await {
            return Ok(info);
        }

        // Then try as namespace.table in internal catalog
        if let Ok(info) = self.resolve_internal("internal", &[first], second).await {
            return Ok(info);
        }

        // Finally try namespace.table in default catalog
        if let Some(ref default) = self.config.default_catalog {
            if let Ok(info) = self.resolve_in_catalog(default, &[first], second).await {
                return Ok(info);
            }
        }

        Err(anyhow!("Table '{}.{}' not found", first, second))
    }

    /// Resolve fully qualified name
    async fn resolve_fully_qualified(&self, parts: &[&str]) -> Result<FederatedTableInfo> {
        let catalog = parts[0];
        let namespace: Vec<&str> = parts[1..parts.len() - 1].to_vec();
        let table = parts.last().unwrap();

        self.resolve_in_catalog(catalog, &namespace, table).await
    }

    /// Resolve in a specific catalog
    async fn resolve_in_catalog(
        &self,
        catalog: &str,
        namespace: &[&str],
        table: &str,
    ) -> Result<FederatedTableInfo> {
        // Check if it's the internal catalog
        if catalog == "internal" || catalog == "proximadb" {
            return self.resolve_internal(catalog, namespace, table).await;
        }

        // Check external catalogs
        if let Some(external) = self.get_external(catalog) {
            return self
                .resolve_external(&external, catalog, namespace, table)
                .await;
        }

        // Try catalog manager
        if let Ok(std_catalog) = self.catalog_manager.get_catalog(catalog).await {
            return self
                .resolve_standard(&std_catalog, catalog, namespace, table)
                .await;
        }

        Err(anyhow!("Catalog '{}' not found", catalog))
    }

    /// Resolve in internal catalog
    async fn resolve_internal(
        &self,
        catalog: &str,
        namespace: &[&str],
        table: &str,
    ) -> Result<FederatedTableInfo> {
        // Build internal lookup path
        let lookup_path = if namespace.is_empty() {
            table.to_string()
        } else {
            format!("{}.{}", namespace.join("."), table)
        };

        // Try to get from internal registry
        if let Ok(obj) = self.internal.get(&lookup_path).await {
            let schema = obj.schema.to_arrow_schema()?;

            return Ok(FederatedTableInfo {
                catalog: catalog.to_string(),
                namespace: namespace.iter().map(|s| s.to_string()).collect(),
                name: table.to_string(),
                schema,
                is_internal: true,
                external_type: None,
                constraint_support: ConstraintSupport::full(),
                properties: obj.properties.clone(),
                location: None,
            });
        }

        Err(anyhow!(
            "Table '{}' not found in internal catalog",
            lookup_path
        ))
    }

    /// Resolve in external catalog
    async fn resolve_external(
        &self,
        catalog: &Arc<dyn ExternalCatalog>,
        catalog_name: &str,
        namespace: &[&str],
        table: &str,
    ) -> Result<FederatedTableInfo> {
        let namespace_str = namespace.join(".");
        let schema = catalog.get_table_schema(&namespace_str, table).await?;

        Ok(FederatedTableInfo {
            catalog: catalog_name.to_string(),
            namespace: namespace.iter().map(|s| s.to_string()).collect(),
            name: table.to_string(),
            schema,
            is_internal: false,
            external_type: Some(catalog.catalog_type()),
            constraint_support: catalog.constraint_support(),
            properties: catalog
                .get_table_properties(&namespace_str, table)
                .await
                .unwrap_or_default(),
            location: catalog.get_table_location(&namespace_str, table).await.ok(),
        })
    }

    /// Resolve in standard catalog
    async fn resolve_standard(
        &self,
        _catalog: &Arc<dyn Catalog>,
        catalog_name: &str,
        namespace: &[&str],
        table: &str,
    ) -> Result<FederatedTableInfo> {
        // Standard catalogs use table identifier
        let id = TableIdentifier::new(
            namespace.iter().map(|s| s.to_string()).collect(),
            table.to_string(),
        );

        // For now, return basic info - full implementation would query the catalog
        Ok(FederatedTableInfo {
            catalog: catalog_name.to_string(),
            namespace: id.namespace,
            name: id.name,
            schema: ArrowSchema::empty(),
            is_internal: false,
            external_type: None,
            constraint_support: ConstraintSupport::parquet(), // Default to minimal
            properties: HashMap::new(),
            location: None,
        })
    }

    // ========================================================================
    // Table Listing
    // ========================================================================

    /// List all tables across all catalogs
    pub async fn list_all_tables(&self) -> Result<Vec<FederatedTableInfo>> {
        let mut tables = Vec::new();

        // List internal tables
        for obj in self.internal.list_all().await {
            if let Ok(schema) = obj.schema.to_arrow_schema() {
                tables.push(FederatedTableInfo {
                    catalog: "internal".to_string(),
                    namespace: obj.namespace.clone(),
                    name: obj.name.clone(),
                    schema,
                    is_internal: true,
                    external_type: None,
                    constraint_support: ConstraintSupport::full(),
                    properties: obj.properties.clone(),
                    location: None,
                });
            }
        }

        // List external catalog tables
        let externals = self.external.read();
        for (name, catalog) in externals.iter() {
            if let Ok(namespaces) = catalog.list_namespaces().await {
                for ns in namespaces {
                    if let Ok(table_names) = catalog.list_tables(&ns).await {
                        for table in table_names {
                            if let Ok(info) =
                                self.resolve_external(catalog, name, &[&ns], &table).await
                            {
                                tables.push(info);
                            }
                        }
                    }
                }
            }
        }

        Ok(tables)
    }

    /// List tables in a specific catalog
    pub async fn list_catalog_tables(&self, catalog: &str) -> Result<Vec<FederatedTableInfo>> {
        if catalog == "internal" || catalog == "proximadb" {
            let mut tables = Vec::new();
            for obj in self.internal.list_all().await {
                if let Ok(schema) = obj.schema.to_arrow_schema() {
                    tables.push(FederatedTableInfo {
                        catalog: catalog.to_string(),
                        namespace: obj.namespace.clone(),
                        name: obj.name.clone(),
                        schema,
                        is_internal: true,
                        external_type: None,
                        constraint_support: ConstraintSupport::full(),
                        properties: obj.properties.clone(),
                        location: None,
                    });
                }
            }
            return Ok(tables);
        }

        if let Some(external) = self.get_external(catalog) {
            let mut tables = Vec::new();
            if let Ok(namespaces) = external.list_namespaces().await {
                for ns in namespaces {
                    if let Ok(table_names) = external.list_tables(&ns).await {
                        for table in table_names {
                            if let Ok(info) = self
                                .resolve_external(&external, catalog, &[&ns], &table)
                                .await
                            {
                                tables.push(info);
                            }
                        }
                    }
                }
            }
            return Ok(tables);
        }

        Err(anyhow!("Catalog '{}' not found", catalog))
    }

    // ========================================================================
    // Constraint Support Queries
    // ========================================================================

    /// Get constraint support for a table
    pub async fn get_constraint_support(&self, fqn: &str) -> Result<ConstraintSupport> {
        let info = self.resolve_table(fqn).await?;
        Ok(info.constraint_support)
    }

    /// Check if a constraint is supported for a table
    pub async fn supports_constraint(&self, fqn: &str, constraint: &str) -> Result<bool> {
        let support = self.get_constraint_support(fqn).await?;
        Ok(support.get_level(constraint) != ConstraintLevel::None)
    }

    // ========================================================================
    // Caching
    // ========================================================================

    fn get_cached(&self, fqn: &str) -> Option<FederatedTableInfo> {
        let cache = self.cache.read();
        if let Some(cached) = cache.get(fqn) {
            let ttl = std::time::Duration::from_secs(self.config.cache_ttl_seconds);
            if cached.cached_at.elapsed() < ttl {
                debug!("Cache hit for table: {}", fqn);
                return Some(cached.info.clone());
            }
        }
        None
    }

    fn cache_table(&self, info: &FederatedTableInfo) {
        let mut cache = self.cache.write();

        // Evict old entries if needed
        if cache.len() >= self.config.max_cache_entries {
            let ttl = std::time::Duration::from_secs(self.config.cache_ttl_seconds);
            cache.retain(|_, v| v.cached_at.elapsed() < ttl);
        }

        cache.insert(
            info.fqn(),
            CachedTableInfo {
                info: info.clone(),
                cached_at: std::time::Instant::now(),
            },
        );
    }

    /// Clear the metadata cache
    pub fn clear_cache(&self) {
        self.cache.write().clear();
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constraint_support_full() {
        let support = ConstraintSupport::full();
        assert_eq!(support.primary_key, ConstraintLevel::Full);
        assert_eq!(support.foreign_key, ConstraintLevel::Full);
        assert!(support.supports_all(&["primary_key", "foreign_key", "unique"]));
    }

    #[test]
    fn test_constraint_support_delta() {
        let support = ConstraintSupport::delta_lake();
        assert_eq!(support.primary_key, ConstraintLevel::Partial);
        assert_eq!(support.foreign_key, ConstraintLevel::None);
        assert!(!support.supports_all(&["foreign_key"]));
    }

    #[test]
    fn test_constraint_support_parquet() {
        let support = ConstraintSupport::parquet();
        assert_eq!(support.primary_key, ConstraintLevel::None);
        assert!(!support.supports_all(&["primary_key"]));
    }

    #[test]
    fn test_federated_table_info_fqn() {
        let info = FederatedTableInfo {
            catalog: "mycat".to_string(),
            namespace: vec!["db".to_string(), "schema".to_string()],
            name: "users".to_string(),
            schema: ArrowSchema::empty(),
            is_internal: false,
            external_type: Some(ExternalCatalogType::Iceberg),
            constraint_support: ConstraintSupport::iceberg(),
            properties: HashMap::new(),
            location: None,
        };

        assert_eq!(info.fqn(), "mycat.db.schema.users");
    }
}
