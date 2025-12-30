/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! MongoDB change stream event types
//!
//! This module defines types for parsing MongoDB change stream events.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// MongoDB change stream event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoChangeEvent {
    /// Resume token for this event
    #[serde(rename = "_id")]
    pub id: ResumeToken,

    /// Operation type
    #[serde(rename = "operationType")]
    pub operation_type: ChangeStreamOperation,

    /// Cluster time
    #[serde(rename = "clusterTime", skip_serializing_if = "Option::is_none")]
    pub cluster_time: Option<serde_json::Value>,

    /// Namespace (database.collection)
    pub ns: Namespace,

    /// Document key
    #[serde(rename = "documentKey", skip_serializing_if = "Option::is_none")]
    pub document_key: Option<DocumentKey>,

    /// Full document (for inserts and with fullDocument option)
    #[serde(rename = "fullDocument", skip_serializing_if = "Option::is_none")]
    pub full_document: Option<serde_json::Value>,

    /// Full document before change (MongoDB 6.0+)
    #[serde(rename = "fullDocumentBeforeChange", skip_serializing_if = "Option::is_none")]
    pub full_document_before_change: Option<serde_json::Value>,

    /// Update description (for update operations)
    #[serde(rename = "updateDescription", skip_serializing_if = "Option::is_none")]
    pub update_description: Option<UpdateDescription>,

    /// Transaction number
    #[serde(rename = "txnNumber", skip_serializing_if = "Option::is_none")]
    pub txn_number: Option<i64>,

    /// Logical session ID
    #[serde(rename = "lsid", skip_serializing_if = "Option::is_none")]
    pub lsid: Option<serde_json::Value>,
}

impl MongoChangeEvent {
    /// Get the document ID as a string
    pub fn get_id(&self) -> Option<String> {
        self.document_key.as_ref().and_then(|dk| dk.id_as_string())
    }

    /// Get full collection name
    pub fn collection_name(&self) -> String {
        format!("{}.{}", self.ns.db, self.ns.coll)
    }

    /// Check if this is an insert operation
    pub fn is_insert(&self) -> bool {
        self.operation_type == ChangeStreamOperation::Insert
    }

    /// Check if this is an update operation
    pub fn is_update(&self) -> bool {
        self.operation_type == ChangeStreamOperation::Update
    }

    /// Check if this is a delete operation
    pub fn is_delete(&self) -> bool {
        self.operation_type == ChangeStreamOperation::Delete
    }

    /// Check if this is a replace operation
    pub fn is_replace(&self) -> bool {
        self.operation_type == ChangeStreamOperation::Replace
    }
}

/// Resume token for change stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeToken {
    /// Token data (opaque)
    #[serde(rename = "_data")]
    pub data: String,
}

impl ResumeToken {
    /// Create a new resume token
    pub fn new(data: impl Into<String>) -> Self {
        Self { data: data.into() }
    }

    /// Get token as string
    pub fn as_str(&self) -> &str {
        &self.data
    }
}

/// Change stream operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeStreamOperation {
    /// Document inserted
    Insert,
    /// Document updated
    Update,
    /// Document replaced
    Replace,
    /// Document deleted
    Delete,
    /// Collection dropped
    Drop,
    /// Collection renamed
    Rename,
    /// Database dropped
    DropDatabase,
    /// Index invalidated
    Invalidate,
}

impl std::fmt::Display for ChangeStreamOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Insert => write!(f, "insert"),
            Self::Update => write!(f, "update"),
            Self::Replace => write!(f, "replace"),
            Self::Delete => write!(f, "delete"),
            Self::Drop => write!(f, "drop"),
            Self::Rename => write!(f, "rename"),
            Self::DropDatabase => write!(f, "dropDatabase"),
            Self::Invalidate => write!(f, "invalidate"),
        }
    }
}

/// Namespace (database.collection)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {
    /// Database name
    pub db: String,
    /// Collection name
    pub coll: String,
}

impl Namespace {
    /// Create a new namespace
    pub fn new(db: impl Into<String>, coll: impl Into<String>) -> Self {
        Self {
            db: db.into(),
            coll: coll.into(),
        }
    }

    /// Get full name
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.db, self.coll)
    }
}

/// Document key (typically contains _id)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentKey {
    /// Document ID
    #[serde(rename = "_id")]
    pub id: serde_json::Value,

    /// Additional key fields (for sharded collections)
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl DocumentKey {
    /// Get ID as string
    pub fn id_as_string(&self) -> Option<String> {
        match &self.id {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(obj) => {
                // Handle ObjectId: { "$oid": "..." }
                obj.get("$oid")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => Some(self.id.to_string()),
        }
    }
}

/// Update description for update operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDescription {
    /// Fields that were updated
    #[serde(rename = "updatedFields", default)]
    pub updated_fields: HashMap<String, serde_json::Value>,

    /// Fields that were removed
    #[serde(rename = "removedFields", default)]
    pub removed_fields: Vec<String>,

    /// Truncated arrays (MongoDB 5.0+)
    #[serde(rename = "truncatedArrays", default)]
    pub truncated_arrays: Vec<TruncatedArray>,
}

impl UpdateDescription {
    /// Check if any fields were updated
    pub fn has_updates(&self) -> bool {
        !self.updated_fields.is_empty()
    }

    /// Check if any fields were removed
    pub fn has_removals(&self) -> bool {
        !self.removed_fields.is_empty()
    }

    /// Get updated field value
    pub fn get_updated(&self, field: &str) -> Option<&serde_json::Value> {
        self.updated_fields.get(field)
    }
}

/// Truncated array information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncatedArray {
    /// Field path
    pub field: String,
    /// New size
    #[serde(rename = "newSize")]
    pub new_size: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_stream_operation_display() {
        assert_eq!(ChangeStreamOperation::Insert.to_string(), "insert");
        assert_eq!(ChangeStreamOperation::Update.to_string(), "update");
        assert_eq!(ChangeStreamOperation::Delete.to_string(), "delete");
    }

    #[test]
    fn test_namespace() {
        let ns = Namespace::new("mydb", "users");
        assert_eq!(ns.full_name(), "mydb.users");
    }

    #[test]
    fn test_document_key_string_id() {
        let key = DocumentKey {
            id: serde_json::json!("user123"),
            extra: HashMap::new(),
        };

        assert_eq!(key.id_as_string(), Some("user123".to_string()));
    }

    #[test]
    fn test_document_key_objectid() {
        let key = DocumentKey {
            id: serde_json::json!({"$oid": "507f1f77bcf86cd799439011"}),
            extra: HashMap::new(),
        };

        assert_eq!(
            key.id_as_string(),
            Some("507f1f77bcf86cd799439011".to_string())
        );
    }

    #[test]
    fn test_document_key_number_id() {
        let key = DocumentKey {
            id: serde_json::json!(12345),
            extra: HashMap::new(),
        };

        assert_eq!(key.id_as_string(), Some("12345".to_string()));
    }

    #[test]
    fn test_update_description() {
        let mut updated = HashMap::new();
        updated.insert("name".to_string(), serde_json::json!("new_name"));
        updated.insert("count".to_string(), serde_json::json!(42));

        let desc = UpdateDescription {
            updated_fields: updated,
            removed_fields: vec!["old_field".to_string()],
            truncated_arrays: vec![],
        };

        assert!(desc.has_updates());
        assert!(desc.has_removals());
        assert_eq!(desc.get_updated("name"), Some(&serde_json::json!("new_name")));
        assert!(desc.get_updated("missing").is_none());
    }

    #[test]
    fn test_resume_token() {
        let token = ResumeToken::new("abc123");
        assert_eq!(token.as_str(), "abc123");
    }

    #[test]
    fn test_mongo_change_event_helpers() {
        let event = MongoChangeEvent {
            id: ResumeToken::new("token"),
            operation_type: ChangeStreamOperation::Insert,
            cluster_time: None,
            ns: Namespace::new("db", "coll"),
            document_key: Some(DocumentKey {
                id: serde_json::json!("id123"),
                extra: HashMap::new(),
            }),
            full_document: None,
            full_document_before_change: None,
            update_description: None,
            txn_number: None,
            lsid: None,
        };

        assert!(event.is_insert());
        assert!(!event.is_update());
        assert!(!event.is_delete());
        assert_eq!(event.get_id(), Some("id123".to_string()));
        assert_eq!(event.collection_name(), "db.coll");
    }

    #[test]
    fn test_change_event_serialization() {
        let event = MongoChangeEvent {
            id: ResumeToken::new("token_data"),
            operation_type: ChangeStreamOperation::Update,
            cluster_time: None,
            ns: Namespace::new("test", "users"),
            document_key: Some(DocumentKey {
                id: serde_json::json!("user1"),
                extra: HashMap::new(),
            }),
            full_document: Some(serde_json::json!({"name": "Test"})),
            full_document_before_change: None,
            update_description: Some(UpdateDescription {
                updated_fields: {
                    let mut m = HashMap::new();
                    m.insert("name".to_string(), serde_json::json!("Test"));
                    m
                },
                removed_fields: vec![],
                truncated_arrays: vec![],
            }),
            txn_number: None,
            lsid: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MongoChangeEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.operation_type, ChangeStreamOperation::Update);
        assert_eq!(parsed.collection_name(), "test.users");
    }
}
