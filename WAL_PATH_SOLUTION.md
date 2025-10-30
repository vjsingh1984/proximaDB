# WAL Path Issue - Root Cause & Solution Design

## Current Situation (53 commits)

**WAL files created but wrong location:**
- Actual: `/Users/.../data/write_buffer/{collection}/wal/*.bcwal`
- Expected: `/tmp/proximadb/d{N}/{collection}/wal/*.bcwal`

## Root Cause Analysis

**The fallback path at line 1755/1930 uses:**
```rust
self.config.multi_disk.data_directories.get(0)
```

**But actual path used is:**
```
/Users/vijay.singh/code/proximaDB/data/write_buffer
```

**This is NOT from multi_disk.data_directories!** It's from somewhere else entirely.

## Critical Discovery

Check line 1780/1935 - WriteAheadLogDiskManager creation:
```rust
let disk_manager = WriteAheadLogDiskManager::new(filesystem_factory, &base_location);
```

The base_location passed here determines where files go. Our metadata query provides this, but if it fails, we have fallbacks.

**But wait** - the actual path suggests it's using `config.write_buffer_directory` NOT `multi_disk.data_directories`!

Look at config:
```
write_buffer_directory: "/Users/vijay.singh/code/proximaDB/data/write_buffer"
```

**This is the smoking gun!**

## Actual Problem

There's likely a DIFFERENT code path or default that uses:
```rust
config.write_buffer_directory  // Wrong!
```

Instead of:
```rust
collection.storage_assignment.base_location  // Right!
```

## Solution Design

**Option 1: Fix Default Fallback**

Change line 1758/1940 from:
```rust
self.config.multi_disk.data_directories.get(0)
```

To explicitly use storage locations from config, not write_buffer_directory.

**Option 2: Remove write_buffer_directory from Config**

The `write_buffer_directory` in config should NOT be used. Delete it or ignore it.
Use only `storage_locations` (d1, d2, d3).

**Option 3: Always Query Collection**

Make get_collection() call required, not optional:
```rust
let collection = provider.get_collection(collection_id).await?;
let assignment = collection.storage_assignment
    .ok_or_else(|| anyhow!("No storage assignment"))?;
let base_location = assignment.base_location;
// No fallbacks - fail loudly if not found
```

## Recommended Solution

**Hybrid:**
1. Query collection for storage_assignment
2. If fails, use `config.storage_locations[hash(collection_id) % 3]`
3. Never use `config.write_buffer_directory`

## Implementation

```rust
// At line ~1740
let base_location = {
    // Try to get from collection metadata
    if let Some(provider_lock) = self.metadata_provider.read().await.as_ref() {
        if let Ok(Some(collection)) = provider_lock.get_collection(collection_id).await {
            if let Some(assignment) = collection.storage_assignment {
                // SUCCESS - use actual assignment
                eprintln!("✅ Using storage assignment: {}", assignment.base_location);
                assignment.base_location.clone()
            } else {
                // Collection exists but no assignment - use hash distribution
                eprintln!("⚠️ Collection has no assignment, using hash distribution");
                let hash = collection_id.bytes().sum::<u8>() as usize;
                let idx = hash % self.config.multi_disk.data_directories.len();
                self.config.multi_disk.data_directories[idx].clone()
            }
        } else {
            // Collection not found - use hash distribution
            eprintln!("⚠️ Collection not found, using hash distribution");
            let hash = collection_id.bytes().sum::<u8>() as usize;
            let idx = hash % self.config.multi_disk.data_directories.len();
            self.config.multi_disk.data_directories[idx].clone()
        }
    } else {
        // No provider - use hash distribution
        eprintln!("⚠️ No metadata provider, using hash distribution");
        let hash = collection_id.bytes().sum::<u8>() as usize;
        let idx = hash % self.config.multi_disk.data_directories.len();
        self.config.multi_disk.data_directories[idx].clone()
    }
};
```

## Why This Will Work

- Always uses config.storage_locations (d1, d2, d3)
- Never uses config.write_buffer_directory
- Deterministic hash distribution as fallback
- Collection metadata as primary source

## Next Session Steps

1. Implement above code at lines ~1740 and ~1925
2. Remove all references to write_buffer_directory for path
3. Rebuild
4. Test - should see files in /tmp/proximadb/d{N}/
5. Recovery should achieve 100%

Estimated time: 20 minutes
