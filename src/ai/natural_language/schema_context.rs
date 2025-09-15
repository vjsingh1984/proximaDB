//! Schema Context Builder
//!
//! Builds database schema context for natural language query translation
//! with tenant-aware table access and security validation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::Result;
use tracing::{debug, warn};

/// Schema context for natural language translation
#[derive(Debug, Clone)]
pub struct SchemaContext {
    table_schemas: HashMap<String, TableSchema>,
    relationships: Vec<TableRelationship>,
    config: SchemaContextConfig,
}

/// Configuration for schema context building
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaContextConfig {
    pub include_column_descriptions: bool,
    pub include_sample_values: bool,
    pub include_relationships: bool,
    pub max_sample_values_per_column: usize,
    pub enable_schema_caching: bool,
}

/// Schema information for a single table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub table_name: String,
    pub description: Option<String>,
    pub columns: Vec<ColumnSchema>,
    pub primary_key: Option<String>,
    pub indexes: Vec<String>,
    pub row_count_estimate: Option<u64>,
}

/// Schema information for a single column
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSchema {
    pub column_name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub description: Option<String>,
    pub sample_values: Vec<String>,
    pub is_foreign_key: bool,
    pub references_table: Option<String>,
    pub references_column: Option<String>,
}

/// Relationship between tables
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRelationship {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub relationship_type: RelationshipType,
}

/// Type of relationship between tables
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RelationshipType {
    OneToOne,
    OneToMany,
    ManyToOne,
    ManyToMany,
}

/// Builder for schema context
pub struct SchemaContextBuilder {
    config: SchemaContextConfig,
}

impl SchemaContext {
    pub async fn new() -> Result<Self> {
        let config = SchemaContextConfig::default();
        let builder = SchemaContextBuilder::new(config);
        builder.build().await
    }

    /// Build schema context description for specific tables
    pub async fn build_context(&self, accessible_tables: &[String]) -> Result<String> {
        let mut context = String::new();

        context.push_str("DATABASE SCHEMA:\n");
        context.push_str("===============\n\n");

        for table_name in accessible_tables {
            if let Some(table_schema) = self.table_schemas.get(table_name) {
                context.push_str(&self.build_table_description(table_schema));
                context.push('\n');
            }
        }

        // Add relationships if enabled
        if self.config.include_relationships {
            context.push_str("TABLE RELATIONSHIPS:\n");
            context.push_str("==================\n");

            let relevant_relationships: Vec<&TableRelationship> = self.relationships.iter()
                .filter(|rel| accessible_tables.contains(&rel.from_table) && accessible_tables.contains(&rel.to_table))
                .collect();

            for relationship in relevant_relationships {
                context.push_str(&format!(
                    "- {}.{} {} {}.{}\n",
                    relationship.from_table,
                    relationship.from_column,
                    self.relationship_to_string(&relationship.relationship_type),
                    relationship.to_table,
                    relationship.to_column
                ));
            }
            context.push('\n');
        }

        // Add usage guidelines
        context.push_str("QUERY GUIDELINES:\n");
        context.push_str("================\n");
        context.push_str("- Use only the tables and columns listed above\n");
        context.push_str("- Always include tenant_id filters where applicable\n");
        context.push_str("- Use proper JOINs for related tables\n");
        context.push_str("- Limit results to prevent resource exhaustion\n");
        context.push_str("- Generate only SELECT statements\n\n");

        Ok(context)
    }

    fn build_table_description(&self, table: &TableSchema) -> String {
        let mut description = format!("TABLE: {}\n", table.table_name);

        if let Some(ref desc) = table.description {
            description.push_str(&format!("Description: {}\n", desc));
        }

        if let Some(row_count) = table.row_count_estimate {
            description.push_str(&format!("Estimated rows: {}\n", row_count));
        }

        description.push_str("Columns:\n");

        for column in &table.columns {
            description.push_str(&format!(
                "  - {} ({}) {}",
                column.column_name,
                column.data_type,
                if column.is_nullable { "NULL" } else { "NOT NULL" }
            ));

            if column.is_foreign_key {
                if let (Some(ref_table), Some(ref_col)) = (&column.references_table, &column.references_column) {
                    description.push_str(&format!(" -> {}.{}", ref_table, ref_col));
                }
            }

            if let Some(ref desc) = column.description {
                description.push_str(&format!(" - {}", desc));
            }

            // Add sample values if configured and available
            if self.config.include_sample_values && !column.sample_values.is_empty() {
                let samples: Vec<&str> = column.sample_values.iter()
                    .map(|s| s.as_str())
                    .take(self.config.max_sample_values_per_column)
                    .collect();
                description.push_str(&format!(" [Examples: {}]", samples.join(", ")));
            }

            description.push('\n');
        }

        description.push('\n');
        description
    }

    fn relationship_to_string(&self, rel_type: &RelationshipType) -> &'static str {
        match rel_type {
            RelationshipType::OneToOne => "→",
            RelationshipType::OneToMany => "→*",
            RelationshipType::ManyToOne => "*→",
            RelationshipType::ManyToMany => "*→*",
        }
    }

    /// Get available tables
    pub fn get_available_tables(&self) -> Vec<String> {
        self.table_schemas.keys().cloned().collect()
    }

    /// Get schema for specific table
    pub fn get_table_schema(&self, table_name: &str) -> Option<&TableSchema> {
        self.table_schemas.get(table_name)
    }

    /// Validate table access for user
    pub fn validate_table_access(&self, table_name: &str, accessible_tables: &[String]) -> bool {
        accessible_tables.contains(&table_name.to_string())
    }
}

impl SchemaContextBuilder {
    pub fn new(config: SchemaContextConfig) -> Self {
        Self { config }
    }

    /// Build complete schema context
    pub async fn build(&self) -> Result<SchemaContext> {
        debug!("Building database schema context");

        // In a real implementation, this would query the database for schema information
        // For now, create a representative schema for ProximaDB
        let table_schemas = self.build_proximadb_schema().await?;
        let relationships = self.build_table_relationships().await?;

        Ok(SchemaContext {
            table_schemas,
            relationships,
            config: self.config.clone(),
        })
    }

    /// Build ProximaDB schema (placeholder for real schema discovery)
    async fn build_proximadb_schema(&self) -> Result<HashMap<String, TableSchema>> {
        let mut schemas = HashMap::new();

        // Collections table
        schemas.insert("collections".to_string(), TableSchema {
            table_name: "collections".to_string(),
            description: Some("Vector collections in the database".to_string()),
            columns: vec![
                ColumnSchema {
                    column_name: "id".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Unique collection identifier".to_string()),
                    sample_values: vec!["coll_1".to_string(), "user_vectors".to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
                ColumnSchema {
                    column_name: "name".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Human-readable collection name".to_string()),
                    sample_values: vec!["Product Embeddings".to_string(), "Customer Vectors".to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
                ColumnSchema {
                    column_name: "tenant_id".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Tenant identifier for multi-tenant isolation".to_string()),
                    sample_values: vec!["tenant_1".to_string(), "company_abc".to_string()],
                    is_foreign_key: true,
                    references_table: Some("tenants".to_string()),
                    references_column: Some("id".to_string()),
                },
                ColumnSchema {
                    column_name: "created_at".to_string(),
                    data_type: "TIMESTAMP".to_string(),
                    is_nullable: false,
                    description: Some("Collection creation timestamp".to_string()),
                    sample_values: vec!["2024-01-15 10:30:00".to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
            ],
            primary_key: Some("id".to_string()),
            indexes: vec!["idx_tenant_id".to_string(), "idx_created_at".to_string()],
            row_count_estimate: Some(1000),
        });

        // Vectors table
        schemas.insert("vectors".to_string(), TableSchema {
            table_name: "vectors".to_string(),
            description: Some("Individual vector records with embeddings and metadata".to_string()),
            columns: vec![
                ColumnSchema {
                    column_name: "id".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Unique vector identifier".to_string()),
                    sample_values: vec!["vec_1".to_string(), "product_123".to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
                ColumnSchema {
                    column_name: "collection_id".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Collection this vector belongs to".to_string()),
                    sample_values: vec!["coll_1".to_string()],
                    is_foreign_key: true,
                    references_table: Some("collections".to_string()),
                    references_column: Some("id".to_string()),
                },
                ColumnSchema {
                    column_name: "tenant_id".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Tenant identifier for isolation".to_string()),
                    sample_values: vec!["tenant_1".to_string()],
                    is_foreign_key: true,
                    references_table: Some("tenants".to_string()),
                    references_column: Some("id".to_string()),
                },
                ColumnSchema {
                    column_name: "metadata".to_string(),
                    data_type: "JSONB".to_string(),
                    is_nullable: true,
                    description: Some("Metadata associated with the vector".to_string()),
                    sample_values: vec![r#"{"category": "electronics", "price": 299.99}"#.to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
            ],
            primary_key: Some("id".to_string()),
            indexes: vec!["idx_collection_id".to_string(), "idx_tenant_id".to_string()],
            row_count_estimate: Some(1000000),
        });

        // Tenants table
        schemas.insert("tenants".to_string(), TableSchema {
            table_name: "tenants".to_string(),
            description: Some("Multi-tenant organizations and their configurations".to_string()),
            columns: vec![
                ColumnSchema {
                    column_name: "id".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Unique tenant identifier".to_string()),
                    sample_values: vec!["tenant_1".to_string(), "company_abc".to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
                ColumnSchema {
                    column_name: "name".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    description: Some("Tenant organization name".to_string()),
                    sample_values: vec!["Acme Corp".to_string(), "TechStart Inc".to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
                ColumnSchema {
                    column_name: "subscription_tier".to_string(),
                    data_type: "VARCHAR(50)".to_string(),
                    is_nullable: false,
                    description: Some("Subscription tier: basic, professional, enterprise".to_string()),
                    sample_values: vec!["enterprise".to_string(), "professional".to_string()],
                    is_foreign_key: false,
                    references_table: None,
                    references_column: None,
                },
            ],
            primary_key: Some("id".to_string()),
            indexes: vec!["idx_subscription_tier".to_string()],
            row_count_estimate: Some(100),
        });

        Ok(schemas)
    }

    /// Build table relationships
    async fn build_table_relationships(&self) -> Result<Vec<TableRelationship>> {
        Ok(vec![
            TableRelationship {
                from_table: "collections".to_string(),
                from_column: "tenant_id".to_string(),
                to_table: "tenants".to_string(),
                to_column: "id".to_string(),
                relationship_type: RelationshipType::ManyToOne,
            },
            TableRelationship {
                from_table: "vectors".to_string(),
                from_column: "collection_id".to_string(),
                to_table: "collections".to_string(),
                to_column: "id".to_string(),
                relationship_type: RelationshipType::ManyToOne,
            },
            TableRelationship {
                from_table: "vectors".to_string(),
                from_column: "tenant_id".to_string(),
                to_table: "tenants".to_string(),
                to_column: "id".to_string(),
                relationship_type: RelationshipType::ManyToOne,
            },
        ])
    }
}

impl Default for SchemaContextConfig {
    fn default() -> Self {
        Self {
            include_column_descriptions: true,
            include_sample_values: true,
            include_relationships: true,
            max_sample_values_per_column: 3,
            enable_schema_caching: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_schema_context_creation() {
        let schema_context = SchemaContext::new().await.unwrap();

        assert!(!schema_context.table_schemas.is_empty());
        assert!(schema_context.table_schemas.contains_key("collections"));
        assert!(schema_context.table_schemas.contains_key("vectors"));
        assert!(schema_context.table_schemas.contains_key("tenants"));
    }

    #[tokio::test]
    async fn test_schema_context_building() {
        let schema_context = SchemaContext::new().await.unwrap();
        let accessible_tables = vec!["collections".to_string(), "vectors".to_string()];

        let context_description = schema_context.build_context(&accessible_tables).await.unwrap();

        assert!(context_description.contains("DATABASE SCHEMA"));
        assert!(context_description.contains("collections"));
        assert!(context_description.contains("vectors"));
        assert!(context_description.contains("tenant_id"));
        assert!(!context_description.contains("tenants")); // Not in accessible tables
    }

    #[test]
    fn test_table_access_validation() {
        let schema_context = SchemaContext {
            table_schemas: HashMap::new(),
            relationships: vec![],
            config: SchemaContextConfig::default(),
        };

        let accessible_tables = vec!["collections".to_string(), "vectors".to_string()];

        assert!(schema_context.validate_table_access("collections", &accessible_tables));
        assert!(schema_context.validate_table_access("vectors", &accessible_tables));
        assert!(!schema_context.validate_table_access("tenants", &accessible_tables));
    }
}