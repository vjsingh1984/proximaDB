# Filesystem Migration Implementation Guide for Claude Code

## 🎯 Purpose
This document provides step-by-step instructions for Claude Code to implement the UnifiedCachingFilesystem migration across ProximaDB storage engines.

---

## 🚀 Quick Start for New Session

### 1. Check Current Status
```bash
# Check migration tracker for current phase/task
cat FILESYSTEM_MIGRATION_TRACKER.md | grep -A 5 "Current Phase"

# Find TODO tasks
grep "🔴 TODO" FILESYSTEM_MIGRATION_TRACKER.md | head -5
```

### 2. Verify Prerequisites
```bash
# Ensure code compiles before starting
cargo build --all-targets 2>&1 | grep -c "error\["
# Should output: 0

# Check for existing zero_copy usage (Phase 1)
grep -r "ZeroCopyIOSystem" src/storage/engines/impls/sst/ | wc -l
# Current count: 5 (needs to be 0)
```

### 3. Pick Up Next Task
Look in `FILESYSTEM_MIGRATION_TRACKER.md` for the lowest numbered 🔴 TODO task without dependencies.

---

## 📚 Phase 1: SST Engine Cleanup (DETAILED)

### Context
SST engine currently uses both `ZeroCopyIOSystem` (legacy) and `UnifiedCachingFilesystem` (new). We need to complete the migration to establish a pattern for other engines.

### Task 1.1.1: Remove ZeroCopyIOSystem Imports

**Step 1**: Open files and remove imports
```rust
// File: src/storage/engines/impls/sst/streaming_compaction.rs
// REMOVE Line 28:
use crate::storage::engines::core::io::zero_copy::ZeroCopyIOSystem;

// File: src/storage/engines/impls/sst/compaction.rs
// REMOVE Lines 28-29:
use crate::storage::engines::core::io::zero_copy::{ZeroCopyIOConfig, ZeroCopyIOSystem};
```

**Step 2**: Add new import
```rust
// ADD to both files:
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
```

**Step 3**: Verify compilation
```bash
cargo check --lib 2>&1 | grep -E "error\[E"
```

### Task 1.1.2: Replace ZeroCopyIOSystem Usage

**Location 1**: `src/storage/engines/impls/sst/compaction.rs:194`

**Current Code**:
```rust
let zero_copy_config = ZeroCopyIOConfig {
    cache_size_mb: 512,
    prefetch_size_kb: 256,
    // ...
};

let zero_copy_system = Arc::new(
    ZeroCopyIOSystem::new(zero_copy_config, filesystem_factory.clone(), Vec::new())
        .await?
);
```

**Replace With**:
```rust
// Get base filesystem
let base_fs = filesystem_factory.get_filesystem("file://")?;

// Create unified filesystem with SST-specific serializer
let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
    base_fs,
    collection_id.clone(),
    "sst".to_string(),
    Arc::new(SstUnifiedMetadataSerializer::new()),
));

// Note: Cache configuration is now handled by UnifiedCachingFilesystem
```

**Location 2**: `src/storage/engines/impls/sst/streaming_compaction.rs:217`

**Current Code**:
```rust
let zero_copy_io = crate::storage::engines::core::io::zero_copy::ZeroCopyIOSystem::new(
    zero_copy_config,
    self.filesystem.clone(),
    Vec::new(),
).await?;
```

**Replace With**:
```rust
// Use the unified filesystem from engine
let unified_fs = self.filesystem.clone(); // Assuming filesystem is now UnifiedCachingFilesystem
```

### Task 1.1.3: Remove SstDirectReader

**Step 1**: Find all usages
```bash
grep -r "SstDirectReader" src/storage/engines/impls/sst/
```

**Step 2**: For each usage, replace with unified approach

**Example Replacement**:
```rust
// OLD:
let reader = SstDirectReader::open(filesystem.clone(), &file_url).await?;

// NEW:
let reader = SstUnifiedReader::new(unified_fs.clone(), &file_url).await?;
```

**Step 3**: Delete the file
```bash
rm src/storage/engines/impls/sst/readers/sst_direct_reader.rs
```

**Step 4**: Remove from mod.rs
```rust
// In src/storage/engines/impls/sst/readers/mod.rs
// REMOVE:
pub mod sst_direct_reader;
pub use sst_direct_reader::SstDirectReader;
```

### Task 1.1.4: Update SstQueryEngine

**File**: `src/storage/engines/impls/sst/readers/sst_query_engine.rs`

**Step 1**: Remove field from struct (around line 115)
```rust
// OLD:
pub struct SstQueryEngine {
    zero_copy_system: Arc<ZeroCopyIOSystem>,
    filesystem: Arc<FilesystemFactory>,
    // ...
}

// NEW:
pub struct SstQueryEngine {
    filesystem: Arc<UnifiedCachingFilesystem>,  // Changed type
    // Remove zero_copy_system field
    // ...
}
```

**Step 2**: Update constructor
```rust
// OLD:
impl SstQueryEngine {
    pub fn new(
        zero_copy_system: Arc<ZeroCopyIOSystem>,
        filesystem: Arc<FilesystemFactory>,
        // ...
    ) -> Self {
        Self {
            zero_copy_system,
            filesystem,
            // ...
        }
    }
}

// NEW:
impl SstQueryEngine {
    pub fn new(
        filesystem: Arc<UnifiedCachingFilesystem>,
        // ...
    ) -> Self {
        Self {
            filesystem,
            // ...
        }
    }
}
```

**Step 3**: Update all methods using zero_copy_system
```rust
// Find all occurrences:
grep "self.zero_copy_system" src/storage/engines/impls/sst/readers/sst_query_engine.rs

// Replace with:
self.filesystem  // UnifiedCachingFilesystem has equivalent methods
```

### Task 1.1.5: Unify Filesystem Fields in Engine

**File**: `src/storage/engines/impls/sst/engine.rs`

**Current Structure**:
```rust
pub struct SstEngine {
    filesystem: Arc<FilesystemFactory>,
    unified_fs: Option<Arc<dyn FileSystem>>,  // Remove this
    // ...
}
```

**New Structure**:
```rust
pub struct SstEngine {
    filesystem: Arc<UnifiedCachingFilesystem>,  // Primary filesystem
    filesystem_factory: Arc<FilesystemFactory>,  // Keep for compatibility
    // ...
}
```

**Update Constructor**:
```rust
impl SstEngine {
    pub async fn new(
        config: SstConfig,
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
    ) -> Result<Self> {
        // Create base filesystem
        let base_fs = filesystem_factory.get_filesystem(&config.storage_url)?;

        // Wrap with unified caching filesystem
        let filesystem = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs,
            collection_id.clone(),
            "sst".to_string(),
            Arc::new(SstUnifiedMetadataSerializer::new()),
        ));

        Ok(Self {
            filesystem,
            filesystem_factory,
            collection_id,
            // ...
        })
    }
}
```

### Task 1.1.9: Create SST Metadata Serializer

**Create New File**: `src/storage/engines/impls/sst/unified_metadata_serializer.rs`

```rust
//! SST-specific metadata serializer for UnifiedCachingFilesystem

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::storage::persistence::filesystem::unified::UnifiedMetadataSerializer;

/// SST-specific metadata that needs caching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstMetadataEntry {
    /// Block-level bloom filters
    pub bloom_filters: HashMap<u32, Vec<u8>>,
    /// Block index entries
    pub index_entries: Vec<IndexEntry>,
    /// Compression dictionaries
    pub compression_dicts: HashMap<u32, Vec<u8>>,
    /// Block statistics
    pub block_stats: BlockStatistics,
}

/// SST metadata serializer for unified caching
pub struct SstUnifiedMetadataSerializer {
    collection_id: String,
}

impl SstUnifiedMetadataSerializer {
    pub fn new() -> Self {
        Self {
            collection_id: String::new(),
        }
    }

    pub fn with_collection(collection_id: String) -> Self {
        Self { collection_id }
    }
}

impl UnifiedMetadataSerializer for SstUnifiedMetadataSerializer {
    fn serialize_metadata(&self, metadata: &[u8]) -> Result<Vec<u8>> {
        // SST uses bincode for metadata serialization
        // This is compatible with FastLanesDataBlock serialization
        Ok(metadata.to_vec())
    }

    fn deserialize_metadata(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Direct passthrough for SST metadata
        Ok(data.to_vec())
    }

    fn metadata_key(&self, key: &str) -> String {
        // SST-specific key formatting
        // Includes collection_id for isolation
        format!("sst_{}_{}", self.collection_id, key)
    }

    fn cache_key_prefix(&self) -> String {
        format!("sst_{}", self.collection_id)
    }

    fn should_cache(&self, key: &str) -> bool {
        // Cache these SST metadata types
        key.contains("bloom") ||
        key.contains("index") ||
        key.contains("header") ||
        key.contains("footer")
    }

    fn cache_ttl_seconds(&self, key: &str) -> u64 {
        if key.contains("bloom") {
            3600 // Bloom filters: 1 hour
        } else if key.contains("index") {
            7200 // Indices: 2 hours
        } else {
            1800 // Default: 30 minutes
        }
    }
}
```

**Add to mod.rs**:
```rust
// In src/storage/engines/impls/sst/mod.rs
pub mod unified_metadata_serializer;
pub use unified_metadata_serializer::SstUnifiedMetadataSerializer;
```

---

## 📚 Phase 2: FastLanes Group Migration

### Task 2.1.1: SWIFT Engine Migration

**File**: `src/storage/engines/impls/swift/engine.rs`

**Current**:
```rust
pub struct SwiftEngine {
    filesystem: Arc<FilesystemFactory>,
    // ...
}
```

**Target**:
```rust
pub struct SwiftEngine {
    filesystem: Arc<UnifiedCachingFilesystem>,
    filesystem_factory: Arc<FilesystemFactory>,
    // ...
}

impl SwiftEngine {
    pub async fn new(
        config: SwiftConfig,
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
    ) -> Result<Self> {
        let base_fs = filesystem_factory.get_filesystem(&config.storage_url)?;

        let filesystem = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs,
            collection_id.clone(),
            "swift".to_string(),
            Arc::new(SwiftUnifiedMetadataSerializer::new()),
        ));

        Ok(Self {
            filesystem,
            filesystem_factory,
            collection_id,
            // ...
        })
    }
}
```

### Task 2.2.1: HELIX Engine Migration

**Similar pattern to SWIFT - follow the same template**

---

## 📚 Phase 3: Parquet Group Optimization

### Task 3.1.1: NOVA Engine Migration

**File**: `src/storage/engines/impls/nova/engine.rs`

**Key Difference**: NOVA needs Parquet-specific caching

```rust
// In nova/unified_metadata_serializer.rs
pub struct NovaUnifiedMetadataSerializer {
    // Cache Parquet-specific metadata
}

impl UnifiedMetadataSerializer for NovaUnifiedMetadataSerializer {
    fn should_cache(&self, key: &str) -> bool {
        // Cache Parquet footers and row group metadata
        key.contains("parquet_footer") ||
        key.contains("row_group") ||
        key.contains("column_chunk") ||
        key.contains("schema")
    }
}
```

---

## 🧪 Testing Each Phase

### After Each Task
```bash
# Compile check
cargo build --lib 2>&1 | grep -c "error\["

# Run engine-specific tests
cargo test --test integration <engine_name>::

# Check for legacy code
grep -r "ZeroCopyIOSystem" src/storage/engines/impls/<engine_name>/
```

### After Each Phase
```bash
# Full test suite
cargo test --all-targets

# Benchmark comparison
cargo bench --bench engine_comparison_bench

# Memory profiling
valgrind --tool=massif cargo run --bin proximadb-server
```

---

## 🔍 Common Issues and Solutions

### Issue 1: Borrow Checker Errors
**Symptom**: "cannot borrow `filesystem` as mutable"
**Solution**: Use `Arc::clone()` instead of borrowing

### Issue 2: Missing Trait Implementation
**Symptom**: "the trait `FileSystem` is not implemented"
**Solution**: UnifiedCachingFilesystem implements FileSystem, check imports

### Issue 3: Async/Await Issues
**Symptom**: "future cannot be sent between threads safely"
**Solution**: Ensure all fields in structs are Send + Sync

### Issue 4: Cache Configuration
**Symptom**: "cache size exceeds memory limit"
**Solution**: Adjust cache sizes in FilesystemConfig

---

## 📏 Progress Validation

### Per-Task Checklist
- [ ] Code compiles without errors
- [ ] No new warnings introduced
- [ ] Tests pass for modified engine
- [ ] No references to old API remain
- [ ] Performance metrics collected

### Per-Phase Checklist
- [ ] All tasks marked ✅ DONE
- [ ] Integration tests pass
- [ ] Benchmarks show improvement
- [ ] Documentation updated
- [ ] Code review completed

---

## 🎬 Handoff Protocol

### Before Ending Session
1. Update task status in `FILESYSTEM_MIGRATION_TRACKER.md`
2. Commit changes with descriptive message
3. Add notes about any blockers or decisions
4. Run `cargo build` to ensure clean state

### Starting New Session
1. Read this guide's Quick Start section
2. Check tracker for current status
3. Pull latest changes
4. Verify clean compilation
5. Continue from next TODO task

---

## 📞 Getting Help

### Resources
- `FILESYSTEM_STANDARDIZATION_PLAN.md` - Overall strategy
- `SHARED_COMPONENTS_ANALYSIS.md` - Component relationships
- `src/storage/persistence/filesystem/unified.rs` - UnifiedCachingFilesystem API
- `src/storage/engines/impls/viper/` - Reference implementation

### Debug Commands
```bash
# Find all filesystem usage
grep -r "filesystem\." src/storage/engines/impls/<engine>/

# Check cache statistics
RUST_LOG=proximadb::storage::persistence::filesystem=debug cargo run

# Trace filesystem calls
strace -e openat,read,write cargo test <specific_test>
```