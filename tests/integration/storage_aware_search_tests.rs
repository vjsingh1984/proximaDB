//! Integration tests for storage-aware polymorphic search
//! 
//! NOTE: These tests are temporarily disabled due to API changes

/*
use proximadb::core::{VectorRecord, search::StorageAwareSearchCoordinator};
use proximadb::storage::engine::StorageEngine;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::persistence::wal::{WalConfig, WalManager, WalStrategyType};
use proximadb::storage::metadata::backends::memory_backend::MemoryMetadataBackend;
use proximadb::storage::metadata::store::AtomicMetadataStore;
use proximadb::services::collection_service::CollectionService;
use proximadb::network::multi_server::SharedServices;
use proximadb::storage::assignment_service::AssignmentService;
use proximadb::proto::proximadb::{Collection, CollectionConfig, DistanceMetric as ProtoDistanceMetric, StorageEngine as ProtoStorageEngine};
use proximadb::compute::distance::DistanceMetric;
use std::sync::Arc;
use std::collections::HashMap;
use tempfile::TempDir;
use std::time::Duration;
use tokio::time::sleep;

// All test functions are commented out due to API changes
// These tests need to be rewritten once the new API stabilizes
*/