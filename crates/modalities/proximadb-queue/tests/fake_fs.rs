//! Test fixture: an in-memory `QueueFs` impl with controllable behavior
//! - inject slow fsync, failing fsync, failing append, etc. Used by the
//! disk_tier tests to verify Strict-mode semantics without depending on
//! real disk I/O timing.
//!
//! Lives under `tests/` (not `src/`) because it's only useful to test
//! consumers; the production code path always uses `LocalFs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use proximadb_queue::error::QueueError;
use proximadb_queue::fs::{Metadata, QueueFs, Result};

#[derive(Debug, Default, Clone, Copy)]
pub struct FakeFsConfig {
    pub fsync_delay: Duration,
    pub fsync_failure_rate: f32, // 0.0 = never fail, 1.0 = always
    pub append_failure_rate: f32,
}

#[derive(Debug)]
pub struct FakeFs {
    state: Mutex<HashMap<PathBuf, Vec<u8>>>,
    pub fsync_call_count: Arc<AtomicUsize>,
    pub append_call_count: Arc<AtomicUsize>,
    config: FakeFsConfig,
}

impl FakeFs {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(HashMap::new()),
            fsync_call_count: Arc::new(AtomicUsize::new(0)),
            append_call_count: Arc::new(AtomicUsize::new(0)),
            config: FakeFsConfig::default(),
        })
    }

    pub fn with_config(config: FakeFsConfig) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(HashMap::new()),
            fsync_call_count: Arc::new(AtomicUsize::new(0)),
            append_call_count: Arc::new(AtomicUsize::new(0)),
            config,
        })
    }

    pub fn fsync_calls(&self) -> usize {
        self.fsync_call_count.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn append_calls(&self) -> usize {
        self.append_call_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl QueueFs for FakeFs {
    async fn create_dir_all(&self, _path: &Path) -> Result<()> {
        Ok(())
    }

    async fn append(&self, path: &Path, data: &[u8]) -> Result<()> {
        self.append_call_count.fetch_add(1, Ordering::Relaxed);
        if self.config.append_failure_rate >= 1.0 {
            return Err(QueueError::Persistence("fake append failure".into()));
        }
        let mut s = self.state.lock().await;
        s.entry(path.to_path_buf())
            .or_default()
            .extend_from_slice(data);
        Ok(())
    }

    async fn fsync(&self, _path: &Path) -> Result<()> {
        self.fsync_call_count.fetch_add(1, Ordering::Relaxed);
        if self.config.fsync_delay > Duration::ZERO {
            tokio::time::sleep(self.config.fsync_delay).await;
        }
        if self.config.fsync_failure_rate >= 1.0 {
            return Err(QueueError::Persistence("fake fsync failure".into()));
        }
        Ok(())
    }

    async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        let s = self.state.lock().await;
        s.get(path)
            .cloned()
            .ok_or_else(|| QueueError::Persistence(format!("read missing {path:?}")))
    }

    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let s = self.state.lock().await;
        Ok(s.keys().filter(|p| p.starts_with(dir)).cloned().collect())
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let mut s = self.state.lock().await;
        let data = s
            .remove(from)
            .ok_or_else(|| QueueError::Persistence(format!("rename missing {from:?}")))?;
        s.insert(to.to_path_buf(), data);
        Ok(())
    }

    async fn delete(&self, path: &Path) -> Result<()> {
        let mut s = self.state.lock().await;
        s.remove(path);
        Ok(())
    }

    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        let s = self.state.lock().await;
        let size = s.get(path).map(|d| d.len() as u64).unwrap_or(0);
        Ok(Metadata { size_bytes: size })
    }
}
