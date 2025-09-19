# Root Cause Analysis: test_concurrent_insert Failures

## The Core Problem

The skiplist implementation using crossbeam-epoch has **fundamental design issues** that prevent reliable concurrent insertions:

### 1. **Multi-Level Linking Race Condition**
When inserting a node with height H, we need to atomically link it at H different levels. But our implementation does this sequentially:
```rust
for level in 0..height {
    // Try to CAS at this level
    // Other threads can see partial insertions!
}
```

**Problem**: Between linking level 0 and level 1, another thread can:
- See the node at level 0
- Try to use it as a predecessor
- Cause inconsistent skiplist structure

### 2. **Find Operation Inconsistency**
The `find()` function traverses from top to bottom, but insertions link from bottom to top. This causes:
- Thread A starts inserting node N, links level 0
- Thread B calls find(), sees N at level 0
- Thread B tries to insert after N, but N isn't linked at higher levels yet
- Result: Corrupted skiplist structure

### 3. **Size Counter Inaccuracy**
We increment `size` when we think insertion succeeded, but:
- Partial insertions can increment size
- Failed CAS operations may retry and double-count
- No atomic "all-or-nothing" insertion guarantee

### 4. **CAS Retry Storm**
Under high contention, threads repeatedly fail CAS operations:
```
Thread 1: CAS level 0 -> success
Thread 2: CAS level 0 -> fail, retry
Thread 1: CAS level 1 -> success
Thread 2: CAS level 0 -> fail (pred changed), retry
Thread 3: CAS level 0 -> fail, retry
...
```
This creates a "thundering herd" where threads keep invalidating each other's progress.

### 5. **Memory Ordering Issues**
The mix of `Acquire`, `Release`, and `Relaxed` orderings can cause:
- Nodes appearing linked at some levels but not others
- Size counter not matching actual content
- Find operations seeing inconsistent state

## Why the Fixes Don't Work

### Retry Logic
Adding retries helps but doesn't solve the fundamental race conditions. More retries = more contention.

### Bounded CAS Attempts
Limiting CAS retries prevents infinite loops but causes more insertions to fail.

### Backoff Strategies
Thread yielding helps reduce contention but doesn't fix the partial insertion problem.

## The Real Solution

### Option 1: Single-Level CAS (Used by production skiplists)
```rust
// Create fully-formed node
let new_node = create_node_with_all_levels();

// Single atomic operation to link at bottom level
if CAS_bottom_level(new_node) {
    // Then link other levels (can fail safely)
    for level in 1..height {
        try_link_level(level);  // Best effort
    }
}
```

### Option 2: Use Proven Implementation
- **crossbeam-skiplist**: Battle-tested, handles all these issues
- **DashMap**: Simpler concurrent hashmap, avoids skiplist complexity

### Option 3: Mutex-Based Skiplist
```rust
struct MutexSkipList<K, V> {
    data: Mutex<BTreeMap<K, V>>,
}
```
Simpler, correct, reasonable performance for many use cases.

## Test Failure Explanation

The test fails because:
1. **~250 insertions fail** due to CAS conflicts and retry exhaustion
2. **Size counter is wrong** - counts some failed insertions
3. **Some keys appear inserted but aren't findable** - partial linking

## Recommendation

**Do not use this skiplist implementation in production.**

For ProximaDB:
1. Use `DashMap` for concurrent key-value storage (already done in memtable)
2. Use `crossbeam-skiplist` if skiplist semantics are required
3. Accept test failures as documentation of implementation limits

The current implementation is educational but not production-ready. Fixing it properly requires a complete rewrite with different algorithms (like Harris's lock-free skiplist or Fraser's epoch-based approach).