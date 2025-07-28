# Collection Persistence Fix Documentation

## Issue Summary
Collections were not persisting after server restart. The metadata was being saved to the filestore backend but not properly loaded during startup.

## Root Cause Analysis

The recovery process has three critical steps:
1. **Metadata Recovery**: Filestore backend recovers collection metadata from snapshots/oplogs
2. **Assignment Service Population**: Storage assignments must be recreated for each collection
3. **WAL Recovery**: WAL needs assignments to know where to flush data

The issue was in step 2 - the `load_collections()` method in `StorageEngine` was scanning filesystem directories instead of loading from the metadata store. This meant:
- Collections were not found after restart
- Assignment service was empty
- WAL couldn't recover properly without assignments

## Solution Implemented

### 1. Modified `load_collections()` in `src/storage/engine.rs`

**Before**: Scanned storage location directories
```rust
// Scanning directories - wrong approach
for location in &self.config.storage_locations {
    let data_dir = PathBuf::from(path);
    let mut entries = tokio::fs::read_dir(&data_dir).await?;
    // ... scan for collection directories
}
```

**After**: Load from metadata provider
```rust
// Load from metadata provider which has recovered collections
let collections = if let Some(provider) = self.get_metadata_provider().await {
    match provider.list_collections().await {
        Ok(collections) => collections,
        Err(e) => return Err(...),
    }
} else {
    return Ok(());
};
```

### 2. Added Assignment Service Population

For each recovered collection, we now:
1. Check if assignment already exists
2. If not, create a new assignment using the assignment service
3. This ensures WAL and storage engines know where data should be stored

```rust
for collection in &collections {
    let collection_id = &collection.id;
    let collection_name = collection.config.as_ref()
        .map(|c| c.name.as_str())
        .unwrap_or(collection_id);
    
    if assignment_service.get_assignment(collection_id).await.is_none() {
        let assignment = assignment_service
            .assign_collection(
                collection_name,
                &self.config.storage_locations,
                &self.config.assignment_config.strategy,
            )
            .await?;
        
        tracing::info!("✅ Assignment created for collection {} at {}", 
            collection_name, assignment.location_url);
    }
}
```

### 3. Fixed MMAP Reader Initialization

Updated to use assignment service for data directories instead of guessing:
```rust
let data_dir = if let Some(assignment) = assignment_service.get_assignment(&collection_id).await {
    if assignment.data_url.starts_with("file://") {
        PathBuf::from(assignment.data_url.strip_prefix("file://").unwrap())
    } else {
        PathBuf::from(&assignment.data_url)
    }
} else {
    continue; // Skip if no assignment
};
```

## Architecture Clarification

As correctly pointed out, the proper architecture is:
1. **CollectionService** owns the metadata backend (Filestore)
2. **Filestore** recovers metadata from snapshots/oplogs on startup
3. **StorageEngine** should query CollectionService/metadata provider for collections
4. **Assignment Service** must be populated from recovered metadata
5. **WAL** uses assignments to determine flush locations

## Testing

The fix ensures:
- Collections created before restart appear after restart
- Assignment service is properly populated
- WAL can recover and flush to correct locations
- Both LSM and VIPER engines can access their data

## Related Issues

### Redundant WAL Directory
The configuration has separate WAL directory (`lsm_wal`) which is redundant since:
- LSM engines manage their own WAL within data directories
- This should be removed in a future cleanup

## Conclusion

The fix properly implements the three-phase recovery process:
1. ✅ Metadata recovery (already working via Filestore)
2. ✅ Assignment service population (fixed)
3. ✅ WAL recovery with proper assignments (now works)

This ensures collections persist across restarts and the system can properly recover all data.