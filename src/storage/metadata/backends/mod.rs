// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metadata Storage Backends
//! 
//! Supports serverless and lightweight storage backends for metadata management:
//! - WAL-based (default) - Filesystem-based storage supporting S3/GCS/ADLS
//! - PostgreSQL/MySQL - For ACID transactions and SQL queries  
//! - SQLite - For embedded scenarios
//! - DynamoDB (AWS) - For serverless auto-scaling
//! - Cosmos DB (Azure) - For multi-model serverless database
//! - Firestore (GCP) - For serverless NoSQL document store

pub mod wal_backend;
pub mod postgres_backend;
pub mod mysql_backend;
pub mod sqlite_backend;
pub mod dynamodb_backend;
pub mod cosmosdb_backend;
pub mod firestore_backend;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

use crate::core::CollectionId;
use super::{CollectionMetadata, SystemMetadata, MetadataFilter, MetadataOperation};

/// Metadata backend configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataBackendConfig {
    /// Backend type
    pub backend_type: MetadataBackendType,
    
    /// Connection configuration
    pub connection: BackendConnectionConfig,
    
    /// Performance tuning
    pub performance: BackendPerformanceConfig,
    
    /// High availability configuration
    pub ha_config: Option<HAConfig>,
    
    /// Backup and recovery
    pub backup_config: Option<BackendBackupConfig>,
}

/// Supported metadata backend types (serverless and lightweight only)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MetadataBackendType {
    /// WAL-based filesystem storage (default) - Supports S3/GCS/ADLS
    Wal,
    /// PostgreSQL database (RDBMS)
    PostgreSQL,
    /// MySQL database (RDBMS) 
    MySQL,
    /// SQLite embedded database (for development/testing)
    SQLite,
    /// Amazon DynamoDB (AWS serverless key-value)
    DynamoDB,
    /// Azure Cosmos DB (Azure serverless multi-model)
    CosmosDB,
    /// Google Firestore (GCP serverless NoSQL)
    Firestore,
    /// Multi-backend with primary/fallback
    MultiBackend {
        primary: Box<MetadataBackendType>,
        fallback: Box<MetadataBackendType>,
    },
}

impl Default for MetadataBackendType {
    fn default() -> Self {
        Self::Wal // Default to WAL-based filesystem storage
    }
}

/// Backend connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConnectionConfig {
    /// Connection string/URL
    pub connection_string: String,
    
    /// Authentication credentials
    pub auth: Option<AuthConfig>,
    
    /// Connection pool settings
    pub pool_config: PoolConfig,
    
    /// SSL/TLS configuration
    pub ssl_config: Option<SSLConfig>,
    
    /// Region (for cloud services)
    pub region: Option<String>,
    
    /// Additional connection parameters
    pub parameters: HashMap<String, String>,
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Username
    pub username: Option<String>,
    
    /// Password
    pub password: Option<String>,
    
    /// API key (for cloud services)
    pub api_key: Option<String>,
    
    /// IAM role ARN (for AWS)
    pub iam_role: Option<String>,
    
    /// Service account (for GCP)
    pub service_account: Option<String>,
    
    /// Token-based authentication
    pub token: Option<String>,
}

/// Connection pool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Minimum pool size
    pub min_connections: u32,
    
    /// Maximum pool size
    pub max_connections: u32,
    
    /// Connection timeout in seconds
    pub connect_timeout_secs: u64,
    
    /// Idle timeout in seconds
    pub idle_timeout_secs: u64,
    
    /// Maximum lifetime in seconds
    pub max_lifetime_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_connections: 1,
            max_connections: 10,
            connect_timeout_secs: 30,
            idle_timeout_secs: 600,
            max_lifetime_secs: 1800,
        }
    }
}

/// SSL/TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SSLConfig {
    /// Enable SSL/TLS
    pub enabled: bool,
    
    /// Verify certificates
    pub verify_certificates: bool,
    
    /// Client certificate path
    pub client_cert_path: Option<String>,
    
    /// Client key path
    pub client_key_path: Option<String>,
    
    /// CA certificate path
    pub ca_cert_path: Option<String>,
}

/// Performance configuration for backends
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendPerformanceConfig {
    /// Enable read replicas
    pub enable_read_replicas: bool,
    
    /// Read replica endpoints
    pub read_replica_endpoints: Vec<String>,
    
    /// Enable caching
    pub enable_caching: bool,
    
    /// Cache TTL in seconds
    pub cache_ttl_secs: u64,
    
    /// Batch size for bulk operations
    pub batch_size: usize,
    
    /// Query timeout in seconds
    pub query_timeout_secs: u64,
    
    /// Enable compression
    pub enable_compression: bool,
}

impl Default for BackendPerformanceConfig {
    fn default() -> Self {
        Self {
            enable_read_replicas: false,
            read_replica_endpoints: Vec::new(),
            enable_caching: true,
            cache_ttl_secs: 300,
            batch_size: 100,
            query_timeout_secs: 30,
            enable_compression: true,
        }
    }
}

/// High availability configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HAConfig {
    /// Enable multi-region replication
    pub multi_region: bool,
    
    /// Replication regions
    pub regions: Vec<String>,
    
    /// Consistency level
    pub consistency_level: ConsistencyLevel,
    
    /// Failover configuration
    pub failover: FailoverConfig,
    
    /// Enable automatic backups
    pub auto_backup: bool,
}

/// Consistency levels for distributed systems
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsistencyLevel {
    /// Eventual consistency
    Eventual,
    /// Strong consistency
    Strong,
    /// Session consistency
    Session,
    /// Bounded staleness
    BoundedStaleness { max_lag_ms: u64 },
}

/// Failover configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    /// Enable automatic failover
    pub auto_failover: bool,
    
    /// Failover timeout in seconds
    pub failover_timeout_secs: u64,
    
    /// Health check interval in seconds
    pub health_check_interval_secs: u64,
    
    /// Maximum retries before failover
    pub max_retries: u32,
}

/// Backend backup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendBackupConfig {
    /// Enable automatic backups
    pub enabled: bool,
    
    /// Backup frequency in hours
    pub frequency_hours: u32,
    
    /// Backup retention in days
    pub retention_days: u32,
    
    /// Backup storage location
    pub backup_location: String,
    
    /// Enable incremental backups
    pub incremental: bool,
    
    /// Enable point-in-time recovery
    pub point_in_time_recovery: bool,
}

/// Statistics for backend monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendStats {
    /// Backend type
    pub backend_type: MetadataBackendType,
    
    /// Connection status
    pub connected: bool,
    
    /// Active connections
    pub active_connections: u32,
    
    /// Total operations
    pub total_operations: u64,
    
    /// Failed operations
    pub failed_operations: u64,
    
    /// Average latency in milliseconds
    pub avg_latency_ms: f64,
    
    /// Cache hit rate (if applicable)
    pub cache_hit_rate: Option<f64>,
    
    /// Last backup time
    pub last_backup_time: Option<chrono::DateTime<chrono::Utc>>,
    
    /// Storage size in bytes
    pub storage_size_bytes: u64,
}

/// Generic metadata backend trait
#[async_trait]
pub trait MetadataBackend: Send + Sync {
    /// Backend name for identification
    fn backend_name(&self) -> &'static str;
    
    /// Initialize the backend
    async fn initialize(&mut self, config: MetadataBackendConfig) -> Result<()>;
    
    /// Health check
    async fn health_check(&self) -> Result<bool>;
    
    /// Create collection metadata
    async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()>;
    
    /// Get collection metadata
    async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<CollectionMetadata>>;
    
    /// Update collection metadata
    async fn update_collection(&self, collection_id: &CollectionId, metadata: CollectionMetadata) -> Result<()>;
    
    /// Delete collection metadata
    async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool>;
    
    /// List collections with filtering
    async fn list_collections(&self, filter: Option<MetadataFilter>) -> Result<Vec<CollectionMetadata>>;
    
    /// Update collection statistics atomically
    async fn update_stats(&self, collection_id: &CollectionId, vector_delta: i64, size_delta: i64) -> Result<()>;
    
    /// Batch operations (atomic if supported)
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()>;
    
    /// Begin transaction (if supported)
    async fn begin_transaction(&self) -> Result<Option<String>>;
    
    /// Commit transaction (if supported)
    async fn commit_transaction(&self, transaction_id: &str) -> Result<()>;
    
    /// Rollback transaction (if supported)
    async fn rollback_transaction(&self, transaction_id: &str) -> Result<()>;
    
    /// Get system metadata
    async fn get_system_metadata(&self) -> Result<SystemMetadata>;
    
    /// Update system metadata
    async fn update_system_metadata(&self, metadata: SystemMetadata) -> Result<()>;
    
    /// Get backend statistics
    async fn get_stats(&self) -> Result<BackendStats>;
    
    /// Backup metadata
    async fn backup(&self, location: &str) -> Result<String>; // Returns backup ID
    
    /// Restore from backup
    async fn restore(&self, backup_id: &str, location: &str) -> Result<()>;
    
    /// Close/cleanup backend
    async fn close(&self) -> Result<()>;
}

/// Backend factory for creating different backend implementations
pub struct MetadataBackendFactory;

impl MetadataBackendFactory {
    /// Create backend based on configuration
    pub async fn create_backend(
        backend_type: MetadataBackendType,
        config: MetadataBackendConfig,
    ) -> Result<Box<dyn MetadataBackend>> {
        match backend_type {
            MetadataBackendType::Wal => {
                let mut backend = Box::new(wal_backend::WalMetadataBackend::new());
                backend.initialize(config).await?;
                Ok(backend)
            }
            MetadataBackendType::PostgreSQL => {
                let mut backend = Box::new(postgres_backend::PostgresMetadataBackend::new());
                backend.initialize(config).await?;
                Ok(backend)
            }
            MetadataBackendType::MySQL => {
                let mut backend = Box::new(mysql_backend::MySQLMetadataBackend::new());
                backend.initialize(config).await?;
                Ok(backend)
            }
            MetadataBackendType::SQLite => {
                let mut backend = Box::new(sqlite_backend::SQLiteMetadataBackend::new());
                backend.initialize(config).await?;
                Ok(backend)
            }
            MetadataBackendType::DynamoDB => {
                let mut backend = Box::new(dynamodb_backend::DynamoDBMetadataBackend::new());
                backend.initialize(config).await?;
                Ok(backend)
            }
            MetadataBackendType::CosmosDB => {
                let mut backend = Box::new(cosmosdb_backend::CosmosDBMetadataBackend::new());
                backend.initialize(config).await?;
                Ok(backend)
            }
            MetadataBackendType::Firestore => {
                let mut backend = Box::new(firestore_backend::FirestoreMetadataBackend::new());
                backend.initialize(config).await?;
                Ok(backend)
            }
            MetadataBackendType::MultiBackend { primary, fallback: _ } => {
                // TODO: Implement multi-backend with primary/fallback
                tracing::warn!("MultiBackend not implemented yet, falling back to primary");
                Box::pin(Self::create_backend(*primary, config)).await
            }
        }
    }
    
    /// Get available backend types
    pub fn available_backends() -> Vec<MetadataBackendType> {
        vec![
            MetadataBackendType::Wal,
            MetadataBackendType::PostgreSQL,
            MetadataBackendType::MySQL,
            MetadataBackendType::SQLite,
            MetadataBackendType::DynamoDB,
            MetadataBackendType::CosmosDB,
            MetadataBackendType::Firestore,
        ]
    }
    
    /// Get backend capabilities
    pub fn backend_capabilities(backend_type: &MetadataBackendType) -> BackendCapabilities {
        match backend_type {
            MetadataBackendType::Wal => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: true,
                supports_secondary_indexes: false,
                supports_full_text_search: false,
                supports_geo_queries: false,
                supports_auto_scaling: false,
                supports_multi_region: false,
                supports_encryption_at_rest: false,
                max_document_size_mb: 16,
                max_connections: 1000,
            },
            MetadataBackendType::PostgreSQL => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: true,
                supports_secondary_indexes: true,
                supports_full_text_search: true,
                supports_geo_queries: true,
                supports_auto_scaling: false,
                supports_multi_region: false,
                supports_encryption_at_rest: true,
                max_document_size_mb: 1024,
                max_connections: 10000,
            },
            MetadataBackendType::DynamoDB => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: false,
                supports_secondary_indexes: true,
                supports_full_text_search: false,
                supports_geo_queries: false,
                supports_auto_scaling: true,
                supports_multi_region: true,
                supports_encryption_at_rest: true,
                max_document_size_mb: 400,
                max_connections: 100000,
            },
            MetadataBackendType::SQLite => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: true,
                supports_secondary_indexes: true,
                supports_full_text_search: true,
                supports_geo_queries: false,
                supports_auto_scaling: false,
                supports_multi_region: false,
                supports_encryption_at_rest: true,
                max_document_size_mb: 1024,
                max_connections: 1,
            },
            MetadataBackendType::MultiBackend { .. } => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: true,
                supports_secondary_indexes: true,
                supports_full_text_search: true,
                supports_geo_queries: true,
                supports_auto_scaling: true,
                supports_multi_region: true,
                supports_encryption_at_rest: true,
                max_document_size_mb: 1024,
                max_connections: 100000,
            },
            MetadataBackendType::MySQL => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: true,
                supports_secondary_indexes: true,
                supports_full_text_search: true,
                supports_geo_queries: true,
                supports_auto_scaling: true,
                supports_multi_region: true,
                supports_encryption_at_rest: true,
                max_document_size_mb: 1024,
                max_connections: 10000,
            },
            MetadataBackendType::CosmosDB => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: false,
                supports_secondary_indexes: true,
                supports_full_text_search: true,
                supports_geo_queries: true,
                supports_auto_scaling: true,
                supports_multi_region: true,
                supports_encryption_at_rest: true,
                max_document_size_mb: 2,
                max_connections: 1000,
            },
            MetadataBackendType::Firestore => BackendCapabilities {
                supports_transactions: true,
                supports_schemas: false,
                supports_secondary_indexes: true,
                supports_full_text_search: false,
                supports_geo_queries: true,
                supports_auto_scaling: true,
                supports_multi_region: true,
                supports_encryption_at_rest: true,
                max_document_size_mb: 1,
                max_connections: 1000,
            },
        }
    }
}

/// Backend capabilities for feature detection
#[derive(Debug, Clone)]
pub struct BackendCapabilities {
    pub supports_transactions: bool,
    pub supports_schemas: bool,
    pub supports_secondary_indexes: bool,
    pub supports_full_text_search: bool,
    pub supports_geo_queries: bool,
    pub supports_auto_scaling: bool,
    pub supports_multi_region: bool,
    pub supports_encryption_at_rest: bool,
    pub max_document_size_mb: u32,
    pub max_connections: u32,
}

impl Default for MetadataBackendConfig {
    fn default() -> Self {
        Self {
            backend_type: MetadataBackendType::default(),
            connection: BackendConnectionConfig {
                connection_string: "file://./data/metadata".to_string(),
                auth: None,
                pool_config: PoolConfig::default(),
                ssl_config: None,
                region: None,
                parameters: HashMap::new(),
            },
            performance: BackendPerformanceConfig::default(),
            ha_config: None,
            backup_config: None,
        }
    }
}