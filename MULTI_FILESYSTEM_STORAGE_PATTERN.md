# Multi-Directory/Multi-Filesystem Storage Pattern for ProximaDB

## Overview

This document outlines the comprehensive multi-directory and multi-filesystem configuration pattern that should be applied consistently across all ProximaDB storage components for performance, scalability, and cloud-native deployment.

## Core Pattern

### Configuration Structure
```toml
[storage.component_config]
# Multiple storage URLs supporting different filesystem types
storage_urls = [
    "file:///mnt/nvme1/component",   # Local NVMe disk 1  
    "file:///mnt/nvme2/component",   # Local NVMe disk 2
    "s3://bucket1/component",        # S3 bucket 1
    "s3://bucket2/component",        # S3 bucket 2  
    "adls://account.dfs.core.windows.net/container/component",  # Azure
    "gcs://bucket/component"         # Google Cloud
]

# Distribution strategy
distribution_strategy = "RoundRobin"  # RoundRobin | Hash | LoadBalanced

# Collection affinity (keep related data together)
collection_affinity = true

# Component-specific settings
memory_flush_size_bytes = 1048576
cache_size_mb = 256
```

### Distribution Strategies

1. **RoundRobin** (Default for fair usage)
   - Each new collection/data assigned to next directory in sequence
   - Ensures fair distribution across all storage locations
   - Best for: Initial assignment, balanced load distribution

2. **Hash**
   - Consistent hash-based placement using collection ID
   - Same collection always goes to same directory
   - Best for: Deterministic placement, data locality

3. **LoadBalanced**
   - Dynamic assignment based on current load (count + size)
   - Selects least loaded directory at assignment time
   - Best for: Adaptive workloads, varying collection sizes

## Implementation Status

### ✅ Completed: WAL (Write-Ahead Log)

**Implementation**: `/workspace/src/storage/persistence/wal/avro.rs`

```rust
/// Collection WAL directory assignment metadata
struct CollectionWalAssignment {
    collection_id: CollectionId,
    wal_directory_url: String,
    directory_index: usize,
    assigned_at: DateTime<Utc>,
    total_size_bytes: u64,
}

/// WAL strategy with assignment tracking
pub struct AvroWalStrategy {
    collection_assignments: Arc<RwLock<HashMap<CollectionId, CollectionWalAssignment>>>,
    round_robin_counter: Arc<RwLock<usize>>,
    // ... other fields
}
```

**Features Implemented**:
- ✅ Round-robin assignment for fair distribution
- ✅ Collection → directory assignment tracking
- ✅ Metadata persistence and recovery from disk inspection
- ✅ Support for file://, s3://, adls://, gcs:// URLs
- ✅ Load balancing based on collection count and size
- ✅ Automatic discovery of existing collections during recovery

**Configuration**:
```toml
[storage.wal_config]
wal_urls = ["file:///workspace/data/wal"]
distribution_strategy = "RoundRobin"
collection_affinity = true
memory_flush_size_bytes = 1048576
```

### 🚧 Next: VIPER Storage Engine

**Target Configuration**:
```toml
[storage.viper_config]
data_urls = [
    "file:///mnt/nvme1/viper",
    "file:///mnt/nvme2/viper", 
    "s3://data-bucket1/viper",
    "s3://data-bucket2/viper"
]
distribution_strategy = "Hash"  # For data locality
collection_affinity = true
segment_size_mb = 512
```

**Implementation Plan**:
- Collection → storage directory assignment
- Parquet file distribution across directories
- Cluster assignment metadata tracking
- Recovery from directory inspection

### 📋 Planned: Collection Metadata Storage

**Target Configuration**:
```toml
[storage.collection_metadata_config]
metadata_urls = [
    "file:///workspace/data/metadata",
    "s3://metadata-bucket/collections"
]
distribution_strategy = "RoundRobin"
backup_replicas = 2  # Cross-directory replication
```

**Implementation Plan**:
- Collection metadata distribution
- Cross-directory replication for reliability
- Atomic updates across replicas

### 📋 Planned: Index Storage (AXIS)

**Target Configuration**:
```toml
[storage.index_config]
index_urls = [
    "file:///mnt/nvme1/indexes",
    "file:///mnt/nvme2/indexes",
    "s3://index-bucket/axis"
]
distribution_strategy = "LoadBalanced"
index_type_affinity = true  # Keep same index type together
```

**Implementation Plan**:
- HNSW, IVF, and hybrid index distribution
- Index type-specific assignment strategies
- Cross-directory index backup

## Core Implementation Components

### 1. Assignment Tracking Structure

```rust
/// Generic storage assignment metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StorageAssignment {
    resource_id: String,           // Collection ID, index ID, etc.
    storage_url: String,           // Assigned storage URL
    directory_index: usize,        // Index in configuration array
    assigned_at: DateTime<Utc>,    // Assignment timestamp
    last_accessed: DateTime<Utc>,  // Last access for LRU
    file_count: usize,            // Number of files
    total_size_bytes: u64,        // Total size
    resource_type: ResourceType,   // WAL, VIPER, INDEX, METADATA
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ResourceType {
    Wal,
    ViperData, 
    Index,
    Metadata,
}
```

### 2. Assignment Strategy Interface

```rust
#[async_trait]
trait StorageAssignmentStrategy {
    async fn assign_storage_url(
        &self,
        resource_id: &str,
        storage_urls: &[String],
        existing_assignments: &HashMap<String, StorageAssignment>
    ) -> Result<(String, usize)>;
    
    async fn discover_existing_assignments(
        &self,
        storage_urls: &[String],
        filesystem: &FilesystemFactory
    ) -> Result<HashMap<String, StorageAssignment>>;
}
```

### 3. Recovery and Discovery

```rust
/// Discover existing resources by inspecting storage directories
async fn discover_resources_from_directories(
    storage_urls: &[String],
    filesystem: &FilesystemFactory,
    resource_type: ResourceType
) -> Result<HashMap<String, StorageAssignment>> {
    let mut assignments = HashMap::new();
    
    for (index, url) in storage_urls.iter().enumerate() {
        // Inspect directory structure
        // Identify resource directories (UUID-like patterns)
        // Calculate sizes and file counts
        // Create assignment metadata
    }
    
    Ok(assignments)
}
```

## Configuration Examples

### Local Multi-Disk Performance

```toml
[storage.wal_config]
wal_urls = [
    "file:///mnt/nvme1/wal",
    "file:///mnt/nvme2/wal",
    "file:///mnt/sata1/wal"
]
distribution_strategy = "RoundRobin"

[storage.viper_config] 
data_urls = [
    "file:///mnt/nvme1/viper",
    "file:///mnt/nvme2/viper"
]
distribution_strategy = "Hash"  # For locality

[storage.index_config]
index_urls = [
    "file:///mnt/nvme1/indexes", 
    "file:///mnt/nvme2/indexes"
]
distribution_strategy = "LoadBalanced"
```

### Cloud Multi-Region Deployment

```toml
[storage.wal_config]
wal_urls = [
    "s3://wal-us-west/proximadb", 
    "s3://wal-us-east/proximadb"
]
distribution_strategy = "Hash"

[storage.viper_config]
data_urls = [
    "s3://data-us-west/viper",
    "s3://data-us-east/viper",
    "gcs://backup-data/viper"  # Cross-cloud backup
]
distribution_strategy = "LoadBalanced"

[storage.collection_metadata_config]
metadata_urls = [
    "s3://metadata-primary/collections",
    "adls://backup.dfs.core.windows.net/metadata/collections"
]
distribution_strategy = "RoundRobin"
backup_replicas = 2
```

### Hybrid Edge-Cloud Setup

```toml
[storage.wal_config]
wal_urls = [
    "file:///local/wal",           # Fast local disk
    "s3://cloud-wal/proximadb"     # Cloud backup
]
distribution_strategy = "LoadBalanced"

[storage.viper_config]
data_urls = [
    "file:///local/data",          # Local cache
    "s3://data-lake/viper"         # Cloud storage
]
distribution_strategy = "LoadBalanced"
```

## Benefits

### Performance
- **Multi-disk I/O**: Parallel operations across disks/buckets
- **Load distribution**: Even utilization of storage resources  
- **Locality optimization**: Related data stays together

### Scalability
- **Horizontal scaling**: Add more directories/buckets as needed
- **Cloud-native**: Seamless multi-cloud and multi-region support
- **Performance tiers**: Mix fast local and slower cloud storage

### Operational
- **Consistent configuration**: Same pattern across all components
- **Automatic discovery**: Recovery without configuration files
- **Monitoring visibility**: Track usage per directory/bucket
- **Failure isolation**: Component failures don't affect entire system

## Next Steps

1. **Apply to VIPER Storage**: Implement collection data distribution
2. **Apply to Index Storage**: Implement AXIS index distribution  
3. **Apply to Metadata Storage**: Implement metadata distribution with replication
4. **Unified Management**: Create common assignment management library
5. **Monitoring Integration**: Add metrics for directory usage and performance
6. **Testing**: Comprehensive multi-directory failure and recovery testing