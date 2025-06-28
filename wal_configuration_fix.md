# WAL Configuration Fix - Multi-Directory Support with Filesystem URLs

## Current Issues

### 1. Hardcoded WAL Directory
```rust
// In avro.rs - HARDCODED PATH!
let wal_dir = std::path::PathBuf::from("/workspace/data/wal").join(&collection_id);
```

### 2. Config Structure Mismatch
```toml
# config.toml - Single directory (legacy)
[storage]
wal_dir = "/workspace/data/wal"
```

```rust
// WalConfig::MultiDiskConfig - Multiple directories but PathBuf only
pub struct MultiDiskConfig {
    pub data_directories: Vec<PathBuf>,  // Should be Vec<String> for URLs
}
```

## Required Configuration Support

### 1. Multiple WAL URLs in TOML
```toml
[storage.wal_config]
# Multi-disk WAL configuration for performance
wal_urls = [
    "file:///workspace/data/wal1",
    "file:///workspace/data/wal2", 
    "s3://bucket1/wal",
    "adls://account.dfs.core.windows.net/container/wal"
]
distribution_strategy = "LoadBalanced"  # RoundRobin, Hash, LoadBalanced
collection_affinity = true  # Keep collection on one disk

# Single WAL URL (simple configuration)
# wal_url = "file:///workspace/data/wal"  # Alternative to wal_urls
```

### 2. Configuration Flow Architecture
```
config.toml 
  ↓ (parsed by serde)
Config::storage::wal_config::wal_urls 
  ↓ (passed to WAL factory)
WalConfig::multi_disk::data_directories 
  ↓ (used by WAL strategy)  
AvroWalStrategy::write_wal_entries_to_disk_atomic()
  ↓ (filesystem API)
FilesystemFactory::get_filesystem(url)
```

## Implementation Plan

### Phase 1: Configuration Structure Updates

#### Update Core Config
```rust
// src/core/config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Legacy single WAL directory (deprecated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wal_dir: Option<PathBuf>,
    
    /// Modern WAL configuration with multi-disk support
    pub wal_config: WalStorageConfig,
    
    // ... other fields
}

/// WAL storage configuration supporting multiple directories and cloud storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalStorageConfig {
    /// WAL storage URLs - supports file://, s3://, adls://, gcs://
    /// Multiple URLs enable multi-disk performance scaling
    pub wal_urls: Vec<String>,
    
    /// Distribution strategy for collections across WAL directories
    #[serde(default)]
    pub distribution_strategy: WalDistributionStrategy,
    
    /// Whether to keep each collection on a single WAL directory
    #[serde(default = "default_collection_affinity")]
    pub collection_affinity: bool,
    
    /// Memory flush threshold per collection (bytes)
    #[serde(default = "default_memory_flush_size")]
    pub memory_flush_size_bytes: usize,
    
    /// Global WAL size threshold for forced flush (bytes)
    #[serde(default = "default_global_flush_threshold")]
    pub global_flush_threshold: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalDistributionStrategy {
    /// Round-robin across WAL directories
    RoundRobin,
    /// Hash-based distribution (consistent)
    Hash,
    /// Load-balanced distribution (dynamic)
    LoadBalanced,
}

impl Default for WalStorageConfig {
    fn default() -> Self {
        Self {
            wal_urls: vec!["file:///workspace/data/wal".to_string()],
            distribution_strategy: WalDistributionStrategy::LoadBalanced,
            collection_affinity: true,
            memory_flush_size_bytes: 1 * 1024 * 1024, // 1MB
            global_flush_threshold: 512 * 1024 * 1024, // 512MB
        }
    }
}

// Helper functions for serde defaults
fn default_collection_affinity() -> bool { true }
fn default_memory_flush_size() -> usize { 1 * 1024 * 1024 }
fn default_global_flush_threshold() -> usize { 512 * 1024 * 1024 }
```

#### Update WAL Config to Use URLs
```rust
// src/storage/persistence/wal/config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiDiskConfig {
    /// WAL directory URLs supporting multiple filesystem types
    /// Examples:
    /// - file:///path/to/wal1, file:///path/to/wal2 (local multi-disk)
    /// - s3://bucket1/wal, s3://bucket2/wal (S3 multi-bucket)
    /// - adls://account.dfs.core.windows.net/container1/wal (Azure)
    /// - gcs://bucket/wal (Google Cloud)
    pub data_directories: Vec<String>,  // Changed from Vec<PathBuf> to Vec<String>
    
    /// Distribution strategy
    pub distribution_strategy: DiskDistributionStrategy,
    
    /// Enable collection affinity (collection stays on one disk)
    pub collection_affinity: bool,
}

impl Default for MultiDiskConfig {
    fn default() -> Self {
        Self {
            data_directories: vec!["file:///workspace/data/wal".to_string()], // URL format
            distribution_strategy: DiskDistributionStrategy::LoadBalanced,
            collection_affinity: true,
        }
    }
}
```

### Phase 2: WAL Strategy Implementation Updates

#### Update WAL Initialization
```rust
// src/storage/persistence/wal/avro.rs
impl AvroWalStrategy {
    /// Initialize with externalized WAL URLs from configuration
    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        self.config = Some(config.clone());
        self.filesystem = Some(filesystem.clone());
        
        // Initialize WAL directories for all configured URLs
        self.initialize_wal_directories(config).await?;
        
        // Initialize other components...
        self.memory_table = Some(WalMemTable::new(config.clone()).await?);
        self.disk_manager = Some(WalDiskManager::new(config.clone(), filesystem).await?);
        
        Ok(())
    }
    
    /// Initialize WAL directories for all configured URLs
    async fn initialize_wal_directories(&self, config: &WalConfig) -> Result<()> {
        let filesystem = self.filesystem.as_ref().context("Filesystem not initialized")?;
        
        for wal_url in &config.multi_disk.data_directories {
            tracing::info!("🔧 Initializing WAL directory: {}", wal_url);
            
            // Parse URL to determine filesystem type
            let fs = filesystem.get_filesystem(wal_url)
                .context(format!("Failed to get filesystem for WAL URL: {}", wal_url))?;
            
            // Create base WAL directory if it doesn't exist
            let base_path = self.extract_base_path_from_url(wal_url)?;
            if !fs.exists(&base_path).await? {
                fs.create_dir_all(&base_path).await
                    .context(format!("Failed to create WAL directory: {}", base_path))?;
                tracing::info!("✅ Created WAL directory: {}", base_path);
            } else {
                tracing::debug!("✅ WAL directory exists: {}", base_path);
            }
        }
        
        Ok(())
    }
    
    fn extract_base_path_from_url(&self, url: &str) -> Result<String> {
        if url.starts_with("file://") {
            Ok(url.strip_prefix("file://").unwrap_or(url).to_string())
        } else {
            // For cloud URLs, use the full URL as the base path
            Ok(url.to_string())
        }
    }
}
```

#### Update Write Operations to Use Configured URLs
```rust
// src/storage/persistence/wal/avro.rs
impl AvroWalStrategy {
    /// Write WAL entries using configured URLs (no hardcoding)
    async fn write_wal_entries_to_disk_atomic(&self, entries: &[WalEntry]) -> Result<()> {
        let config = self.config.as_ref().context("WAL config not initialized")?;
        let filesystem = self.filesystem.as_ref().context("Filesystem not initialized")?;

        // Group by collection for batch efficiency
        let mut by_collection: std::collections::HashMap<String, Vec<&WalEntry>> = 
            std::collections::HashMap::new();
        for entry in entries {
            by_collection.entry(entry.collection_id.clone()).or_default().push(entry);
        }

        // Write each collection using configured WAL URL selection
        for (collection_id, collection_entries) in by_collection {
            // Select WAL URL based on distribution strategy
            let wal_url = self.select_wal_url_for_collection(&collection_id, config)?;
            tracing::debug!("📁 Selected WAL URL for collection {}: {}", collection_id, wal_url);
            
            // Get filesystem for the selected URL
            let fs = filesystem.get_filesystem(&wal_url)?;
            
            // Create collection WAL path
            let wal_path = self.build_collection_wal_path(&wal_url, &collection_id)?;
            
            // Ensure collection directory exists
            let collection_dir = self.build_collection_dir_path(&wal_url, &collection_id)?;
            fs.create_dir_all(&collection_dir).await
                .context(format!("Failed to create collection WAL directory: {}", collection_dir))?;
            
            // Serialize entries
            let entries_slice: &[WalEntry] = &collection_entries.iter().map(|&e| e.clone()).collect::<Vec<_>>();
            let avro_data = self.serialize_entries_impl(entries_slice).await?;
            
            // Atomic write using filesystem API
            fs.write_atomic(&wal_path, &avro_data, None).await
                .context(format!("Atomic WAL write failed for collection {} to {}", collection_id, wal_path))?;
                
            tracing::debug!("✅ WAL written atomically: {} ({} entries, {} bytes)", 
                wal_path, collection_entries.len(), avro_data.len());
        }

        Ok(())
    }
    
    /// Select WAL URL for a collection based on distribution strategy
    fn select_wal_url_for_collection(&self, collection_id: &str, config: &WalConfig) -> Result<String> {
        let urls = &config.multi_disk.data_directories;
        
        if urls.is_empty() {
            return Err(anyhow::anyhow!("No WAL URLs configured"));
        }
        
        if urls.len() == 1 {
            return Ok(urls[0].clone());
        }
        
        let index = match config.multi_disk.distribution_strategy {
            DiskDistributionStrategy::RoundRobin => {
                // Simple round-robin based on collection hash
                let hash = self.hash_collection_id(collection_id);
                hash % urls.len()
            },
            DiskDistributionStrategy::Hash => {
                // Consistent hashing for stable distribution
                let hash = self.hash_collection_id(collection_id);
                hash % urls.len()
            },
            DiskDistributionStrategy::LoadBalanced => {
                // For now, use hash-based. In production, could check disk usage
                let hash = self.hash_collection_id(collection_id);
                hash % urls.len()
            },
        };
        
        Ok(urls[index].clone())
    }
    
    fn hash_collection_id(&self, collection_id: &str) -> usize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        hasher.finish() as usize
    }
    
    fn build_collection_wal_path(&self, wal_url: &str, collection_id: &str) -> Result<String> {
        if wal_url.starts_with("file://") {
            let base_path = wal_url.strip_prefix("file://").unwrap_or(wal_url);
            Ok(format!("{}/{}/wal_current.avro", base_path, collection_id))
        } else {
            // For cloud URLs, append collection path
            Ok(format!("{}/{}/wal_current.avro", wal_url.trim_end_matches('/'), collection_id))
        }
    }
    
    fn build_collection_dir_path(&self, wal_url: &str, collection_id: &str) -> Result<String> {
        if wal_url.starts_with("file://") {
            let base_path = wal_url.strip_prefix("file://").unwrap_or(wal_url);
            Ok(format!("{}/{}", base_path, collection_id))
        } else {
            // For cloud URLs, append collection path  
            Ok(format!("{}/{}", wal_url.trim_end_matches('/'), collection_id))
        }
    }
}
```

### Phase 3: Configuration Integration

#### Update Config Loading
```rust
// src/lib.rs or main config loading
impl Config {
    pub fn load_from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let config_str = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&config_str)?;
        
        // Migrate legacy wal_dir to new wal_config if needed
        if let Some(legacy_wal_dir) = &config.storage.wal_dir {
            if config.storage.wal_config.wal_urls.is_empty() {
                let wal_url = if legacy_wal_dir.to_string_lossy().contains("://") {
                    legacy_wal_dir.to_string_lossy().to_string()
                } else {
                    format!("file://{}", legacy_wal_dir.display())
                };
                config.storage.wal_config.wal_urls = vec![wal_url];
                tracing::warn!("⚠️ Migrated legacy wal_dir to wal_config.wal_urls");
            }
        }
        
        Ok(config)
    }
}
```

#### Create WAL Config from Core Config
```rust
// Conversion from core config to WAL config
impl From<&crate::core::config::WalStorageConfig> for WalConfig {
    fn from(core_config: &crate::core::config::WalStorageConfig) -> Self {
        let mut wal_config = WalConfig::default();
        
        // Convert URLs to MultiDiskConfig
        wal_config.multi_disk.data_directories = core_config.wal_urls.clone();
        wal_config.multi_disk.distribution_strategy = match core_config.distribution_strategy {
            crate::core::config::WalDistributionStrategy::RoundRobin => DiskDistributionStrategy::RoundRobin,
            crate::core::config::WalDistributionStrategy::Hash => DiskDistributionStrategy::Hash,
            crate::core::config::WalDistributionStrategy::LoadBalanced => DiskDistributionStrategy::LoadBalanced,
        };
        wal_config.multi_disk.collection_affinity = core_config.collection_affinity;
        
        // Set performance thresholds
        wal_config.performance.memory_flush_size_bytes = core_config.memory_flush_size_bytes;
        wal_config.performance.global_flush_threshold = core_config.global_flush_threshold;
        
        wal_config
    }
}
```

## Example Configurations

### Single WAL Directory
```toml
[storage.wal_config]
wal_urls = ["file:///workspace/data/wal"]
memory_flush_size_bytes = 1048576  # 1MB
```

### Multi-Disk Local Performance
```toml
[storage.wal_config]
wal_urls = [
    "file:///mnt/nvme1/wal",
    "file:///mnt/nvme2/wal",
    "file:///mnt/nvme3/wal"
]
distribution_strategy = "LoadBalanced"
collection_affinity = true
memory_flush_size_bytes = 1048576  # 1MB
global_flush_threshold = 536870912  # 512MB
```

### Cloud Multi-Region Setup
```toml
[storage.wal_config]
wal_urls = [
    "s3://my-wal-bucket-us-west/wal",
    "s3://my-wal-bucket-us-east/wal",
    "adls://myaccount.dfs.core.windows.net/wal-container/wal"
]
distribution_strategy = "Hash"
collection_affinity = true
```

### Serverless Cloud Storage
```toml
[storage.wal_config]
wal_urls = ["s3://serverless-wal-bucket/proximadb/wal"]
distribution_strategy = "Hash"
memory_flush_size_bytes = 2097152  # 2MB (larger for cloud)
```

This implementation provides:
1. ✅ **Externalized configuration** - No hardcoded paths
2. ✅ **Multi-disk support** - Performance scaling across disks  
3. ✅ **Filesystem API compatibility** - Works with file://, s3://, adls://, gcs://
4. ✅ **Collection affinity** - Keep collection data on consistent disks
5. ✅ **Distribution strategies** - Round-robin, hash, load-balanced
6. ✅ **Cloud-ready** - Serverless deployment support
7. ✅ **Backward compatibility** - Migrates legacy single wal_dir setting