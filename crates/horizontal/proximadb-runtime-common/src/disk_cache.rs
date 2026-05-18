//! Disk Cache Manager — local caching of frequently accessed files to reduce
//! cloud storage API calls and bandwidth costs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::{debug, trace, warn};

/// Disk cache manager with LRU eviction
pub struct DiskCacheManager {
    cache_dir: PathBuf,
    max_size_bytes: usize,
    current_size: Arc<RwLock<usize>>,
    entries: Arc<DashMap<String, CacheEntry>>,
    stats: Arc<CacheStats>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct CacheEntry {
    local_path: PathBuf,
    size: usize,
    last_accessed: Instant,
    access_count: u64,
    created_at: SystemTime,
}

#[derive(Debug, Default)]
struct CacheStats {
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
    evictions: std::sync::atomic::AtomicU64,
    bytes_saved: std::sync::atomic::AtomicU64,
}

impl DiskCacheManager {
    pub fn new(cache_dir: PathBuf, max_size_gb: usize) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        Self {
            cache_dir,
            max_size_bytes: max_size_gb * 1024 * 1024 * 1024,
            current_size: Arc::new(RwLock::new(0)),
            entries: Arc::new(DashMap::new()),
            stats: Arc::new(CacheStats::default()),
        }
    }

    /// Return the local path string if the key is cached and the file exists.
    pub async fn get(&self, key: &str) -> Option<String> {
        if let Some(mut entry) = self.entries.get_mut(key) {
            entry.last_accessed = Instant::now();
            entry.access_count += 1;
            self.stats
                .hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = entry.local_path.clone();
            if path.exists() {
                debug!("Disk cache hit for {}", key);
                return Some(path.to_string_lossy().to_string());
            }
        }
        self.stats
            .misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        None
    }

    /// Read cached bytes directly, counting bytes saved.
    pub async fn get_data(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(path) = self.get(key).await {
            match tokio::fs::read(&path).await {
                Ok(data) => {
                    self.stats
                        .bytes_saved
                        .fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    Some(data)
                }
                Err(e) => {
                    warn!("Failed to read cached file {}: {}", path, e);
                    self.entries.remove(key);
                    None
                }
            }
        } else {
            None
        }
    }

    /// Write bytes into the cache, evicting LRU entries when over capacity.
    pub async fn put(&self, key: &str, data: &[u8]) {
        let size = data.len();
        let mut current_size = self.current_size.write().await;
        if *current_size + size > self.max_size_bytes {
            self.evict_lru(*current_size + size - self.max_size_bytes)
                .await;
            *current_size = *self.current_size.read().await;
        }
        let cache_path = self.get_cache_path(key);
        if let Err(e) = tokio::fs::write(&cache_path, data).await {
            warn!("Failed to write cache file: {}", e);
            return;
        }
        let entry = CacheEntry {
            local_path: cache_path,
            size,
            last_accessed: Instant::now(),
            access_count: 1,
            created_at: SystemTime::now(),
        };
        self.entries.insert(key.to_string(), entry);
        *current_size += size;
        trace!("Cached {} bytes for key {}", size, key);
    }

    pub async fn invalidate(&self, key: &str) {
        if let Some((_, entry)) = self.entries.remove(key) {
            let _ = tokio::fs::remove_file(&entry.local_path).await;
            let mut current_size = self.current_size.write().await;
            *current_size = current_size.saturating_sub(entry.size);
        }
    }

    pub async fn invalidate_prefix(&self, prefix: &str) {
        let keys_to_remove: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| entry.key().starts_with(prefix))
            .map(|entry| entry.key().clone())
            .collect();
        for key in keys_to_remove {
            self.invalidate(&key).await;
        }
    }

    fn get_cache_path(&self, key: &str) -> PathBuf {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        self.cache_dir
            .join(format!("{:016x}.cache", hasher.finish()))
    }

    async fn evict_lru(&self, bytes_needed: usize) {
        let mut entries: Vec<(String, Instant, usize)> = self
            .entries
            .iter()
            .map(|e| (e.key().clone(), e.value().last_accessed, e.value().size))
            .collect();
        entries.sort_by_key(|(_, time, _)| *time);
        let mut freed = 0;
        for (key, _, size) in entries {
            if freed >= bytes_needed {
                break;
            }
            self.invalidate(&key).await;
            freed += size;
            self.stats
                .evictions
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn stats(&self) -> DiskCacheStatistics {
        DiskCacheStatistics {
            hits: self.stats.hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.stats.misses.load(std::sync::atomic::Ordering::Relaxed),
            evictions: self
                .stats
                .evictions
                .load(std::sync::atomic::Ordering::Relaxed),
            bytes_saved: self
                .stats
                .bytes_saved
                .load(std::sync::atomic::Ordering::Relaxed),
            entries: self.entries.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiskCacheStatistics {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub bytes_saved: u64,
    pub entries: usize,
}

impl DiskCacheStatistics {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}
