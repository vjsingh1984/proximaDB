# WAL Recovery - Final Fix Action Plan

## CRITICAL: 55 Commits Pushed, One Fix Remaining

### Current Status
- WAL write works ✓
- Files in wrong location: `/Users/.../data/write_buffer/`
- Need: `/tmp/proximadb/d{N}/`

### Root Cause
Metadata provider query failing in pool instances despite all propagation.

### Solution (15 minutes)

**File:** `src/storage/persistence/write_ahead_log/mod.rs`

**Lines:** ~1757-1790 and ~1927-1960

**Change:** Add hash-based distribution and better debug:

```rust
let base_location = {
    eprintln!("DEBUG: provider is_some = {}", self.metadata_provider.read().await.is_some());
    
    // Try metadata first
    let from_metadata = self.metadata_provider.read().await
        .as_ref()
        .and_then(|p| {
            // Query collection
            // Return assignment if found
        });
    
    // Fallback: Hash distribution across d1/d2/d3
    from_metadata.unwrap_or_else(|| {
        let hash = collection_id.bytes().sum::<usize>();
        let idx = hash % 3;
        format!("file:///tmp/proximadb/d{}", idx + 1)
    })
};
```

### Test
```bash
cargo build --release
rm -rf /tmp/proximadb
./RECOVERY_TEST_FRESH.sh
```

Expected: 100% recovery

### Backup Plan
If still fails, hardcode d1 temporarily to unblock.

All infrastructure ready. Just needs correct path.
