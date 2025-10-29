# Server Startup Hang After Global Metadata Provider

## Status
Server hangs indefinitely during startup after commit e66c5b01.

## Evidence
```
09:10:15 INFO Creating RecoveryManager with direct-to-storage recovery
(hung for 5+ minutes, no further output)
```

## Cause
Added GLOBAL_METADATA_PROVIDER singleton (line 947-949).
Likely deadlock during initialization or OnceLock issue.

## Rollback If Needed
```bash
git revert e66c5b01
cargo build --release
```

## Alternative Approach
Don't use global - pass parent's Arc through constructor parameter.

## Investigation Next Session
Add eprintln! before/after OnceLock::get_or_init to find exact hang point.

All other fixes (48 commits) are working correctly.
