# CRITICAL BUG: Vector Data Not Recovering After Server Restart

## Severity: **CRITICAL** - Data Loss

## Bug Description

Vectors inserted into collections are **NOT recovered** after server restart, despite:
- Collection metadata being recovered correctly
- WAL files potentially being written
- No errors during shutdown/startup

## Reproduction

```bash
cd clients/python
export PYTHONPATH=src
python3 tests/server_lifecycle/test_wal_persistence_detailed.py
```

## Observed Behavior

```
STEP 1: Create Collection and Insert Data
✅ Collection: wal_test_1761607828
   ID: 1vDWfUD
   Storage: None  ← SUSPICIOUS: Should have storage path
✅ Inserted 20 vectors

STEP 2: Restart Server
(Server restarts successfully)

STEP 3: Verify Data Recovery
✅ Collection recovered: wal_test_1761607828
✅ Recovered 0/20 vectors  ← CRITICAL: All vectors lost!

Result: 0% recovery rate
```

## Expected Behavior

- Collection recovered: ✅ (working)
- Vectors recovered: Should be >=90% (18/20 minimum)
- Storage path: Should be `file:///tmp/proximadb/d{N}/...`

## Root Cause Analysis

### Issue 1: Storage Path is None

Collection creation returns `Storage: None` instead of actual storage assignment.

**Possible causes:**
1. Storage assignment not included in create_collection response
2. Response parsing issue in Python SDK
3. Server not assigning storage on collection creation

### Issue 2: Vectors Not Recovered

Even if WAL files exist, vectors are not found after restart.

**Possible causes:**
1. WAL recovery not running during server startup
2. WAL files not being written despite success message
3. Search not checking WAL after recovery
4. Collection-to-storage mapping lost on restart

## Investigation Needed

### Server Side (Rust):

1. **Check WAL write:**
   - Are WAL files actually being written to disk?
   - Location: `/tmp/proximadb/d{N}/{collection_id}/wal/*.bcwal`

2. **Check WAL recovery:**
   - Does server read WAL files on startup?
   - Log message: "Recovering from WAL" or similar
   - Check: `src/storage/persistence/write_ahead_log/recovery_manager.rs`

3. **Check storage assignment:**
   - Why is storage_assignment null in response?
   - Check: `src/services/collection/manager.rs` create_collection response

4. **Check search after recovery:**
   - Does search check WAL/memtable after recovery?
   - Or only persistent storage?

### Client Side (Python):

1. **Check response parsing:**
   - How is `collection.storage_assignment` extracted?
   - Is field name different in proto?

## Temporary Workarounds

**None** - This is a data loss bug with no workaround.

Users should NOT rely on server restart preserving data until this is fixed.

## Impact

**CRITICAL for production deployment:**
- ❌ Cannot safely restart server without data loss
- ❌ Kubernetes pod restarts would lose data
- ❌ Server crashes would lose all unflushed data
- ❌ Rolling updates impossible

## Priority

**P0 - BLOCKER** for production use

Must be fixed before:
- Production deployment
- Beta release
- Any customer-facing use

## Test Files Affected

- `test_wal_persistence_detailed.py` - Fails with 0% recovery
- `test_comprehensive_recovery.py` - Likely also fails
- All tests that rely on server restart

## Related Logs

Server shows during insert:
```
INFO 💾 WAL batch 8WkqMzPePA written to disk for collection 1vDVviz (81 vectors)
```

But after restart, search returns 0 results.

## Next Steps

1. **Immediate:** Add debug logging to WAL recovery
2. **Urgent:** Fix storage_assignment in create_collection response
3. **Urgent:** Verify WAL recovery runs on server startup
4. **Urgent:** Test WAL file reading manually

## Manual Verification

```bash
# After running test, check if WAL files exist
find /tmp/proximadb -name "*.bcwal" -exec ls -lh {} \;

# Check if files contain data
find /tmp/proximadb -name "*.bcwal" -exec wc -c {} \;
```

If files exist and have data, the bug is in recovery logic.
If files don't exist, the bug is in write logic.
