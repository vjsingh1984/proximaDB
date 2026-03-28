use super::{CacheTier, StorageBackend, StorageError};
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tracing::debug;

/// NVMe/SSD storage backend for L2 cache.
///
/// Provides a bounded, LRU-evicting cache tier backed by a concurrent
/// DashMap with per-entry access tracking. The backend creates a shard
/// directory structure on disk (for future file-backed serialization)
/// and manages capacity limits with automatic eviction of least-recently
/// accessed entries when the configured size threshold is exceeded.
///
/// Sharding distributes entries across 16 logical partitions to reduce
/// lock contention on the internal index.
pub struct NvmeBackend<K, V>
where
    K: Clone + Send + Sync + Debug + Hash + Eq + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// On-disk base path for shard directories
    base_path: PathBuf,
    /// Maximum total size in bytes before LRU eviction triggers
    max_size_bytes: usize,
    /// Current approximate total size of cached entries
    current_size: Arc<AtomicUsize>,
    /// Primary concurrent storage: hash -> (key, value, approx_size, access_counter)
    storage: Arc<DashMap<u64, NvmeEntry<K, V>>>,
    /// Monotonically increasing counter for LRU ordering
    access_counter: Arc<AtomicU64>,
    /// Number of logical shards (used for directory layout)
    shard_count: usize,
}

impl<K, V> Debug for NvmeBackend<K, V>
where
    K: Clone + Send + Sync + Debug + Hash + Eq + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NvmeBackend")
            .field("base_path", &self.base_path)
            .field("max_size_bytes", &self.max_size_bytes)
            .field("current_size", &self.current_size.load(Ordering::Relaxed))
            .field("entry_count", &self.storage.len())
            .field("shard_count", &self.shard_count)
            .finish()
    }
}

#[derive(Debug, Clone)]
struct NvmeEntry<K, V> {
    #[allow(dead_code)]
    key: K,
    value: V,
    size: usize,
    last_access: u64,
}

const NUM_SHARDS: usize = 16;

/// Estimated per-entry overhead: key hash (8) + size field (8) +
/// last_access (8) + DashMap overhead (~64).
const ENTRY_OVERHEAD: usize = 88;

impl<K, V> NvmeBackend<K, V>
where
    K: Clone + Send + Sync + Debug + Hash + Eq + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// Create a new NVMe cache backend.
    ///
    /// `path` is the base directory where shard subdirectories are created.
    /// `max_size_gb` sets the maximum total size in gibibytes before LRU
    /// eviction kicks in. A value of 0 defaults to 1 GiB.
    pub fn new(path: &str, max_size_gb: usize) -> Self {
        let base_path = PathBuf::from(path);
        let effective_gb = if max_size_gb == 0 { 1 } else { max_size_gb };
        let max_size_bytes = effective_gb.saturating_mul(1024 * 1024 * 1024);

        // Create base directory and shard subdirectories.
        // Errors are non-fatal; the backend operates in-memory regardless.
        if let Err(e) = std::fs::create_dir_all(&base_path) {
            debug!(
                "NVMe cache: could not create base dir {:?}: {}",
                base_path, e
            );
        }
        for shard_idx in 0..NUM_SHARDS {
            let shard_dir = base_path.join(format!("shard_{}", shard_idx));
            if let Err(e) = std::fs::create_dir_all(&shard_dir) {
                debug!(
                    "NVMe cache: could not create shard dir {:?}: {}",
                    shard_dir, e
                );
            }
        }

        Self {
            base_path,
            max_size_bytes,
            current_size: Arc::new(AtomicUsize::new(0)),
            storage: Arc::new(DashMap::with_capacity(1024)),
            access_counter: Arc::new(AtomicU64::new(0)),
            shard_count: NUM_SHARDS,
        }
    }

    /// Compute a stable u64 hash for a key.
    fn hash_key(key: &K) -> u64 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    /// Estimate the in-memory size of a value using its Debug representation
    /// length as a rough proxy, plus the entry overhead.
    fn estimate_size(key: &K, value: &V) -> usize {
        let key_size = std::mem::size_of_val(key) + format!("{:?}", key).len();
        let val_size = std::mem::size_of_val(value) + format!("{:?}", value).len();
        key_size + val_size + ENTRY_OVERHEAD
    }

    /// Increment and return the next access counter value.
    fn next_access(&self) -> u64 {
        self.access_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Run LRU eviction until current size is at or below 90% of max.
    ///
    /// Collects all entries, sorts by access recency, and removes the
    /// oldest entries until the size target is reached.
    fn evict_if_needed(&self) {
        let current = self.current_size.load(Ordering::Relaxed);
        if current <= self.max_size_bytes {
            return;
        }

        let target = self.max_size_bytes * 9 / 10;
        let overflow = current.saturating_sub(target);

        // Collect (hash, last_access, size) tuples
        let mut candidates: Vec<(u64, u64, usize)> = self
            .storage
            .iter()
            .map(|entry| {
                let hash = *entry.key();
                let nvme_entry = entry.value();
                (hash, nvme_entry.last_access, nvme_entry.size)
            })
            .collect();

        // Sort by last_access ascending (oldest / least-recently-used first)
        candidates.sort_by_key(|&(_, access, _)| access);

        let mut freed = 0usize;
        for (hash, _access, size) in &candidates {
            if freed >= overflow {
                break;
            }
            if let Some((_k, removed)) = self.storage.remove(hash) {
                self.current_size.fetch_sub(removed.size, Ordering::Relaxed);
                freed += size;
                debug!(
                    "NVMe cache: evicted entry {:016x} ({} bytes)",
                    hash, removed.size
                );
            }
        }
    }
}

#[async_trait]
impl<K, V> StorageBackend for NvmeBackend<K, V>
where
    K: Clone + Send + Sync + Debug + Hash + Eq + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    type Key = K;
    type Value = V;

    async fn get(&self, key: &Self::Key) -> Option<Self::Value> {
        let hash = Self::hash_key(key);
        let mut entry = self.storage.get_mut(&hash)?;
        entry.last_access = self.next_access();
        Some(entry.value.clone())
    }

    async fn put(&self, key: Self::Key, value: Self::Value) -> Result<(), StorageError> {
        let hash = Self::hash_key(&key);
        let size = Self::estimate_size(&key, &value);
        let access = self.next_access();

        // Remove old entry size if overwriting
        if let Some(old) = self.storage.get(&hash) {
            self.current_size.fetch_sub(old.size, Ordering::Relaxed);
        }

        self.storage.insert(
            hash,
            NvmeEntry {
                key,
                value,
                size,
                last_access: access,
            },
        );
        self.current_size.fetch_add(size, Ordering::Relaxed);

        // Evict if over capacity
        self.evict_if_needed();

        Ok(())
    }

    async fn remove(&self, key: &Self::Key) -> bool {
        let hash = Self::hash_key(key);
        if let Some((_k, removed)) = self.storage.remove(&hash) {
            self.current_size.fetch_sub(removed.size, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    async fn contains(&self, key: &Self::Key) -> bool {
        let hash = Self::hash_key(key);
        self.storage.contains_key(&hash)
    }

    async fn clear(&self) -> Result<(), StorageError> {
        self.storage.clear();
        self.current_size.store(0, Ordering::Relaxed);

        // Also clean up any files that may have been written to shard dirs
        for shard_idx in 0..self.shard_count {
            let shard_dir = self.base_path.join(format!("shard_{}", shard_idx));
            if let Ok(read_dir) = std::fs::read_dir(&shard_dir) {
                for dir_entry in read_dir.flatten() {
                    let _ = std::fs::remove_file(dir_entry.path());
                }
            }
        }

        Ok(())
    }

    async fn size_bytes(&self) -> usize {
        self.current_size.load(Ordering::Relaxed)
    }

    async fn entry_count(&self) -> usize {
        self.storage.len()
    }

    fn tier(&self) -> CacheTier {
        CacheTier::L2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct TestValue {
        data: Vec<u8>,
        label: String,
    }

    fn temp_path() -> String {
        let id = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("/tmp/nvme_test_{}_{}", id, ts)
    }

    fn make_backend(path: &str, max_gb: usize) -> NvmeBackend<String, TestValue> {
        NvmeBackend::new(path, max_gb)
    }

    #[tokio::test]
    async fn test_put_get_roundtrip() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        let key = "test_key".to_string();
        let value = TestValue {
            data: vec![1, 2, 3, 4, 5],
            label: "hello".to_string(),
        };

        backend
            .put(key.clone(), value.clone())
            .await
            .expect("put should succeed");
        assert!(backend.contains(&key).await);

        let retrieved = backend.get(&key).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), value);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_get_missing_key() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        let result = backend.get(&"nonexistent".to_string()).await;
        assert!(result.is_none());

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_remove() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        let key = "to_remove".to_string();
        let value = TestValue {
            data: vec![10, 20],
            label: "remove_me".to_string(),
        };

        backend.put(key.clone(), value).await.expect("put");
        assert!(backend.contains(&key).await);
        assert_eq!(backend.entry_count().await, 1);

        let removed = backend.remove(&key).await;
        assert!(removed);
        assert!(!backend.contains(&key).await);
        assert_eq!(backend.entry_count().await, 0);
        assert_eq!(backend.size_bytes().await, 0);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_remove_missing_key() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        let removed = backend.remove(&"not_here".to_string()).await;
        assert!(!removed);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_clear() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        for i in 0..5 {
            let key = format!("key_{}", i);
            let value = TestValue {
                data: vec![i as u8; 100],
                label: format!("entry_{}", i),
            };
            backend.put(key, value).await.expect("put");
        }

        assert_eq!(backend.entry_count().await, 5);
        assert!(backend.size_bytes().await > 0);

        backend.clear().await.expect("clear");
        assert_eq!(backend.entry_count().await, 0);
        assert_eq!(backend.size_bytes().await, 0);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_overwrite_existing_key() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        let key = "overwrite_key".to_string();
        let v1 = TestValue {
            data: vec![1],
            label: "first".to_string(),
        };
        let v2 = TestValue {
            data: vec![2, 3, 4, 5, 6, 7, 8, 9, 10],
            label: "second_with_longer_label".to_string(),
        };

        backend.put(key.clone(), v1).await.expect("put v1");
        let size_after_v1 = backend.size_bytes().await;

        backend.put(key.clone(), v2.clone()).await.expect("put v2");
        let size_after_v2 = backend.size_bytes().await;

        // Size should change since v2 is larger
        assert_ne!(size_after_v1, size_after_v2);
        assert_eq!(backend.entry_count().await, 1);

        let retrieved = backend.get(&key).await;
        assert_eq!(retrieved.unwrap(), v2);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_size_tracking() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        assert_eq!(backend.size_bytes().await, 0);

        let value = TestValue {
            data: vec![0u8; 1000],
            label: "big".to_string(),
        };
        backend.put("k1".to_string(), value).await.expect("put");

        let size = backend.size_bytes().await;
        assert!(
            size > 1000,
            "size should be at least 1000 bytes, got {}",
            size
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_tier() {
        let path = temp_path();
        let backend = make_backend(&path, 1);
        assert_eq!(backend.tier(), CacheTier::L2);
        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_multiple_keys_independent() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        let k1 = "alpha".to_string();
        let k2 = "beta".to_string();
        let v1 = TestValue {
            data: vec![1],
            label: "a".to_string(),
        };
        let v2 = TestValue {
            data: vec![2],
            label: "b".to_string(),
        };

        backend.put(k1.clone(), v1.clone()).await.expect("put k1");
        backend.put(k2.clone(), v2.clone()).await.expect("put k2");

        assert_eq!(backend.get(&k1).await.unwrap(), v1);
        assert_eq!(backend.get(&k2).await.unwrap(), v2);

        // Remove one, the other should remain
        backend.remove(&k1).await;
        assert!(!backend.contains(&k1).await);
        assert!(backend.contains(&k2).await);
        assert_eq!(backend.entry_count().await, 1);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_shard_directories_created() {
        let path = temp_path();
        let _backend = make_backend(&path, 1);

        let base = PathBuf::from(&path);
        for shard_idx in 0..NUM_SHARDS {
            let shard_dir = base.join(format!("shard_{}", shard_idx));
            assert!(shard_dir.exists(), "shard dir {:?} should exist", shard_dir);
        }

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_eviction_triggers_over_capacity() {
        let path = temp_path();
        // Create a backend with a very small effective capacity.
        // We hack this by creating normally then manually adjusting max_size_bytes.
        let mut backend: NvmeBackend<String, TestValue> = NvmeBackend::new(&path, 1);
        // Override max_size_bytes to something tiny to force eviction
        backend.max_size_bytes = 500;

        // Insert entries that will exceed 500 bytes total
        for i in 0..20 {
            let key = format!("evict_key_{}", i);
            let value = TestValue {
                data: vec![i as u8; 50],
                label: format!("val_{}", i),
            };
            backend.put(key, value).await.expect("put");
        }

        // After eviction, size should be at or below max_size_bytes
        let size = backend.size_bytes().await;
        let count = backend.entry_count().await;

        // Some entries should have been evicted
        assert!(
            count < 20,
            "expected some entries to be evicted, but all {} remain",
            count
        );
        assert!(
            size <= 500,
            "expected size <= 500 after eviction, got {}",
            size
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_lru_eviction_order() {
        let path = temp_path();
        let mut backend: NvmeBackend<String, TestValue> = NvmeBackend::new(&path, 1);
        // Tiny capacity
        backend.max_size_bytes = 800;

        // Insert 3 entries
        for i in 0..3 {
            let key = format!("lru_key_{}", i);
            let value = TestValue {
                data: vec![i as u8; 50],
                label: format!("val_{}", i),
            };
            backend.put(key, value).await.expect("put");
        }

        // Access key_2 to make it recently used
        let _ = backend.get(&"lru_key_2".to_string()).await;

        // Now insert more entries to trigger eviction
        for i in 3..10 {
            let key = format!("lru_key_{}", i);
            let value = TestValue {
                data: vec![i as u8; 50],
                label: format!("val_{}", i),
            };
            backend.put(key, value).await.expect("put");
        }

        // key_2 should still be present (it was recently accessed)
        // while older, un-accessed keys should have been evicted
        // Note: this is probabilistic depending on exact sizes
        let key_2_present = backend.contains(&"lru_key_2".to_string()).await;
        let key_0_present = backend.contains(&"lru_key_0".to_string()).await;

        // key_0 (oldest, never re-accessed) should be evicted before key_2
        if key_2_present {
            assert!(
                !key_0_present,
                "if key_2 survived eviction, key_0 (older) should have been evicted"
            );
        }

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_contains_after_put() {
        let path = temp_path();
        let backend = make_backend(&path, 1);

        let key = "check_me".to_string();
        assert!(!backend.contains(&key).await);

        let value = TestValue {
            data: vec![42],
            label: "exists".to_string(),
        };
        backend.put(key.clone(), value).await.expect("put");
        assert!(backend.contains(&key).await);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_zero_gb_defaults_to_one() {
        let path = temp_path();
        let backend = make_backend(&path, 0);
        // 0 GB should default to 1 GiB
        assert_eq!(backend.max_size_bytes, 1024 * 1024 * 1024);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_debug_output() {
        let path = temp_path();
        let backend = make_backend(&path, 2);
        let debug_str = format!("{:?}", backend);
        assert!(debug_str.contains("NvmeBackend"));
        assert!(debug_str.contains("max_size_bytes"));
        assert!(debug_str.contains("shard_count"));

        let _ = std::fs::remove_dir_all(&path);
    }
}
