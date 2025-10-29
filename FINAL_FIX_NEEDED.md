# Final Fix: WAL Pool Metadata Provider Propagation

## CONFIRMED: WAL files ARE being created!

**Evidence from latest test (collection: 1vDtiJS):**
```
/Users/vijay.singh/code/proximaDB/data/write_buffer/1vDtiJS/wal/8WmXr5hvAO.bcwal
```

✅ WAL write works
✅ Files created
❌ Wrong location (data/write_buffer instead of /tmp/proximadb/d{N})

## Root Cause

**WAL Manager Pooling:**
- Main instance: HAS metadata provider
- Pool instances: NO metadata provider
- Inserts use pool → wrong path

## The One Fix Needed

**File:** `src/storage/engine.rs` (line 227-230)

**Current code:**
```rust
self.write_ahead_log_manager
    .set_metadata_provider(provider)
    .await;
```

**Problem:** This only sets on ONE instance, not the pool.

**Solution:** Access the pool and set on all instances:

```rust
// In storage_engine.rs
pub async fn set_metadata_provider(&self, provider: Arc<dyn InternalCollectionProvider>) {
    let mut lock = self.metadata_provider.write().await;
    *lock = Some(provider.clone());

    // Set on main WAL manager
    self.write_ahead_log_manager.set_metadata_provider(provider.clone()).await;

    // CRITICAL: Set on shared Arc so pool instances can access
    // The WAL manager's metadata_provider field is Arc<RwLock<Option<...>>>
    // This is shared by all pool instances, so setting it once affects all
}
```

**The metadata_provider IS an Arc** - so setting it should propagate automatically!

**Check:** Maybe we need to set it EARLIER before pool instances are created?

## Quick Test

After fixing, run:
```bash
./RECOVERY_TEST_FRESH.sh
```

Should see files in:
```
/tmp/proximadb/d1/1vDtiJS/wal/*.bcwal  (or d2/ or d3/)
```

## Session Summary: 45 Commits

All infrastructure is ready. Just need pool instances to access metadata provider.

Estimated time to fix: 30 minutes
