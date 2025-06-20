// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Storage Policy System - Replaces Storage Tiers
//!
//! This module provides a flexible storage policy system that directly uses
//! filesystem URLs instead of abstract storage tiers. This simplifies the
//! architecture by removing the double abstraction of tiers + filesystem.
//!
//! ## Benefits
//! - Direct control over storage locations
//! - Cloud-native with natural mapping to S3/GCS/Azure
//! - Simpler architecture with one abstraction layer
//! - Better performance without tier routing overhead

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Storage policy for a collection
/// Defines where different types of data are stored using filesystem URLs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePolicy {
    /// Collection name this policy applies to
    pub collection_name: String,
    
    /// Primary storage for hot/active data (e.g., "file:///fast-ssd/data")
    pub primary_storage: String,
    
    /// Secondary storage for warm data (e.g., "s3://bucket/warm-data")
    pub secondary_storage: Option<String>,
    
    /// Archive storage for cold data (e.g., "s3://glacier/archive")
    pub archive_storage: Option<String>,
    
    /// WAL storage location (e.g., "file:///nvme/wal")
    pub wal_storage: String,
    
    /// Metadata storage location (e.g., "s3://bucket/metadata")
    pub metadata_storage: String,
    
    /// Index storage location (e.g., "file:///ssd/indexes")
    pub index_storage: String,
    
    /// Custom storage locations for specific data types
    pub custom_storage: HashMap<String, String>,
    
    /// Data lifecycle policies
    pub lifecycle: StorageLifecycle,
}

/// Storage lifecycle configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLifecycle {
    /// Move data to secondary storage after this duration (seconds)
    pub move_to_secondary_after_seconds: Option<u64>,
    
    /// Move data to archive storage after this duration (seconds)
    pub move_to_archive_after_seconds: Option<u64>,
    
    /// Delete data after this duration (seconds)
    pub delete_after_seconds: Option<u64>,
    
    /// Keep recent data in cache (number of days)
    pub cache_recent_days: u32,
    
    /// Compression policy
    pub compression: CompressionPolicy,
}

/// Compression policy for storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionPolicy {
    /// Enable compression for primary storage
    pub compress_primary: bool,
    
    /// Enable compression for secondary storage
    pub compress_secondary: bool,
    
    /// Always compress archive storage
    pub compress_archive: bool,
    
    /// Compression algorithm (e.g., "lz4", "zstd", "snappy")
    pub algorithm: String,
    
    /// Compression level (1-9, higher = better compression)
    pub level: u8,
}

/// Storage location with performance characteristics
#[derive(Debug, Clone)]
pub struct StorageLocation {
    /// Filesystem URL (e.g., "s3://bucket/path")
    pub url: String,
    
    /// Expected latency in microseconds
    pub expected_latency_us: u64,
    
    /// Expected throughput in MB/s
    pub expected_throughput_mbps: f64,
    
    /// Cost per GB per month (for cloud storage)
    pub cost_per_gb_month: Option<f64>,
    
    /// Whether this storage supports direct memory mapping
    pub supports_mmap: bool,
    
    /// Whether this storage supports streaming
    pub supports_streaming: bool,
}

impl StoragePolicy {
    /// Create a simple local-only storage policy
    pub fn local_only(collection_name: String, base_path: &str) -> Self {
        Self {
            collection_name,
            primary_storage: format!("file://{}/data", base_path),
            secondary_storage: None,
            archive_storage: None,
            wal_storage: format!("file://{}/wal", base_path),
            metadata_storage: format!("file://{}/metadata", base_path),
            index_storage: format!("file://{}/indexes", base_path),
            custom_storage: HashMap::new(),
            lifecycle: StorageLifecycle::default(),
        }
    }
    
    /// Create a cloud-native storage policy
    pub fn cloud_native(
        collection_name: String,
        local_cache: &str,
        cloud_bucket: &str,
    ) -> Self {
        Self {
            collection_name,
            primary_storage: format!("file://{}/cache", local_cache),
            secondary_storage: Some(format!("s3://{}/data", cloud_bucket)),
            archive_storage: Some(format!("s3://{}/archive", cloud_bucket)),
            wal_storage: format!("file://{}/wal", local_cache),
            metadata_storage: format!("s3://{}/metadata", cloud_bucket),
            index_storage: format!("file://{}/indexes", local_cache),
            custom_storage: HashMap::new(),
            lifecycle: StorageLifecycle {
                move_to_secondary_after_seconds: Some(86400), // 1 day
                move_to_archive_after_seconds: Some(2592000), // 30 days
                delete_after_seconds: None,
                cache_recent_days: 7,
                compression: CompressionPolicy {
                    compress_primary: false,
                    compress_secondary: true,
                    compress_archive: true,
                    algorithm: "zstd".to_string(),
                    level: 3,
                },
            },
        }
    }
    
    /// Get storage location for a specific data age
    pub fn get_storage_for_age(&self, age_seconds: u64) -> &str {
        if let Some(archive_after) = self.lifecycle.move_to_archive_after_seconds {
            if age_seconds >= archive_after {
                if let Some(ref archive) = self.archive_storage {
                    return archive;
                }
            }
        }
        
        if let Some(secondary_after) = self.lifecycle.move_to_secondary_after_seconds {
            if age_seconds >= secondary_after {
                if let Some(ref secondary) = self.secondary_storage {
                    return secondary;
                }
            }
        }
        
        &self.primary_storage
    }
    
    /// Check if data should be compressed at this age
    pub fn should_compress(&self, age_seconds: u64) -> bool {
        if let Some(archive_after) = self.lifecycle.move_to_archive_after_seconds {
            if age_seconds >= archive_after {
                return self.lifecycle.compression.compress_archive;
            }
        }
        
        if let Some(secondary_after) = self.lifecycle.move_to_secondary_after_seconds {
            if age_seconds >= secondary_after {
                return self.lifecycle.compression.compress_secondary;
            }
        }
        
        self.lifecycle.compression.compress_primary
    }
}

impl Default for StorageLifecycle {
    fn default() -> Self {
        Self {
            move_to_secondary_after_seconds: None,
            move_to_archive_after_seconds: None,
            delete_after_seconds: None,
            cache_recent_days: 7,
            compression: CompressionPolicy::default(),
        }
    }
}

impl Default for CompressionPolicy {
    fn default() -> Self {
        Self {
            compress_primary: false,
            compress_secondary: true,
            compress_archive: true,
            algorithm: "lz4".to_string(),
            level: 1,
        }
    }
}

/// Storage policy manager for all collections
pub struct StoragePolicyManager {
    /// Policies by collection name
    policies: HashMap<String, StoragePolicy>,
    
    /// Default policy for collections without specific policy
    default_policy: StoragePolicy,
}

impl StoragePolicyManager {
    /// Create new storage policy manager
    pub fn new(default_base_path: &str) -> Self {
        Self {
            policies: HashMap::new(),
            default_policy: StoragePolicy::local_only("default".to_string(), default_base_path),
        }
    }
    
    /// Set policy for a collection
    pub fn set_policy(&mut self, policy: StoragePolicy) {
        self.policies.insert(policy.collection_name.clone(), policy);
    }
    
    /// Get policy for a collection
    pub fn get_policy(&self, collection_name: &str) -> &StoragePolicy {
        self.policies.get(collection_name).unwrap_or(&self.default_policy)
    }
    
    /// Remove policy for a collection
    pub fn remove_policy(&mut self, collection_name: &str) -> Option<StoragePolicy> {
        self.policies.remove(collection_name)
    }
}