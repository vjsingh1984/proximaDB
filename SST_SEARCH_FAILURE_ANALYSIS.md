# SST Engine Search Failure Analysis

## Executive Summary

**Root Cause**: The SST engine's `discover_sstable_files()` method incorrectly parses the storage URL, causing it to look for files in the wrong directory.

**Impact**: All SST search operations fail immediately after successful flush with "No such file or directory" error.

**Status**: Bug introduced by incorrect path parsing logic in search module.

---

## Detailed Analysis

### 1. Path Construction Flow

#### Flush Operation (CORRECT)

**File**: `src/storage/engines/impls/sst/flush/mod.rs`

**Line 155-161**:
```rust
let storage_url = StoragePath::collection_data_path(
    &assignment.base_location,  // "/tmp/proximadb-bench"
    params.collection_id        // "sst-none"
);
```

**Utility Function** (`src/utils/storage_path.rs`, Line 19-21):
```rust
pub fn collection_data_path(base_url: &str, collection_id: &str) -> String {
    format!("{}/{}/data", base_url, collection_id)
}
```

**Result**: Flush writes to `/tmp/proximadb-bench/sst-none/data/*.sst`

#### Search Operation (INCORRECT)

**File**: `src/storage/engines/impls/sst/search/mod.rs`

**Line 70-73**: Search receives storage_url from context
```rust
let storage_url = ctx
    .collection_storage_path()
    .ok_or_else(|| SstError::InvalidArgument("No storage URL in context".into()))?;
```

**Context Method** (`src/storage/traits.rs`, Line 1850-1853):
```rust
pub fn collection_storage_path(&self) -> Option<String> {
    self.storage_url()  // Gets base_location from storage_assignment
        .map(|base| crate::utils::StoragePath::collection_data_path(base, &self.collection_id()))
}
```

This returns: `/tmp/proximadb-bench/sst-none/data`

**Line 178**: Search passes this to `discover_sstable_files()`
```rust
let sstable_files = self.discover_sstable_files(storage_url).await?;
```

**Line 228-233**: The bug occurs here
```rust
async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();

    // Parse collection path to extract base URL and collection ID
    let (base_url, collection_id) = self.parse_storage_url(storage_url)?;
    let data_url = crate::utils::StoragePath::collection_data_path(&base_url, &collection_id);
    // ...
}
```

### 2. The Bug: parse_storage_url()

**File**: `src/storage/engines/impls/sst/search/mod.rs`, Line 250-264

```rust
fn parse_storage_url(&self, storage_url: &str) -> Result<(String, String)> {
    if let Some((base, coll)) = crate::utils::StoragePath::parse_collection_path(
        &format!("{}/dummy", storage_url)
    ) {
        Ok((base, coll))
    } else {
        // Fallback: assume storage_url is base_url/collection_id format
        if let Some(last_slash) = storage_url.rfind('/') {
            let base = &storage_url[..last_slash];  // BUG HERE!
            let collection = &storage_url[last_slash + 1..];
            Ok((base.to_string(), collection.to_string()))
        } else {
            Err(SstError::InvalidArgument(format!("Invalid storage URL format: {}", storage_url)).into())
        }
    }
}
```

### 3. The Bug Explained

**Input**: `storage_url = "/tmp/proximadb-bench/sst-none/data"`

**Parsing Logic** (Line 257-259):
```rust
if let Some(last_slash) = storage_url.rfind('/') {
    let base = &storage_url[..last_slash];        // "/tmp/proximadb-bench/sst-none"
    let collection = &storage_url[last_slash + 1..];  // "data"
```

**Extracted Values**:
- `base_url` = `/tmp/proximadb-bench/sst-none`
- `collection_id` = `"data"` ❌ (WRONG! Should be "sst-none")

**Reconstruction** (Line 233):
```rust
let data_url = crate::utils::StoragePath::collection_data_path(&base_url, &collection_id);
// Expands to: format!("{}/{}/data", "/tmp/proximadb-bench/sst-none", "data")
// Result: "/tmp/proximadb-bench/sst-none/data/data"
```

**Final Path**: Search looks in `/tmp/proximadb-bench/sst-none/data/data/*.sst` ❌

**Correct Path**: Files are actually in `/tmp/proximadb-bench/sst-none/data/*.sst` ✓

### 4. Path Comparison Table

| Operation | Input | Intermediate | Final Path | Status |
|-----------|-------|--------------|------------|--------|
| **Flush** | base=`/tmp/proximadb-bench`<br>collection=`sst-none` | N/A | `/tmp/proximadb-bench/sst-none/data/*.sst` | ✓ Correct |
| **Search** | storage_url=`/tmp/proximadb-bench/sst-none/data` | Parsed:<br>base=`/tmp/proximadb-bench/sst-none`<br>collection=`data` | `/tmp/proximadb-bench/sst-none/data/data/*.sst` | ❌ Wrong |

---

## Impact Assessment

### Affected Components
1. **SST Engine**: All search operations fail
2. **Benchmark**: bench_04_storage_unified.rs shows immediate failure
3. **Production**: Any SST-based collection searches would fail

### Error Manifestation
```
❌ Pure search failed for sst with none: IO error: No such file or directory (os error 2)
```

This occurs at Line 237 in `discover_sstable_files()`:
```rust
let entries = fs.list(&data_url).await?;  // Fails: data_url is wrong
```

---

## Root Cause Summary

**The bug is a logic error in path parsing**:

1. `discover_sstable_files()` receives a **data directory path** (`base/collection/data`)
2. `parse_storage_url()` **assumes** it receives a **collection path** (`base/collection`)
3. It splits on the last `/` and treats `"data"` as the collection ID
4. It then reconstructs the path, adding `/data` again, creating `base/collection/data/data`

**The fix**: `discover_sstable_files()` should **NOT** re-parse and reconstruct the path. It should use the storage_url directly since it's already the correct data directory path.

---

## Recommended Fix

### Option 1: Use storage_url Directly (Simplest)

**File**: `src/storage/engines/impls/sst/search/mod.rs`, Line 228-246

**Current Code**:
```rust
async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();

    // Parse collection path to extract base URL and collection ID
    let (base_url, collection_id) = self.parse_storage_url(storage_url)?;
    let data_url = crate::utils::StoragePath::collection_data_path(&base_url, &collection_id);

    // List files in the collection directory
    let fs = self.filesystem().get_filesystem(&data_url)?;
    let entries = fs.list(&data_url).await?;
    // ...
}
```

**Fixed Code**:
```rust
async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();

    // storage_url is already the data directory path from collection_storage_path()
    // No need to parse and reconstruct - use it directly
    let data_url = storage_url;

    // List files in the collection directory
    let fs = self.filesystem().get_filesystem(data_url)?;
    let entries = fs.list(data_url).await?;

    for entry in entries {
        if !entry.metadata.is_directory && entry.name.ends_with(".sst") {
            files.push(entry.url);
        }
    }

    debug!("📂 Discovered {} SST files in {}", files.len(), data_url);
    Ok(files)
}
```

### Option 2: Fix parse_storage_url() Logic (More Complex)

**File**: `src/storage/engines/impls/sst/search/mod.rs`, Line 250-264

**Current Code**:
```rust
fn parse_storage_url(&self, storage_url: &str) -> Result<(String, String)> {
    // ... existing logic ...
    if let Some(last_slash) = storage_url.rfind('/') {
        let base = &storage_url[..last_slash];
        let collection = &storage_url[last_slash + 1..];
        Ok((base.to_string(), collection.to_string()))
    }
}
```

**Fixed Code**:
```rust
fn parse_storage_url(&self, storage_url: &str) -> Result<(String, String)> {
    // Handle case where storage_url is already a data path (ends with /data)
    let path_to_parse = if storage_url.ends_with("/data") {
        &storage_url[..storage_url.len() - 5]  // Remove "/data"
    } else {
        storage_url
    };

    // ... existing logic using path_to_parse ...
}
```

---

## Testing Verification

### Before Fix
```bash
cargo bench --bench bench_04_storage_unified 2>&1 | grep "sst.*none"
```
Expected output:
```
✓ Flushed 1000 vectors, 8770856 bytes written in 17ms
✓ Created 1 files/directories:
      data (DIR)
❌ Pure search failed for sst with none: IO error: No such file or directory (os error 2)
```

### After Fix
Expected output:
```
✓ Flushed 1000 vectors, 8770856 bytes written in 17ms
✓ Created 1 files/directories:
      data (DIR)
✅ FOUND: Pure search returned 10 results for sst with none in 2ms
```

---

## Additional Notes

### Why This Bug Wasn't Caught Earlier

1. **Different path conventions**: Flush and search use different entry points
2. **No integration tests**: No tests verify flush → search roundtrip
3. **Recent refactoring**: The modularization of SST engine may have introduced this

### Related Issues

This same pattern may affect other engines:
- VIPER: Check `discover_parquet_files()`
- NOVA: Check similar discovery methods
- Other engines: Verify path parsing logic

### Hyphen Change Impact

The recent change from `sst_none` to `sst-none` (using hyphens) is **NOT** the root cause. The bug would occur with either naming convention. The hyphen change simply made the bug more visible in the benchmark output.

---

## Implementation Priority

**Priority**: HIGH - Blocks all SST search operations

**Recommended Approach**: Option 1 (use storage_url directly)
- Simpler
- Less error-prone
- Removes unnecessary parsing logic
- Aligns with how other engines work

**Estimated Fix Time**: 15 minutes + testing

---

## File Locations Summary

| File | Lines | Issue |
|------|-------|-------|
| `benches/bench_04_storage_unified.rs` | 212-217, 265-280, 438-453 | Benchmark setup (correct) |
| `src/storage/engines/impls/sst/flush/mod.rs` | 155-161 | Flush path construction (correct) |
| `src/storage/engines/impls/sst/search/mod.rs` | 228-246 | discover_sstable_files() (BUG HERE) |
| `src/storage/engines/impls/sst/search/mod.rs` | 250-264 | parse_storage_url() (PROBLEMATIC) |
| `src/storage/traits.rs` | 1850-1853 | collection_storage_path() (correct) |
| `src/utils/storage_path.rs` | 19-21 | collection_data_path() (correct) |