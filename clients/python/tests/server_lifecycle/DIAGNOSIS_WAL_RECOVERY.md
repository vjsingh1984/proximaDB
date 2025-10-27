# WAL Recovery Bug Diagnosis

## Investigation Results

### ✅ CONFIRMED: WAL Files ARE Written to Disk

```bash
$ find /tmp/proximadb -name "*.bcwal" -ls | head -10
... 371830 /tmp/proximadb/d1/1vDUYDT/wal/8WkkFljRlA.bcwal
...   1200 /tmp/proximadb/d1/1vDVz5m/wal/8WkqbvS2vQ.bcwal
... 220984 /tmp/proximadb/d1/1vDVvBW/wal/8WkqKhsgim.bcwal
```

**Conclusion:** Write path is working correctly. Files contain data (varying sizes).

---

### ✅ CONFIRMED: Storage Assignment IS Created

**File:** `src/services/collection/manager.rs:559`

```rust
storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
    primary_path: base_location.clone(),
    backup_paths: vec![],
    engine: config.storage_engine.unwrap_or(StorageEngine::Sst as i32),
    ...
}),
```

**Conclusion:** Server creates storage_assignment correctly.

---

### ✅ CONFIRMED: WAL Recovery Code Exists

**File:** `src/storage/engine.rs:280`

Recovery sequence:
1. Get recovery manager
2. List all collections
3. For each collection: recover_collection()
4. Report total vectors recovered

---

### ⚠️ PROBLEM: Multiple Silent Failure Points

#### Failure Point 1: No Recovery Manager (line 286)
```rust
None => {
    warn!("No recovery manager available, skipping WAL recovery");
    return Ok(());  // SILENT FAILURE!
}
```

#### Failure Point 2: No Metadata Provider (line 297)
```rust
None => {
    warn!("No metadata provider set, cannot recover collections");
    return Ok(());  // SILENT FAILURE!
}
```

#### Failure Point 3: Per-Collection Errors (line 331)
```rust
Err(e) => {
    warn!("Failed to recover collection {}: {}", collection.id, e);
    // Continue with other collections even if one fails
    // SILENT FAILURE for that collection!
}
```

#### Failure Point 4: Startup Error Swallowing (lib.rs:353)
```rust
Err(e) => {
    tracing::warn!(
        "WAL recovery failed (continuing anyway): {}", e
    );
    // Don't fail startup if WAL recovery fails
    // SERVER STARTS WITH NO DATA!
}
```

---

## Root Cause Hypothesis

**Most Likely:** One of these is happening:

1. **Recovery manager is None**
   - Why? WAL manager not initialized properly
   - Result: Silent skip with warning

2. **Metadata provider is None**
   - Why? Not set during storage engine initialization
   - Result: Silent skip with warning

3. **Collections list is empty**
   - Why? Metadata recovered but list_collections returns []
   - Result: Loop never runs, 0 vectors recovered

4. **Per-collection recovery fails**
   - Why? Storage path invalid, WAL files not found, etc.
   - Result: Warning logged but data lost

---

## Diagnostic Steps

### Step 1: Check Server Startup Logs

Need to capture server output from test to see:

```
Expected logs:
INFO 🔄 STORAGE_ENGINE: Starting WAL recovery for all collections...
INFO 📋 STORAGE_ENGINE: Found X collections to recover
INFO 🔍 STORAGE_ENGINE: Recovering collection: {id}
INFO ✅ STORAGE_ENGINE: Collection {id} recovered: Y vectors from Z files
INFO 🎉 STORAGE_ENGINE: WAL recovery complete: N total vectors recovered
```

**Action:** Modify test to capture and display server logs.

### Step 2: Add Debug Logging

Temporarily add more logging to identify which failure point is hit:

```rust
// In src/storage/engine.rs:280
pub async fn recover_from_wal(&self) -> Result<()> {
    eprintln!("DEBUG: recover_from_wal called");

    let recovery_manager = match self.write_ahead_log_manager.recovery_manager() {
        Some(manager) => {
            eprintln!("DEBUG: Recovery manager EXISTS");
            manager
        }
        None => {
            eprintln!("DEBUG: Recovery manager is NONE - THIS IS THE BUG!");
            warn!("No recovery manager available");
            return Ok(());
        }
    };

    let metadata_provider = self.metadata_provider.read().await;
    let provider = match metadata_provider.as_ref() {
        Some(p) => {
            eprintln!("DEBUG: Metadata provider EXISTS");
            p
        }
        None => {
            eprintln!("DEBUG: Metadata provider is NONE - THIS IS THE BUG!");
            warn!("No metadata provider set");
            return Ok(());
        }
    };

    let collections = provider.list_collections().await?;
    eprintln!("DEBUG: Found {} collections", collections.len());

    // ... rest of function
}
```

### Step 3: Check Recovery Manager Initialization

**File:** `src/storage/persistence/write_ahead_log/mod.rs:1248`

Check if `recovery_manager()` method returns `Some` or `None`:

```rust
pub fn recovery_manager(&self) -> Option<Arc<RecoveryManager>> {
    self.recovery_manager.clone()  // Is this None?
}
```

**Question:** Is recovery_manager actually initialized in WAL manager constructor?

### Step 4: Check Metadata Provider

**File:** `src/storage/engine.rs`

Check if `metadata_provider` is set during initialization:

```rust
pub struct StorageEngine {
    metadata_provider: Arc<RwLock<Option<Arc<dyn MetadataBackend>>>>,
    // Is this being set to Some(...) during initialization?
}
```

---

## Quick Fix Recommendation

**SHORT TERM:** Make recovery failures ERROR not WARN:

```rust
// In src/lib.rs:349
match storage.recover_from_wal().await {
    Ok(()) => {
        tracing::info!("✅ Vectors recovered from WAL successfully");
    }
    Err(e) => {
        // CHANGE THIS:
        tracing::error!("❌ CRITICAL: WAL recovery failed: {}", e);
        return Err(e.into());  // FAIL STARTUP if recovery fails!
    }
}
```

**LONG TERM:** Fix why recovery manager or metadata provider is None.

---

## Expected Fix

Once fixed, test output should show:

```
STEP 3: Verify Data Recovery
✅ Collection recovered: wal_test_1761607828
✅ Recovered 20/20 vectors  ← Should be 100%

📊 Recovery Status:
   Collection: ✅ Recovered
   Vectors: 20/20 recovered (100%)  ← This

✅ SUCCESS: WAL persistence working correctly!
```

---

## Files to Modify

1. `src/lib.rs:353` - Don't swallow recovery errors
2. `src/storage/engine.rs:286,297` - Don't return Ok() on missing components
3. `src/storage/engine.rs:331` - Don't swallow per-collection errors
4. Investigate why recovery_manager or metadata_provider is None

---

## Testing After Fix

```bash
cd clients/python
export PYTHONPATH=src
python3 tests/server_lifecycle/test_wal_persistence_detailed.py
```

Should show:
- ✅ 20/20 vectors recovered (100%)
- ✅ All metadata preserved
- ✅ WAL files in correct location
