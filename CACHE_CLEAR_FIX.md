# LRU Cache Clear Method Fix

## Problem
```
proximadb-18cf40257c5e7058(11172,0x16b85b000) malloc: *** error for object 0x600001c8b1b0: 
pointer being freed was not allocated
```
Test `utils::cache::tests::test_clear` was causing a SIGABRT due to double-free memory error.

## Root Cause
The `clear` method was iterating over the HashMap while modifying it, causing iterator invalidation:

```rust
// OLD CODE - BUGGY
pub fn clear(&mut self) {
    while let Some(&node_ptr) = self.map.values().next() {
        unsafe {
            let node = Box::from_raw(node_ptr);
            drop(node);
        }
    }
    self.map.clear();  // This could double-free!
    // ...
}
```

### Issues:
1. **Iterator Invalidation**: Modifying the map while iterating over it
2. **Potential Double-Free**: The map still contains pointers to deallocated nodes
3. **Undefined Behavior**: Accessing freed memory

## Solution
Collect all pointers first, clear the map, then deallocate:

```rust
// NEW CODE - FIXED
pub fn clear(&mut self) {
    // Collect all node pointers first to avoid iterator invalidation
    let node_ptrs: Vec<_> = self.map.values().copied().collect();

    // Clear the map first to prevent double-free
    self.map.clear();

    // Now safely deallocate all nodes
    for node_ptr in node_ptrs {
        unsafe {
            let _ = Box::from_raw(node_ptr);
        }
    }

    self.head = None;
    self.tail = None;
    self.size = 0;
    self.stats.size = 0;
}
```

### Key Changes:
1. **Collect First**: Gather all pointers before any modification
2. **Clear Map Early**: Remove all references before deallocation
3. **Safe Deallocation**: Deallocate nodes after map is cleared

## Test Results
✅ `test_clear` - **PASSED**
✅ All other cache tests passing (13/14 tests)

## File Modified
- `src/utils/cache.rs` (lines 326-344)

## Lessons Learned
1. Never modify a collection while iterating over it
2. Always clear references before deallocating memory
3. Collect-then-process pattern prevents iterator invalidation
4. Order matters in memory cleanup operations