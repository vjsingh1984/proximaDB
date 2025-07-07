//! Distributed coordination module for ProximaDB
//! 
//! This module implements the distributed architecture foundation including:
//! - Storage-aware coordination for different deployment models
//! - Deployment-specific replication strategies
//! 
//! NOTE: Distributed features are temporarily disabled for single-node optimization.
//! These modules are parked until the core single-node performance is optimized.

// Temporarily disabled for single-node optimization
// pub mod storage_aware_coordinator;
// pub mod replication_manager;

// pub use storage_aware_coordinator::StorageAwareDistributedCoordinator;
// pub use replication_manager::ReplicationManager;