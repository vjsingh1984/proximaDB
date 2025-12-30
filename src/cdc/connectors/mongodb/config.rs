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

//! MongoDB connector configuration

use std::time::Duration;
use serde::{Deserialize, Serialize};

/// MongoDB connector configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoDbConfig {
    /// MongoDB connection URI
    pub connection_uri: String,

    /// Database to monitor (None = all databases)
    pub database: Option<String>,

    /// Collections to monitor
    pub collections: Vec<MongoCollectionConfig>,

    /// Full document option for updates
    pub full_document: FullDocumentOption,

    /// Maximum await time for change stream
    #[serde(default = "default_max_await_time")]
    pub max_await_time: Duration,

    /// Batch size for change stream
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,

    /// Connection timeout
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: Duration,

    /// Server selection timeout
    #[serde(default = "default_server_selection_timeout")]
    pub server_selection_timeout: Duration,
}

fn default_max_await_time() -> Duration {
    Duration::from_secs(1)
}

fn default_batch_size() -> u32 {
    100
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_server_selection_timeout() -> Duration {
    Duration::from_secs(30)
}

impl Default for MongoDbConfig {
    fn default() -> Self {
        Self {
            connection_uri: String::new(),
            database: None,
            collections: Vec::new(),
            full_document: FullDocumentOption::default(),
            max_await_time: default_max_await_time(),
            batch_size: default_batch_size(),
            connect_timeout: default_connect_timeout(),
            server_selection_timeout: default_server_selection_timeout(),
        }
    }
}

impl MongoDbConfig {
    /// Create a new MongoDB configuration
    pub fn new(connection_uri: impl Into<String>) -> Self {
        Self {
            connection_uri: connection_uri.into(),
            ..Default::default()
        }
    }

    /// Set database to monitor
    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    /// Add a collection configuration
    pub fn with_collection(mut self, collection: MongoCollectionConfig) -> Self {
        self.collections.push(collection);
        self
    }

    /// Set full document option
    pub fn with_full_document(mut self, option: FullDocumentOption) -> Self {
        self.full_document = option;
        self
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.connection_uri.is_empty() {
            return Err("Connection URI is required".to_string());
        }
        Ok(())
    }

    /// Get collection configuration
    pub fn get_collection(&self, database: &str, collection: &str) -> Option<&MongoCollectionConfig> {
        self.collections
            .iter()
            .find(|c| {
                c.database.as_deref() == Some(database) || c.database.is_none()
            })
            .filter(|c| c.name == collection)
    }

    /// Check if a collection should be captured
    pub fn should_capture(&self, database: &str, collection: &str) -> bool {
        // If no collections specified, capture all
        if self.collections.is_empty() {
            return true;
        }

        // Check if collection is in the list
        self.collections.iter().any(|c| {
            let db_match = c.database.as_deref() == Some(database) || c.database.is_none();
            let coll_match = c.name == collection || c.name == "*";
            db_match && coll_match
        })
    }
}

/// MongoDB collection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoCollectionConfig {
    /// Collection name (* for all)
    pub name: String,

    /// Database name (None = use config database)
    pub database: Option<String>,

    /// Field containing the primary key (default: _id)
    pub key_field: String,

    /// Field containing vector data
    pub vector_field: Option<String>,

    /// Fields to embed into vectors
    pub embed_fields: Option<Vec<String>>,

    /// Fields to include as metadata
    pub metadata_fields: Vec<String>,

    /// Pipeline stages to add to change stream
    pub pipeline: Vec<serde_json::Value>,
}

impl MongoCollectionConfig {
    /// Create a new collection configuration
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            database: None,
            key_field: "_id".to_string(),
            vector_field: None,
            embed_fields: None,
            metadata_fields: Vec::new(),
            pipeline: Vec::new(),
        }
    }

    /// Set database
    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    /// Set key field
    pub fn with_key_field(mut self, field: impl Into<String>) -> Self {
        self.key_field = field.into();
        self
    }

    /// Set vector field
    pub fn with_vector_field(mut self, field: impl Into<String>) -> Self {
        self.vector_field = Some(field.into());
        self
    }

    /// Set embed fields
    pub fn with_embed_fields(mut self, fields: Vec<String>) -> Self {
        self.embed_fields = Some(fields);
        self
    }

    /// Set metadata fields
    pub fn with_metadata_fields(mut self, fields: Vec<String>) -> Self {
        self.metadata_fields = fields;
        self
    }

    /// Add a pipeline stage
    pub fn with_pipeline_stage(mut self, stage: serde_json::Value) -> Self {
        self.pipeline.push(stage);
        self
    }

    /// Get full collection name
    pub fn full_name(&self, default_db: Option<&str>) -> String {
        let db = self.database.as_deref().or(default_db).unwrap_or("test");
        format!("{}.{}", db, self.name)
    }
}

/// Full document option for change streams
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FullDocumentOption {
    /// Don't return full document
    #[default]
    Default,
    /// Return full document for inserts and updates
    UpdateLookup,
    /// Return full document before change (MongoDB 6.0+)
    WhenAvailable,
    /// Require full document (error if not available)
    Required,
}

impl FullDocumentOption {
    /// Convert to MongoDB driver option string
    pub fn as_str(&self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::UpdateLookup => Some("updateLookup"),
            Self::WhenAvailable => Some("whenAvailable"),
            Self::Required => Some("required"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mongodb_config_default() {
        let config = MongoDbConfig::default();
        assert!(config.connection_uri.is_empty());
        assert!(config.database.is_none());
        assert!(config.collections.is_empty());
    }

    #[test]
    fn test_mongodb_config_builder() {
        let config = MongoDbConfig::new("mongodb://localhost:27017")
            .with_database("mydb")
            .with_full_document(FullDocumentOption::UpdateLookup)
            .with_collection(MongoCollectionConfig::new("users"));

        assert_eq!(config.connection_uri, "mongodb://localhost:27017");
        assert_eq!(config.database, Some("mydb".to_string()));
        assert_eq!(config.full_document, FullDocumentOption::UpdateLookup);
        assert_eq!(config.collections.len(), 1);
    }

    #[test]
    fn test_config_validation() {
        let config = MongoDbConfig::default();
        assert!(config.validate().is_err());

        let config = MongoDbConfig::new("mongodb://localhost:27017");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_collection_config() {
        let coll = MongoCollectionConfig::new("products")
            .with_database("shop")
            .with_vector_field("embedding")
            .with_metadata_fields(vec!["name".to_string(), "price".to_string()]);

        assert_eq!(coll.full_name(None), "shop.products");
        assert_eq!(coll.full_name(Some("default")), "shop.products");

        let coll2 = MongoCollectionConfig::new("users");
        assert_eq!(coll2.full_name(Some("mydb")), "mydb.users");
    }

    #[test]
    fn test_should_capture() {
        let config = MongoDbConfig::new("mongodb://localhost")
            .with_collection(MongoCollectionConfig::new("users").with_database("db1"))
            .with_collection(MongoCollectionConfig::new("*").with_database("db2"));

        assert!(config.should_capture("db1", "users"));
        assert!(!config.should_capture("db1", "orders"));
        assert!(config.should_capture("db2", "anything"));

        // Empty collections = capture all
        let config_all = MongoDbConfig::new("mongodb://localhost");
        assert!(config_all.should_capture("any", "collection"));
    }

    #[test]
    fn test_full_document_options() {
        assert_eq!(FullDocumentOption::Default.as_str(), None);
        assert_eq!(FullDocumentOption::UpdateLookup.as_str(), Some("updateLookup"));
        assert_eq!(FullDocumentOption::WhenAvailable.as_str(), Some("whenAvailable"));
        assert_eq!(FullDocumentOption::Required.as_str(), Some("required"));
    }

    #[test]
    fn test_config_serialization() {
        let config = MongoDbConfig::new("mongodb://localhost:27017")
            .with_database("test")
            .with_collection(MongoCollectionConfig::new("coll1"));

        let json = serde_json::to_string(&config).unwrap();
        let parsed: MongoDbConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.connection_uri, config.connection_uri);
        assert_eq!(parsed.database, config.database);
    }

    #[test]
    fn test_collection_with_pipeline() {
        let stage = serde_json::json!({
            "$match": {
                "operationType": "insert"
            }
        });

        let coll = MongoCollectionConfig::new("events")
            .with_pipeline_stage(stage.clone());

        assert_eq!(coll.pipeline.len(), 1);
        assert_eq!(coll.pipeline[0], stage);
    }

    #[test]
    fn test_key_field_default() {
        let coll = MongoCollectionConfig::new("test");
        assert_eq!(coll.key_field, "_id");

        let coll = MongoCollectionConfig::new("test")
            .with_key_field("custom_id");
        assert_eq!(coll.key_field, "custom_id");
    }
}
