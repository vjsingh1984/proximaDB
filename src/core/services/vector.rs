//! Vector service types including unified VectorRecord

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// Unified vector record that replaces all schema_types and unified_types versions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorRecord {
    /// Vector identifier (required for unified schema, optional for schema_types compatibility)
    pub id: String,
    /// Collection identifier
    pub collection_id: String,
    /// Vector data as float array (required)
    pub vector: Vec<f32>,
    /// Optional metadata as key-value pairs
    pub metadata: HashMap<String, serde_json::Value>,
    /// Creation timestamp
    pub timestamp: DateTime<Utc>,
    /// Creation timestamp (unified schema field)
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
    /// TTL support for MVCC and automatic cleanup
    pub expires_at: Option<DateTime<Utc>>,
    /// Record version for optimistic concurrency (schema_types compatibility)
    pub version: i64,
}

impl VectorRecord {
    /// Create a new vector record with minimal fields
    pub fn new(
        id: String,
        collection_id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            collection_id,
            vector,
            metadata,
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
        }
    }
    
    /// Create vector record with explicit timestamp
    pub fn with_timestamp(
        id: String,
        collection_id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, serde_json::Value>,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            collection_id,
            vector,
            metadata,
            timestamp,
            created_at: timestamp,
            updated_at: timestamp,
            expires_at: None,
            version: 1,
        }
    }
    
    /// Set TTL for automatic cleanup
    pub fn with_ttl(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }
    
    /// Update the record and increment version
    pub fn update(&mut self) -> &mut Self {
        self.updated_at = Utc::now();
        self.version += 1;
        self
    }
    
    /// Check if record has expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }
}

/// Convert from schema_types VectorRecord (which has optional id)
impl From<crate::core::avro_unified::VectorRecord> for VectorRecord {
    fn from(schema_record: crate::core::avro_unified::VectorRecord) -> Self {
        let now = Utc::now();
        let timestamp = DateTime::from_timestamp_millis(schema_record.timestamp).unwrap_or(now);
        
        let expires_at = schema_record.expires_at.map(|ts| {
            DateTime::from_timestamp(ts, 0).unwrap_or(now)
        });
        
        Self {
            id: schema_record.id,
            collection_id: "unknown".to_string(), // schema_types doesn't have collection_id
            vector: schema_record.vector,
            metadata: schema_record.metadata,
            timestamp,
            created_at: timestamp,
            updated_at: timestamp,
            expires_at,
            version: schema_record.version,
        }
    }
}

/// Convert to schema_types VectorRecord for API compatibility
impl From<VectorRecord> for crate::core::avro_unified::VectorRecord {
    fn from(unified_record: VectorRecord) -> Self {
        Self {
            id: unified_record.id,
            collection_id: unified_record.collection_id,
            vector: unified_record.vector,
            metadata: unified_record.metadata,
            timestamp: unified_record.timestamp.timestamp_millis(),
            created_at: unified_record.created_at.timestamp_millis(),
            updated_at: unified_record.updated_at.timestamp_millis(),
            expires_at: unified_record.expires_at.map(|dt| dt.timestamp_millis()),
            version: unified_record.version,
            rank: None,
            score: None,
            distance: None,
        }
    }
}