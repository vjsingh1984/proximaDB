# Lock Contention Analysis & Optimization Strategy

## Executive Summary
Profiling and optimization strategy for identifying and fixing lock contention hotspots in ProximaDB.

## Current Lock Usage Analysis

### HashMap Protected by Locks
**Candidates for DashMap replacement:**
1. `UnifiedQuantizationEngine::codebook_cache` - ✅ **FIXED** (replaced with DashMap)
2. Swift ID index maps - **HIGH PRIORITY** (async context)
3. Metadata caches - **MEDIUM PRIORITY**
4. Query result caches - **MEDIUM PRIORITY**

### VecDeque Protected by Mutex
**Candidates for lock-free queues:**
1. WAL write queue
2. Compaction queue
3. Event processing queue

### BTreeMap Usage
**Note**: std::collections::BTreeMap is appropriate for in-memory ordered data.
Our internal BPlusTree is optimized for disk I/O, not in-memory operations.

## Profiling Strategy

### 1. Add Instrumentation
```rust
use std::time::Instant;
use metrics::{histogram, counter};

pub struct InstrumentedRwLock<T> {
    inner: RwLock<T>,
    name: &'static str,
}

impl<T> InstrumentedRwLock<T> {
    pub fn read(&self) -> RwLockReadGuard<T> {
        let start = Instant::now();
        let guard = self.inner.read().unwrap();
        let duration = start.elapsed();
        
        histogram!("lock_acquisition_time", duration, "lock" => self.name, "type" => "read");
        
        if duration > Duration::from_micros(100) {
            counter!("lock_contention_events", 1, "lock" => self.name);
        }
        
        guard
    }
}
```

### 2. Production Profiling Commands
```bash
# Using perf to identify lock contention
perf record -g -e 'sched:sched_switch' ./proximadb-server
perf report --stdio | grep -A5 "futex_wait"

# Using bpftrace for real-time analysis
bpftrace -e 'tracepoint:syscalls:sys_enter_futex { @locks[kstack] = count(); }'

# Using tokio-console for async runtime analysis
RUSTFLAGS="--cfg tokio_unstable" cargo build
tokio-console
```

### 3. Key Metrics to Monitor
- Lock acquisition time (p50, p95, p99)
- Lock hold time
- Queue depth for locks
- Context switches per second
- CPU time spent in futex_wait

## Optimization Decision Tree

```
Is the lock experiencing contention? (>100μs acquisition time)
├─ NO → Keep current implementation
└─ YES → What data structure is protected?
    ├─ HashMap → Consider:
    │   ├─ DashMap (if no ordering needed)
    │   ├─ Sharded HashMap (if custom logic needed)
    │   └─ Read-copy-update pattern
    ├─ VecDeque → Consider:
    │   ├─ crossbeam::channel (MPMC)
    │   ├─ tokio::sync::mpsc (async)
    │   └─ Lock-free queue (custom)
    └─ Complex State → Consider:
        ├─ Actor pattern
        ├─ State machine with CAS
        └─ Optimistic locking

```

## Specific Recommendations

### 1. Replace High-Contention HashMaps with DashMap
**When to use DashMap:**
- High read/write concurrency
- No ordering requirements
- Key-value cache patterns
- No complex transactions

**When NOT to use DashMap:**
- Need consistent iteration
- Complex multi-key transactions
- Memory constraints (DashMap has overhead)

### 2. Replace VecDeque<Task> with Channels
```rust
// Before: Mutex<VecDeque<Task>>
let queue = Arc::new(Mutex::new(VecDeque::new()));

// After: crossbeam channel (lock-free)
let (sender, receiver) = crossbeam::channel::unbounded();
```

### 3. Use Sharding for Custom Logic
```rust
pub struct ShardedMap<K, V> {
    shards: Vec<RwLock<HashMap<K, V>>>,
    hasher: RandomState,
}

impl<K: Hash, V> ShardedMap<K, V> {
    fn get_shard(&self, key: &K) -> usize {
        let hash = self.hasher.hash_one(key);
        hash as usize % self.shards.len()
    }
    
    pub fn get(&self, key: &K) -> Option<V> {
        let shard = self.get_shard(key);
        self.shards[shard].read().unwrap().get(key).cloned()
    }
}
```

## Benchmark Suite for Lock Contention

```rust
#[bench]
fn bench_hashmap_concurrent_reads(b: &mut Bencher) {
    let map = Arc::new(RwLock::new(HashMap::new()));
    // Populate map...
    
    b.iter(|| {
        let handles: Vec<_> = (0..100).map(|_| {
            let map = map.clone();
            thread::spawn(move || {
                map.read().unwrap().get(&random_key())
            })
        }).collect();
        
        for h in handles {
            h.join().unwrap();
        }
    });
}

#[bench]
fn bench_dashmap_concurrent_reads(b: &mut Bencher) {
    let map = Arc::new(DashMap::new());
    // Populate map...
    
    b.iter(|| {
        let handles: Vec<_> = (0..100).map(|_| {
            let map = map.clone();
            thread::spawn(move || {
                map.get(&random_key())
            })
        }).collect();
        
        for h in handles {
            h.join().unwrap();
        }
    });
}
```

## Expected Performance Improvements

### DashMap vs RwLock<HashMap>
| Scenario | RwLock<HashMap> | DashMap | Improvement |
|----------|----------------|---------|-------------|
| 100% reads | 1M ops/sec | 8M ops/sec | 8x |
| 80/20 read/write | 500K ops/sec | 4M ops/sec | 8x |
| 50/50 read/write | 100K ops/sec | 2M ops/sec | 20x |
| High contention | 10K ops/sec | 1M ops/sec | 100x |

### Channel vs Mutex<VecDeque>
| Scenario | Mutex<VecDeque> | crossbeam::channel | Improvement |
|----------|-----------------|-------------------|-------------|
| Single producer | 1M ops/sec | 10M ops/sec | 10x |
| Multi producer | 100K ops/sec | 5M ops/sec | 50x |
| Burst traffic | 50K ops/sec | 8M ops/sec | 160x |

## Implementation Priority

### Phase 1 (Immediate)
1. ✅ Replace UnifiedQuantizationEngine cache with DashMap
2. Fix Swift ID index async locks
3. Add lock instrumentation to top 10 hotspots

### Phase 2 (This Week)
1. Profile under production-like load
2. Replace high-contention HashMaps with DashMap
3. Replace task queues with channels

### Phase 3 (Next Sprint)
1. Implement sharded maps for complex cases
2. Add lock-free skip list for ordered data
3. Optimize remaining bottlenecks

## Monitoring Dashboard

Add these metrics to Grafana:
```yaml
panels:
  - title: "Lock Acquisition Time"
    query: "histogram_quantile(0.99, lock_acquisition_time)"
    
  - title: "Lock Contention Events"
    query: "rate(lock_contention_events[1m])"
    
  - title: "Async Runtime Blocking"
    query: "tokio_runtime_blocking_time"
    
  - title: "Context Switches"
    query: "rate(node_context_switches_total[1m])"
```

## Conclusion

Lock contention is often the #1 scalability bottleneck. By:
1. **Profiling first** to identify actual hotspots
2. **Replacing locks with lock-free structures** where appropriate
3. **Monitoring continuously** in production

We can achieve:
- **10-100x improvement** in high-contention scenarios
- **Linear scalability** with CPU cores
- **Predictable latency** under load
- **Elimination of priority inversion** and convoy effects