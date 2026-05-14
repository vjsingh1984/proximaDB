//! Internal Schema Registry
//!
//! Provides unified schema management for all data models.
//! This is the core registry that stores and manages CatalogObject instances.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use proximadb_catalog::CatalogTableSchema;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::{
    CatalogObject, ConstraintEnforcer, ObjectSchema, ObjectType, SchemaEnforcementMode,
    TableConstraint,
};
use crate::catalog::TableIdentifier;

/// Internal schema registry for multi-model objects
pub struct InternalSchemaRegistry {
    /// Objects indexed by FQN (catalog.namespace.name)
    objects: RwLock<HashMap<String, Arc<CatalogObject>>>,

    /// Objects indexed by ID
    objects_by_id: RwLock<HashMap<String, Arc<CatalogObject>>>,

    /// Constraint enforcer
    constraint_enforcer: Arc<ConstraintEnforcer>,

    /// Default catalog name
    default_catalog: String,

    /// Default namespace
    default_namespace: Vec<String>,
}

impl InternalSchemaRegistry {
    /// Create a new schema registry
    pub fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            objects_by_id: RwLock::new(HashMap::new()),
            constraint_enforcer: Arc::new(ConstraintEnforcer::new()),
            default_catalog: "default".to_string(),
            default_namespace: vec!["public".to_string()],
        }
    }

    /// Create with custom defaults
    pub fn with_defaults(catalog: impl Into<String>, namespace: Vec<String>) -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            objects_by_id: RwLock::new(HashMap::new()),
            constraint_enforcer: Arc::new(ConstraintEnforcer::new()),
            default_catalog: catalog.into(),
            default_namespace: namespace,
        }
    }

    /// Register a new object
    pub async fn register(&self, object: CatalogObject) -> Result<Arc<CatalogObject>> {
        let fqn = object.fqn();
        let id = object.object_id.clone();

        // Check for duplicates
        {
            let objects = self.objects.read().await;
            if objects.contains_key(&fqn) {
                return Err(anyhow!("Object '{}' already exists", fqn));
            }
        }

        let arc_object = Arc::new(object);

        // Insert into both indexes
        {
            let mut objects = self.objects.write().await;
            let mut objects_by_id = self.objects_by_id.write().await;

            objects.insert(fqn.clone(), arc_object.clone());
            objects_by_id.insert(id, arc_object.clone());
        }

        info!(
            "Registered object: {} (type: {})",
            fqn, arc_object.object_type
        );
        Ok(arc_object)
    }

    /// Create and register an RDBMS table
    pub async fn create_table(
        &self,
        identifier: &TableIdentifier,
        schema: CatalogTableSchema,
    ) -> Result<Arc<CatalogObject>> {
        let catalog = self.default_catalog.clone();
        let namespace = if identifier.namespace.is_empty() {
            self.default_namespace.clone()
        } else {
            identifier.namespace.clone()
        };

        let object_schema = ObjectSchema::from_table_schema(&schema);

        let object =
            CatalogObject::new(catalog, namespace, &identifier.name, ObjectType::RdbmsTable)
                .with_schema(object_schema, SchemaEnforcementMode::Strict);

        self.register(object).await
    }

    /// Create and register a vector collection
    pub async fn create_vector_collection(
        &self,
        name: &str,
        dimension: u32,
        distance_metric: &str,
    ) -> Result<Arc<CatalogObject>> {
        use super::{ModelProperties, VectorProperties};
        use proximadb_catalog::{CatalogColumn, CatalogDataType};

        let object_schema = ObjectSchema {
            columns: vec![
                CatalogColumn::new(1, "id", CatalogDataType::String).nullable(false),
                CatalogColumn::new(2, "vector", CatalogDataType::Vector).nullable(false),
                CatalogColumn::new(3, "metadata", CatalogDataType::Json),
            ],
            primary_key: vec!["id".to_string()],
            constraints: vec![],
            indexes: vec![],
            model_properties: ModelProperties::Vector(VectorProperties {
                dimension,
                distance_metric: distance_metric.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let object = CatalogObject::new(
            self.default_catalog.clone(),
            self.default_namespace.clone(),
            name,
            ObjectType::VectorCollection,
        )
        .with_schema(object_schema, SchemaEnforcementMode::Hybrid);

        self.register(object).await
    }

    /// Create and register a document collection
    pub async fn create_document_collection(
        &self,
        name: &str,
        json_schema: Option<&str>,
    ) -> Result<Arc<CatalogObject>> {
        use super::{DocumentProperties, ModelProperties};
        use proximadb_catalog::{CatalogColumn, CatalogDataType};

        let object_schema = ObjectSchema {
            columns: vec![
                CatalogColumn::new(1, "_id", CatalogDataType::String).nullable(false),
                CatalogColumn::new(2, "document", CatalogDataType::Json).nullable(false),
                CatalogColumn::new(3, "_created_at", CatalogDataType::TimestampTz),
                CatalogColumn::new(4, "_updated_at", CatalogDataType::TimestampTz),
            ],
            primary_key: vec!["_id".to_string()],
            constraints: vec![],
            indexes: vec![],
            model_properties: ModelProperties::Document(DocumentProperties {
                json_schema: json_schema.map(|s| s.to_string()),
                id_generation: "uuid".to_string(),
                enable_full_text: false,
                indexed_paths: vec![],
            }),
            ..Default::default()
        };

        let object = CatalogObject::new(
            self.default_catalog.clone(),
            self.default_namespace.clone(),
            name,
            ObjectType::DocumentCollection,
        )
        .with_schema(object_schema, SchemaEnforcementMode::Flexible);

        self.register(object).await
    }

    /// Create and register a graph
    pub async fn create_graph(&self, name: &str, directed: bool) -> Result<Arc<CatalogObject>> {
        use super::{GraphProperties, ModelProperties};

        let object_schema = ObjectSchema {
            columns: vec![],
            primary_key: vec![],
            constraints: vec![],
            indexes: vec![],
            model_properties: ModelProperties::Graph(GraphProperties {
                graph_type: if directed {
                    "directed".to_string()
                } else {
                    "undirected".to_string()
                },
                allow_self_loops: true,
                allow_multi_edges: false,
                node_labels: vec![],
                edge_types: vec![],
            }),
            ..Default::default()
        };

        let object = CatalogObject::new(
            self.default_catalog.clone(),
            self.default_namespace.clone(),
            name,
            ObjectType::Graph,
        )
        .with_schema(object_schema, SchemaEnforcementMode::Flexible);

        self.register(object).await
    }

    /// Create and register a log stream
    pub async fn create_log_stream(
        &self,
        name: &str,
        retention_seconds: u64,
    ) -> Result<Arc<CatalogObject>> {
        use super::{ModelProperties, ObservabilityProperties};
        use proximadb_catalog::{CatalogColumn, CatalogDataType};

        let object_schema = ObjectSchema {
            columns: vec![
                CatalogColumn::new(1, "timestamp", CatalogDataType::TimestampTz).nullable(false),
                CatalogColumn::new(2, "level", CatalogDataType::String),
                CatalogColumn::new(3, "message", CatalogDataType::String),
                CatalogColumn::new(4, "labels", CatalogDataType::Json),
                CatalogColumn::new(5, "trace_id", CatalogDataType::String),
                CatalogColumn::new(6, "span_id", CatalogDataType::String),
            ],
            primary_key: vec!["timestamp".to_string()],
            constraints: vec![],
            indexes: vec![],
            model_properties: ModelProperties::Observability(ObservabilityProperties {
                stream_type: "logs".to_string(),
                retention_seconds,
                rollup_intervals: vec![],
                cardinality_limits: HashMap::new(),
            }),
            ..Default::default()
        };

        let object = CatalogObject::new(
            self.default_catalog.clone(),
            self.default_namespace.clone(),
            name,
            ObjectType::LogStream,
        )
        .with_schema(object_schema, SchemaEnforcementMode::Flexible);

        self.register(object).await
    }

    /// Get object by FQN
    pub async fn get(&self, fqn: &str) -> Result<Arc<CatalogObject>> {
        let objects = self.objects.read().await;
        objects
            .get(fqn)
            .cloned()
            .ok_or_else(|| anyhow!("Object '{}' not found", fqn))
    }

    /// Get object by ID
    pub async fn get_by_id(&self, id: &str) -> Result<Arc<CatalogObject>> {
        let objects = self.objects_by_id.read().await;
        objects
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Object with ID '{}' not found", id))
    }

    /// Get object by identifier (resolves namespace)
    pub async fn get_by_identifier(
        &self,
        identifier: &TableIdentifier,
    ) -> Result<Arc<CatalogObject>> {
        let fqn = self.resolve_fqn(identifier);
        self.get(&fqn).await
    }

    /// Check if object exists
    pub async fn exists(&self, fqn: &str) -> bool {
        let objects = self.objects.read().await;
        objects.contains_key(fqn)
    }

    /// List all objects
    pub async fn list_all(&self) -> Vec<Arc<CatalogObject>> {
        let objects = self.objects.read().await;
        objects.values().cloned().collect()
    }

    /// List objects by type
    pub async fn list_by_type(&self, object_type: ObjectType) -> Vec<Arc<CatalogObject>> {
        let objects = self.objects.read().await;
        objects
            .values()
            .filter(|o| o.object_type == object_type)
            .cloned()
            .collect()
    }

    /// List objects in namespace
    pub async fn list_in_namespace(
        &self,
        catalog: &str,
        namespace: &[String],
    ) -> Vec<Arc<CatalogObject>> {
        let objects = self.objects.read().await;
        objects
            .values()
            .filter(|o| o.catalog == catalog && o.namespace == namespace)
            .cloned()
            .collect()
    }

    /// Drop an object
    pub async fn drop(&self, fqn: &str) -> Result<bool> {
        let mut objects = self.objects.write().await;
        let mut objects_by_id = self.objects_by_id.write().await;

        if let Some(obj) = objects.remove(fqn) {
            objects_by_id.remove(&obj.object_id);
            info!("Dropped object: {}", fqn);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Update object schema
    pub async fn update_schema(
        &self,
        fqn: &str,
        new_schema: ObjectSchema,
        change_description: Option<String>,
        changed_by: Option<String>,
    ) -> Result<Arc<CatalogObject>> {
        let mut objects = self.objects.write().await;
        let mut objects_by_id = self.objects_by_id.write().await;

        let obj = objects
            .get_mut(fqn)
            .ok_or_else(|| anyhow!("Object '{}' not found", fqn))?;

        // Clone and update
        let mut updated = (**obj).clone();
        updated.update_schema(new_schema, change_description, changed_by);

        let arc_updated = Arc::new(updated);

        // Update both indexes
        objects.insert(fqn.to_string(), arc_updated.clone());
        objects_by_id.insert(arc_updated.object_id.clone(), arc_updated.clone());

        debug!(
            "Updated schema for object: {} (version: {})",
            fqn, arc_updated.schema_version
        );
        Ok(arc_updated)
    }

    /// Add constraint to object
    pub async fn add_constraint(
        &self,
        fqn: &str,
        constraint: TableConstraint,
    ) -> Result<Arc<CatalogObject>> {
        let obj = self.get(fqn).await?;

        let mut new_schema = obj.schema.clone();
        new_schema.constraints.push(constraint.clone());

        self.update_schema(
            fqn,
            new_schema,
            Some(format!("Added constraint: {}", constraint.name)),
            None,
        )
        .await
    }

    /// Drop constraint from object
    pub async fn drop_constraint(
        &self,
        fqn: &str,
        constraint_name: &str,
    ) -> Result<Arc<CatalogObject>> {
        let obj = self.get(fqn).await?;

        let mut new_schema = obj.schema.clone();
        let initial_len = new_schema.constraints.len();
        new_schema.constraints.retain(|c| c.name != constraint_name);

        if new_schema.constraints.len() == initial_len {
            return Err(anyhow!(
                "Constraint '{}' not found on object '{}'",
                constraint_name,
                fqn
            ));
        }

        self.update_schema(
            fqn,
            new_schema,
            Some(format!("Dropped constraint: {}", constraint_name)),
            None,
        )
        .await
    }

    /// Get constraint enforcer
    pub fn constraint_enforcer(&self) -> Arc<ConstraintEnforcer> {
        self.constraint_enforcer.clone()
    }

    /// Resolve identifier to FQN
    fn resolve_fqn(&self, identifier: &TableIdentifier) -> String {
        let namespace = if identifier.namespace.is_empty() {
            self.default_namespace.clone()
        } else {
            identifier.namespace.clone()
        };

        let mut parts = vec![self.default_catalog.clone()];
        parts.extend(namespace);
        parts.push(identifier.name.clone());
        parts.join(".")
    }

    /// Get all tables (RDBMS)
    pub async fn list_tables(&self) -> Vec<Arc<CatalogObject>> {
        self.list_by_type(ObjectType::RdbmsTable).await
    }

    /// Get all vector collections
    pub async fn list_vector_collections(&self) -> Vec<Arc<CatalogObject>> {
        self.list_by_type(ObjectType::VectorCollection).await
    }

    /// Get all graphs
    pub async fn list_graphs(&self) -> Vec<Arc<CatalogObject>> {
        self.list_by_type(ObjectType::Graph).await
    }

    /// Get all document collections
    pub async fn list_document_collections(&self) -> Vec<Arc<CatalogObject>> {
        self.list_by_type(ObjectType::DocumentCollection).await
    }

    /// Get schema version for an object
    pub async fn get_schema_version(&self, fqn: &str) -> Result<i32> {
        let obj = self.get(fqn).await?;
        Ok(obj.schema_version)
    }

    /// Get schema at specific version
    pub async fn get_schema_at_version(&self, fqn: &str, version: i32) -> Result<ObjectSchema> {
        let obj = self.get(fqn).await?;
        obj.get_schema_at_version(version)
            .cloned()
            .ok_or_else(|| anyhow!("Schema version {} not found for object '{}'", version, fqn))
    }

    /// Get schema history for an object
    pub async fn get_schema_history(&self, fqn: &str) -> Result<Vec<i32>> {
        let obj = self.get(fqn).await?;
        let mut versions: Vec<i32> = obj.schema_history.iter().map(|s| s.version).collect();
        versions.push(obj.schema_version);
        Ok(versions)
    }

    /// Get object count
    pub async fn count(&self) -> usize {
        let objects = self.objects.read().await;
        objects.len()
    }

    /// Clear all objects (for testing)
    #[cfg(test)]
    pub async fn clear(&self) {
        let mut objects = self.objects.write().await;
        let mut objects_by_id = self.objects_by_id.write().await;
        objects.clear();
        objects_by_id.clear();
    }
}

impl Default for InternalSchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_catalog::{CatalogColumn, CatalogDataType};

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = InternalSchemaRegistry::new();
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_create_table() {
        let registry = InternalSchemaRegistry::new();

        let schema = CatalogTableSchema::new("users")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String))
            .with_primary_key(vec!["id".to_string()]);

        let identifier = TableIdentifier::new(vec![], "users".to_string());
        let obj = registry
            .create_table(&identifier, schema)
            .await
            .expect("table creation should succeed");

        assert_eq!(obj.object_type, ObjectType::RdbmsTable);
        assert_eq!(obj.enforcement_mode, SchemaEnforcementMode::Strict);
        assert_eq!(obj.schema.columns.len(), 2);
    }

    #[tokio::test]
    async fn test_create_vector_collection() {
        let registry = InternalSchemaRegistry::new();

        let obj = registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .expect("vector collection creation should succeed");

        assert_eq!(obj.object_type, ObjectType::VectorCollection);
        assert_eq!(obj.enforcement_mode, SchemaEnforcementMode::Hybrid);
    }

    #[tokio::test]
    async fn test_create_document_collection() {
        let registry = InternalSchemaRegistry::new();

        let obj = registry
            .create_document_collection("products", None)
            .await
            .expect("document collection creation should succeed");

        assert_eq!(obj.object_type, ObjectType::DocumentCollection);
        assert_eq!(obj.enforcement_mode, SchemaEnforcementMode::Flexible);
    }

    #[tokio::test]
    async fn test_create_graph() {
        let registry = InternalSchemaRegistry::new();

        let obj = registry
            .create_graph("social", true)
            .await
            .expect("graph creation should succeed");

        assert_eq!(obj.object_type, ObjectType::Graph);
    }

    #[tokio::test]
    async fn test_list_by_type() {
        let registry = InternalSchemaRegistry::new();

        registry
            .create_vector_collection("vec1", 128, "l2")
            .await
            .expect("vec1 creation should succeed");
        registry
            .create_vector_collection("vec2", 256, "cosine")
            .await
            .expect("vec2 creation should succeed");
        registry
            .create_graph("graph1", true)
            .await
            .expect("graph1 creation should succeed");

        let vectors = registry.list_vector_collections().await;
        assert_eq!(vectors.len(), 2);

        let graphs = registry.list_graphs().await;
        assert_eq!(graphs.len(), 1);
    }

    #[tokio::test]
    async fn test_get_by_fqn() {
        let registry = InternalSchemaRegistry::new();

        registry
            .create_vector_collection("test_vec", 128, "l2")
            .await
            .expect("test_vec creation should succeed");

        let obj = registry
            .get("default.public.test_vec")
            .await
            .expect("test_vec should exist");
        assert_eq!(obj.name, "test_vec");
    }

    #[tokio::test]
    async fn test_drop_object() {
        let registry = InternalSchemaRegistry::new();

        registry
            .create_vector_collection("to_drop", 128, "l2")
            .await
            .expect("to_drop creation should succeed");

        assert!(registry.exists("default.public.to_drop").await);

        let dropped = registry
            .drop("default.public.to_drop")
            .await
            .expect("drop should succeed");
        assert!(dropped);

        assert!(!registry.exists("default.public.to_drop").await);
    }

    #[tokio::test]
    async fn test_schema_update_with_history() {
        let registry = InternalSchemaRegistry::new();

        let schema = CatalogTableSchema::new("evolving")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64))
            .with_primary_key(vec!["id".to_string()]);

        let identifier = TableIdentifier::new(vec![], "evolving".to_string());
        registry
            .create_table(&identifier, schema)
            .await
            .expect("evolving table creation should succeed");

        // Update schema
        let new_schema = ObjectSchema {
            columns: vec![
                CatalogColumn::new(1, "id", CatalogDataType::Int64),
                CatalogColumn::new(2, "name", CatalogDataType::String),
            ],
            primary_key: vec!["id".to_string()],
            ..Default::default()
        };

        registry
            .update_schema(
                "default.public.evolving",
                new_schema,
                Some("Added name".to_string()),
                Some("admin".to_string()),
            )
            .await
            .expect("schema update should succeed");

        let obj = registry
            .get("default.public.evolving")
            .await
            .expect("evolving table should exist");
        assert_eq!(obj.schema_version, 2);
        assert_eq!(obj.schema_history.len(), 1);
    }

    #[tokio::test]
    async fn test_add_constraint() {
        let registry = InternalSchemaRegistry::new();

        let schema = CatalogTableSchema::new("constrained")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64))
            .with_column(CatalogColumn::new(2, "email", CatalogDataType::String))
            .with_primary_key(vec!["id".to_string()]);

        let identifier = TableIdentifier::new(vec![], "constrained".to_string());
        registry
            .create_table(&identifier, schema)
            .await
            .expect("constrained table creation should succeed");

        // Add unique constraint
        let constraint = TableConstraint::unique("uq_email", vec!["email".to_string()]);
        registry
            .add_constraint("default.public.constrained", constraint)
            .await
            .expect("constraint addition should succeed");

        let obj = registry
            .get("default.public.constrained")
            .await
            .expect("constrained table should exist");
        assert_eq!(obj.schema.constraints.len(), 1);
    }

    #[tokio::test]
    async fn test_duplicate_registration_fails() {
        let registry = InternalSchemaRegistry::new();

        registry
            .create_graph("unique_graph", true)
            .await
            .expect("unique_graph creation should succeed");

        let result = registry.create_graph("unique_graph", true).await;
        assert!(result.is_err());
    }
}
