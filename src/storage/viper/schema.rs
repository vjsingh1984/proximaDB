//! VIPER Schema Generation Strategies
//!
//! This module implements the Strategy pattern for dynamic schema generation
//! based on collection configuration and metadata requirements.

use anyhow::Result;
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use std::sync::Arc;

use super::config::ViperSchemaBuilder;
use crate::core::CollectionId;
use crate::schema_types::CollectionConfig;

/// Strategy Pattern: Defines different schema generation strategies
pub trait SchemaGenerationStrategy: Send + Sync {
    fn generate_schema(&self) -> Result<Arc<Schema>>;
    fn get_filterable_fields(&self) -> &[String];
    fn get_collection_id(&self) -> &CollectionId;
    fn supports_ttl(&self) -> bool;
    fn get_version(&self) -> u32;
}

/// Concrete Strategy Implementation for VIPER schema generation
#[derive(Debug, Clone)]
pub struct ViperSchemaStrategy {
    collection_config: CollectionConfig,
    schema_version: u32,
    enable_ttl: bool,
    enable_extra_meta: bool,
}

impl ViperSchemaStrategy {
    pub fn new(collection_config: &CollectionConfig) -> Self {
        Self {
            collection_config: collection_config.clone(),
            schema_version: 1,
            enable_ttl: true,
            enable_extra_meta: true,
        }
    }

    pub(super) fn from_builder(
        builder: ViperSchemaBuilder,
        collection_config: &CollectionConfig,
    ) -> Self {
        Self {
            collection_config: collection_config.clone(),
            schema_version: builder.get_schema_version(),
            enable_ttl: builder.get_enable_ttl(),
            enable_extra_meta: builder.get_enable_extra_meta(),
        }
    }
}

impl SchemaGenerationStrategy for ViperSchemaStrategy {
    fn generate_schema(&self) -> Result<Arc<Schema>> {
        let mut fields = Vec::new();

        // Core required fields
        fields.push(Field::new("id", DataType::Utf8, false));
        fields.push(Field::new(
            "vectors",
            DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
            false,
        ));

        // Dynamic filterable metadata columns (up to 16)
        for (i, field_name) in self
            .collection_config
            .filterable_metadata_fields
            .iter()
            .enumerate()
        {
            if i >= 16 {
                break;
            } // Safety check
            fields.push(Field::new(
                &format!("meta_{}", field_name),
                DataType::Utf8,
                true,
            ));
        }

        // Extra metadata as Map type for unlimited additional fields
        if self.enable_extra_meta {
            let map_field = Field::new(
                "extra_meta",
                DataType::Map(
                    Arc::new(Field::new(
                        "entries",
                        DataType::Struct(
                            vec![
                                Field::new("key", DataType::Utf8, false),
                                Field::new("value", DataType::Utf8, true),
                            ]
                            .into(),
                        ),
                        false,
                    )),
                    false,
                ),
                true,
            );
            fields.push(map_field);
        }

        // TTL support
        if self.enable_ttl {
            fields.push(Field::new(
                "expires_at",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            ));
        }

        // Metadata fields for optimization
        fields.push(Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ));
        fields.push(Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ));

        Ok(Arc::new(Schema::new(fields)))
    }

    fn get_filterable_fields(&self) -> &[String] {
        &self.collection_config.filterable_metadata_fields
    }

    fn get_collection_id(&self) -> &CollectionId {
        &self.collection_config.name
    }

    fn supports_ttl(&self) -> bool {
        self.enable_ttl
    }

    fn get_version(&self) -> u32 {
        self.schema_version
    }
}

/// Alternative strategy for legacy schema support
#[derive(Debug, Clone)]
pub struct LegacySchemaStrategy {
    collection_id: CollectionId,
}

impl LegacySchemaStrategy {
    pub fn new(collection_id: CollectionId) -> Self {
        Self { collection_id }
    }
}

impl SchemaGenerationStrategy for LegacySchemaStrategy {
    fn generate_schema(&self) -> Result<Arc<Schema>> {
        // Generate simple legacy schema without dynamic metadata fields
        let fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vectors",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
                false,
            ),
            Field::new("metadata", DataType::Binary, true), // Serialized metadata
            Field::new(
                "created_at",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
        ];

        Ok(Arc::new(Schema::new(fields)))
    }

    fn get_filterable_fields(&self) -> &[String] {
        &[] // No filterable fields in legacy schema
    }

    fn get_collection_id(&self) -> &CollectionId {
        &self.collection_id
    }

    fn supports_ttl(&self) -> bool {
        false
    }

    fn get_version(&self) -> u32 {
        0 // Legacy version
    }
}
