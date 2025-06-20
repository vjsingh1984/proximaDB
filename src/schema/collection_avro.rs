// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Collection Avro Schema - Single Source of Truth
//!
//! This module defines the canonical Avro schema and record structure for collections.
//! Used by:
//! - Filestore metadata backend for persistence
//! - gRPC handlers for API operations  
//! - Recovery and compaction logic
//! - WAL operations
//!
//! Design principles:
//! - Single schema definition to avoid duplicates
//! - Minimal translation between gRPC and storage
//! - Pure Avro records for maximum efficiency
//! - Schema evolution support with versioning

use anyhow::{Context, Result};
use apache_avro::Schema;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical Avro schema for collection metadata - SINGLE SOURCE OF TRUTH
pub const COLLECTION_AVRO_SCHEMA: &str = r#"
{
  "type": "record",
  "name": "CollectionRecord",
  "namespace": "ai.proximadb.schema",
  "doc": "Collection metadata with schema evolution support",
  "fields": [
    {"name": "uuid", "type": "string", "doc": "Internal UUID for storage organization"},
    {"name": "name", "type": "string", "doc": "User-provided collection name (unique)"},
    {"name": "display_name", "type": "string", "doc": "User-friendly display name"},
    {"name": "dimension", "type": "int", "doc": "Vector dimension"},
    {"name": "distance_metric", "type": {"type": "enum", "name": "DistanceMetric", "symbols": ["COSINE", "EUCLIDEAN", "DOT_PRODUCT", "HAMMING"]}, "default": "COSINE"},
    {"name": "indexing_algorithm", "type": {"type": "enum", "name": "IndexingAlgorithm", "symbols": ["HNSW", "IVF", "PQ", "FLAT", "ANNOY"]}, "default": "HNSW"},
    {"name": "storage_engine", "type": {"type": "enum", "name": "StorageEngine", "symbols": ["VIPER", "LSM", "MMAP", "HYBRID"]}, "default": "VIPER"},
    {"name": "created_at", "type": "long", "doc": "Creation timestamp (epoch millis)"},
    {"name": "updated_at", "type": "long", "doc": "Last update timestamp (epoch millis)"},
    {"name": "version", "type": "long", "doc": "Metadata version for optimistic locking", "default": 1},
    {"name": "vector_count", "type": "long", "doc": "Current number of vectors", "default": 0},
    {"name": "total_size_bytes", "type": "long", "doc": "Total storage size in bytes", "default": 0},
    {"name": "config", "type": "string", "doc": "JSON serialized configuration", "default": "{}"},
    {"name": "description", "type": ["null", "string"], "doc": "Optional description", "default": null},
    {"name": "tags", "type": {"type": "array", "items": "string"}, "doc": "User-defined tags", "default": []},
    {"name": "owner", "type": ["null", "string"], "doc": "Owner identifier", "default": null},
    {"name": "access_pattern", "type": {"type": "enum", "name": "AccessPattern", "symbols": ["HOT", "NORMAL", "COLD", "ARCHIVE"]}, "default": "NORMAL"},
    {"name": "filterable_fields", "type": {"type": "array", "items": "string"}, "doc": "Metadata fields that support filtering", "default": []},
    {"name": "indexing_config", "type": "string", "doc": "JSON serialized indexing configuration", "default": "{}"},
    {"name": "retention_policy", "type": ["null", "string"], "doc": "JSON serialized retention policy", "default": null},
    {"name": "schema_version", "type": "int", "doc": "Schema version for evolution", "default": 1}
  ]
}
"#;

/// Collection record - pure Avro structure for maximum efficiency
/// This is the ONLY collection metadata structure used throughout the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionRecord {
    pub uuid: String,
    pub name: String,
    pub display_name: String,
    pub dimension: i32,
    pub distance_metric: DistanceMetric,
    pub indexing_algorithm: IndexingAlgorithm,
    pub storage_engine: StorageEngine,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
    pub vector_count: i64,
    pub total_size_bytes: i64,
    pub config: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub access_pattern: AccessPattern,
    pub filterable_fields: Vec<String>,
    pub indexing_config: String,
    pub retention_policy: Option<String>,
    pub schema_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DistanceMetric {
    #[serde(rename = "COSINE")]
    Cosine,
    #[serde(rename = "EUCLIDEAN")]
    Euclidean,
    #[serde(rename = "DOT_PRODUCT")]
    DotProduct,
    #[serde(rename = "HAMMING")]
    Hamming,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IndexingAlgorithm {
    #[serde(rename = "HNSW")]
    Hnsw,
    #[serde(rename = "IVF")]
    Ivf,
    #[serde(rename = "PQ")]
    Pq,
    #[serde(rename = "FLAT")]
    Flat,
    #[serde(rename = "ANNOY")]
    Annoy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StorageEngine {
    #[serde(rename = "VIPER")]
    Viper,
    #[serde(rename = "LSM")]
    Lsm,
    #[serde(rename = "MMAP")]
    Mmap,
    #[serde(rename = "HYBRID")]
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AccessPattern {
    #[serde(rename = "HOT")]
    Hot,
    #[serde(rename = "NORMAL")]
    Normal,
    #[serde(rename = "COLD")]
    Cold,
    #[serde(rename = "ARCHIVE")]
    Archive,
}

impl Default for DistanceMetric {
    fn default() -> Self {
        Self::Cosine
    }
}

impl Default for IndexingAlgorithm {
    fn default() -> Self {
        Self::Hnsw
    }
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::Viper
    }
}

impl Default for AccessPattern {
    fn default() -> Self {
        Self::Normal
    }
}

impl CollectionRecord {
    /// Create new collection record with generated UUID
    pub fn new(name: String, dimension: i32) -> Self {
        let uuid = Uuid::new_v4().to_string();
        let display_name = name.clone();
        let now = Utc::now().timestamp_millis();
        
        Self {
            uuid,
            name,
            display_name,
            dimension,
            distance_metric: DistanceMetric::default(),
            indexing_algorithm: IndexingAlgorithm::default(),
            storage_engine: StorageEngine::default(),
            created_at: now,
            updated_at: now,
            version: 1,
            vector_count: 0,
            total_size_bytes: 0,
            config: "{}".to_string(),
            description: None,
            tags: Vec::new(),
            owner: None,
            access_pattern: AccessPattern::default(),
            filterable_fields: Vec::new(),
            indexing_config: "{}".to_string(),
            retention_policy: None,
            schema_version: 1,
        }
    }
    
    /// Get storage path for this collection (UUID-based organization)
    pub fn storage_path(&self, base_path: &str) -> String {
        format!("{}/{}", base_path, self.uuid)
    }
    
    /// Get UUID as Uuid type
    pub fn get_uuid(&self) -> Result<Uuid> {
        Uuid::parse_str(&self.uuid).context("Invalid UUID in collection record")
    }
    
    /// Update vector count and size statistics
    pub fn update_stats(&mut self, vector_delta: i64, size_delta: i64) {
        self.vector_count = (self.vector_count + vector_delta).max(0);
        self.total_size_bytes = (self.total_size_bytes + size_delta).max(0);
        self.updated_at = Utc::now().timestamp_millis();
        self.version += 1;
    }
    
    /// Check if this record is newer than another (for compaction)
    pub fn is_newer_than(&self, other: &CollectionRecord) -> bool {
        self.version > other.version || 
        (self.version == other.version && self.updated_at > other.updated_at)
    }
    
    /// Merge updates from another record (for compaction)
    pub fn merge_from(&mut self, other: &CollectionRecord) {
        if other.is_newer_than(self) {
            // Take all fields from the newer record
            *self = other.clone();
        }
    }
}

/// gRPC to Avro conversion utilities
impl CollectionRecord {
    /// Convert from gRPC CollectionConfig to Avro CollectionRecord
    pub fn from_grpc_config(
        name: String,
        grpc_config: &crate::proto::proximadb::CollectionConfig,
    ) -> Result<Self> {
        let mut record = Self::new(name, grpc_config.dimension);
        
        // Map gRPC enums to Avro enums
        record.distance_metric = match grpc_config.distance_metric {
            1 => DistanceMetric::Cosine,
            2 => DistanceMetric::Euclidean,
            3 => DistanceMetric::DotProduct,
            4 => DistanceMetric::Hamming,
            _ => DistanceMetric::Cosine,
        };
        
        record.indexing_algorithm = match grpc_config.indexing_algorithm {
            1 => IndexingAlgorithm::Hnsw,
            2 => IndexingAlgorithm::Ivf,
            3 => IndexingAlgorithm::Pq,
            4 => IndexingAlgorithm::Flat,
            5 => IndexingAlgorithm::Annoy,
            _ => IndexingAlgorithm::Hnsw,
        };
        
        record.storage_engine = match grpc_config.storage_engine {
            1 => StorageEngine::Viper,
            2 => StorageEngine::Lsm,
            3 => StorageEngine::Mmap,
            4 => StorageEngine::Hybrid,
            _ => StorageEngine::Viper,
        };
        
        record.filterable_fields = grpc_config.filterable_metadata_fields.clone();
        record.indexing_config = serde_json::to_string(&grpc_config.indexing_config)?;
        
        Ok(record)
    }
    
    /// Convert to gRPC CollectionConfig format
    pub fn to_grpc_config(&self) -> crate::proto::proximadb::CollectionConfig {
        crate::proto::proximadb::CollectionConfig {
            name: self.name.clone(),
            dimension: self.dimension,
            distance_metric: match self.distance_metric {
                DistanceMetric::Cosine => 1,
                DistanceMetric::Euclidean => 2,
                DistanceMetric::DotProduct => 3,
                DistanceMetric::Hamming => 4,
            },
            storage_engine: match self.storage_engine {
                StorageEngine::Viper => 1,
                StorageEngine::Lsm => 2,
                StorageEngine::Mmap => 3,
                StorageEngine::Hybrid => 4,
            },
            indexing_algorithm: match self.indexing_algorithm {
                IndexingAlgorithm::Hnsw => 1,
                IndexingAlgorithm::Ivf => 2,
                IndexingAlgorithm::Pq => 3,
                IndexingAlgorithm::Flat => 4,
                IndexingAlgorithm::Annoy => 5,
            },
            filterable_metadata_fields: self.filterable_fields.clone(),
            indexing_config: serde_json::from_str(&self.indexing_config).unwrap_or_default(),
        }
    }
}

/// Schema utilities
pub struct CollectionAvroSchema;

impl CollectionAvroSchema {
    /// Get the canonical Avro schema
    pub fn get_schema() -> Result<Schema> {
        Schema::parse_str(COLLECTION_AVRO_SCHEMA)
            .context("Failed to parse collection Avro schema")
    }
    
    /// Serialize record to Avro binary
    pub fn serialize_record(record: &CollectionRecord) -> Result<Vec<u8>> {
        let schema = Self::get_schema()?;
        let mut writer = apache_avro::Writer::new(&schema, Vec::new());
        writer.append_ser(record)?;
        Ok(writer.into_inner()?)
    }
    
    /// Deserialize record from Avro binary
    pub fn deserialize_record(data: &[u8]) -> Result<CollectionRecord> {
        let reader = apache_avro::Reader::new(data)?;
        for value in reader {
            let record: CollectionRecord = apache_avro::from_value(&value?)?;
            return Ok(record);
        }
        Err(anyhow::anyhow!("No record found in Avro data"))
    }
    
    /// Validate schema compatibility for evolution
    pub fn validate_schema_evolution(old_schema: &str, new_schema: &str) -> Result<bool> {
        let old = Schema::parse_str(old_schema)?;
        let new = Schema::parse_str(new_schema)?;
        
        // Basic compatibility check - in production this would be more sophisticated
        Ok(old.name() == new.name())
    }
}

/// Collection operation types for compaction logic
#[derive(Debug, Clone, PartialEq)]
pub enum CollectionOperation {
    Insert(CollectionRecord),
    Update(CollectionRecord),
    Delete(String), // Collection name
}

impl CollectionOperation {
    /// Get the collection name this operation affects
    pub fn collection_name(&self) -> &str {
        match self {
            CollectionOperation::Insert(record) => &record.name,
            CollectionOperation::Update(record) => &record.name,
            CollectionOperation::Delete(name) => name,
        }
    }
    
    /// Check if this is a deletion operation
    pub fn is_delete(&self) -> bool {
        matches!(self, CollectionOperation::Delete(_))
    }
    
    /// Get the record if this is an insert/update operation
    pub fn get_record(&self) -> Option<&CollectionRecord> {
        match self {
            CollectionOperation::Insert(record) | CollectionOperation::Update(record) => Some(record),
            CollectionOperation::Delete(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collection_record_creation() {
        let record = CollectionRecord::new("test_collection".to_string(), 128);
        
        assert_eq!(record.name, "test_collection");
        assert_eq!(record.dimension, 128);
        assert_eq!(record.vector_count, 0);
        assert_eq!(record.version, 1);
        assert!(!record.uuid.is_empty());
    }

    #[test]
    fn test_schema_parsing() {
        let schema = CollectionAvroSchema::get_schema();
        assert!(schema.is_ok());
    }

    #[test]
    fn test_record_serialization() {
        let record = CollectionRecord::new("test".to_string(), 64);
        
        let serialized = CollectionAvroSchema::serialize_record(&record);
        assert!(serialized.is_ok());
        
        let deserialized = CollectionAvroSchema::deserialize_record(&serialized.unwrap());
        assert!(deserialized.is_ok());
        assert_eq!(deserialized.unwrap(), record);
    }

    #[test]
    fn test_version_comparison() {
        let mut record1 = CollectionRecord::new("test".to_string(), 64);
        let mut record2 = record1.clone();
        
        record2.version = 2;
        assert!(record2.is_newer_than(&record1));
        assert!(!record1.is_newer_than(&record2));
        
        record1.merge_from(&record2);
        assert_eq!(record1.version, 2);
    }
}