use serde::{Deserialize, Serialize};
use std::collections::HashMap;
// use uuid::Uuid; // Unused import - using String for IDs now

pub type VectorId = String;
pub type CollectionId = String;
pub type NodeId = String;
pub type Vector = Vec<f32>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorRecord {
    pub id: VectorId,
    pub collection_id: CollectionId,
    pub vector: Vector,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// TTL support for MVCC and automatic cleanup
    /// - None: Record never expires (active indefinitely)
    /// - Some(timestamp): Record expires at specified timestamp
    /// Used for soft deletes, versioning, and tombstone management
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub collection_id: CollectionId,
    pub vector: Vector,
    pub k: usize,
    pub filters: Option<HashMap<String, serde_json::Value>>,
    pub threshold: Option<f32>,
}

// NOTE: SearchResult moved to unified_types.rs to avoid duplication
pub use crate::core::unified_types::SearchResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSearchRequest {
    pub collection_id: CollectionId,
    pub query_vector: Vector,
    pub k: usize,
    pub filter: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: CollectionId,
    pub name: String,
    pub dimension: usize,
    pub schema_type: SchemaType,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaType {
    Document,
    Relational,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSchema {
    pub fields: HashMap<String, FieldType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationalSchema {
    pub tables: HashMap<String, TableSchema>,
    pub relationships: Vec<Relationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub columns: HashMap<String, ColumnType>,
    pub primary_key: Vec<String>,
    pub indexes: Vec<Index>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldType {
    String,
    Integer,
    Float,
    Boolean,
    Array(Box<FieldType>),
    Object(HashMap<String, FieldType>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnType {
    Text,
    Integer,
    Float,
    Boolean,
    Timestamp,
    Vector(usize), // dimension
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub relationship_type: RelationshipType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RelationshipType {
    OneToOne,
    OneToMany,
    ManyToMany,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub index_type: IndexType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexType {
    BTree,
    Hash,
    Vector,
}
