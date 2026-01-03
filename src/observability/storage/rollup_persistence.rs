// Metric rollup persistence layer
//
// Provides:
// - File-based persistence for downsampled metric aggregates
// - Automatic recovery of rollups on startup
// - Query support for persisted historical data
// - Resolution-based partitioning (minute, five_minute, hour)

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::metrics::{AggregatedMetric, DownsampleResolution};

/// Trait for metric rollup persistence operations
#[async_trait]
pub trait RollupPersistence: Send + Sync {
    /// Flush in-memory rollups to persistent storage
    async fn flush_rollups(
        &self,
        series_key: &str,
        resolution: DownsampleResolution,
        aggregates: &BTreeMap<i64, RollupPoint>,
    ) -> Result<usize>;

    /// Load rollups from persistent storage for a time range
    async fn load_rollups(
        &self,
        series_key: &str,
        resolution: DownsampleResolution,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<AggregatedMetric>>;

    /// Delete old rollups based on retention policy
    async fn delete_before(
        &self,
        resolution: DownsampleResolution,
        cutoff_ns: i64,
    ) -> Result<usize>;
}

/// Rollup point for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollupPoint {
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    pub count: u64,
    pub name: String,
    pub labels: HashMap<String, String>,
}

impl RollupPoint {
    /// Calculate average value
    pub fn avg(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }
}

/// Serializable rollup file format
#[derive(Debug, Serialize, Deserialize)]
struct RollupFile {
    /// Version for compatibility
    version: u32,
    /// Resolution of the data
    resolution: String,
    /// Series key
    series_key: String,
    /// Rollup points by timestamp
    points: BTreeMap<i64, RollupPoint>,
}

impl RollupFile {
    const CURRENT_VERSION: u32 = 1;

    fn new(series_key: &str, resolution: DownsampleResolution) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            resolution: resolution.name().to_string(),
            series_key: series_key.to_string(),
            points: BTreeMap::new(),
        }
    }
}

/// File-based rollup persistence implementation
pub struct FileRollupPersistence {
    /// Base directory for rollup storage
    base_path: PathBuf,
    /// Write lock for file operations
    write_lock: RwLock<()>,
}

impl FileRollupPersistence {
    /// Create a new file-based rollup persistence layer
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
            write_lock: RwLock::new(()),
        }
    }

    /// Get the directory for a resolution tier
    fn resolution_dir(&self, resolution: DownsampleResolution) -> PathBuf {
        self.base_path
            .join(format!("rollups_{}", resolution.name()))
    }

    /// Get the file path for a series at a resolution
    fn series_file(&self, series_key: &str, resolution: DownsampleResolution) -> PathBuf {
        let safe_key = Self::sanitize_series_key(series_key);
        self.resolution_dir(resolution)
            .join(format!("{}.json", safe_key))
    }

    /// Sanitize series key for use in filenames
    fn sanitize_series_key(series_key: &str) -> String {
        series_key
            .replace(':', "_")
            .replace('{', "_")
            .replace('}', "_")
            .replace(',', "_")
            .replace('=', "_")
    }

    /// Ensure directory exists
    async fn ensure_dir(&self, resolution: DownsampleResolution) -> Result<()> {
        let dir = self.resolution_dir(resolution);
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .await
                .context("Failed to create rollup directory")?;
        }
        Ok(())
    }

    /// Load existing rollup file or create new
    async fn load_or_create_file(
        &self,
        series_key: &str,
        resolution: DownsampleResolution,
    ) -> Result<RollupFile> {
        let path = self.series_file(series_key, resolution);

        if path.exists() {
            let content = fs::read_to_string(&path)
                .await
                .context("Failed to read rollup file")?;
            serde_json::from_str(&content).context("Failed to parse rollup file")
        } else {
            Ok(RollupFile::new(series_key, resolution))
        }
    }

    /// Save rollup file
    async fn save_file(&self, file: &RollupFile, resolution: DownsampleResolution) -> Result<()> {
        self.ensure_dir(resolution).await?;

        let path = self.series_file(&file.series_key, resolution);
        let content =
            serde_json::to_string_pretty(file).context("Failed to serialize rollup file")?;

        fs::write(&path, content)
            .await
            .context("Failed to write rollup file")?;

        Ok(())
    }
}

#[async_trait]
impl RollupPersistence for FileRollupPersistence {
    async fn flush_rollups(
        &self,
        series_key: &str,
        resolution: DownsampleResolution,
        aggregates: &BTreeMap<i64, RollupPoint>,
    ) -> Result<usize> {
        if aggregates.is_empty() || resolution == DownsampleResolution::Raw {
            return Ok(0);
        }

        let _guard = self.write_lock.write().await;

        // Load existing file or create new
        let mut file = self.load_or_create_file(series_key, resolution).await?;

        // Merge new points (overwrite if exists)
        let count = aggregates.len();
        for (ts, point) in aggregates {
            file.points.insert(*ts, point.clone());
        }

        // Save back
        self.save_file(&file, resolution).await?;

        debug!(
            "Flushed {} rollup points for '{}' at resolution {} to disk",
            count,
            series_key,
            resolution.name()
        );

        Ok(count)
    }

    async fn load_rollups(
        &self,
        series_key: &str,
        resolution: DownsampleResolution,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<AggregatedMetric>> {
        if resolution == DownsampleResolution::Raw {
            return Ok(Vec::new());
        }

        let path = self.series_file(series_key, resolution);
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path)
            .await
            .context("Failed to read rollup file")?;
        let file: RollupFile =
            serde_json::from_str(&content).context("Failed to parse rollup file")?;

        // Filter by time range
        let aggregates: Vec<AggregatedMetric> = file
            .points
            .range(start_ns..=end_ns)
            .map(|(ts, point)| AggregatedMetric {
                name: point.name.clone(),
                timestamp_ns: *ts,
                min: point.min,
                max: point.max,
                avg: point.avg(),
                sum: point.sum,
                count: point.count,
                labels: point.labels.clone(),
            })
            .collect();

        debug!(
            "Loaded {} rollup points for '{}' at resolution {} from disk",
            aggregates.len(),
            series_key,
            resolution.name()
        );

        Ok(aggregates)
    }

    async fn delete_before(
        &self,
        resolution: DownsampleResolution,
        cutoff_ns: i64,
    ) -> Result<usize> {
        if resolution == DownsampleResolution::Raw {
            return Ok(0);
        }

        let _guard = self.write_lock.write().await;
        let dir = self.resolution_dir(resolution);

        if !dir.exists() {
            return Ok(0);
        }

        let mut total_deleted = 0;

        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                // Load and update file
                let content = match fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let mut file: RollupFile = match serde_json::from_str(&content) {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                // Remove old points
                let to_remove: Vec<i64> = file
                    .points
                    .keys()
                    .filter(|ts| **ts < cutoff_ns)
                    .cloned()
                    .collect();

                total_deleted += to_remove.len();
                for ts in to_remove {
                    file.points.remove(&ts);
                }

                // Save or delete file
                if file.points.is_empty() {
                    fs::remove_file(&path).await.ok();
                } else {
                    let content = serde_json::to_string_pretty(&file)?;
                    fs::write(&path, content).await.ok();
                }
            }
        }

        info!(
            "Deleted {} rollup points at resolution {} before cutoff",
            total_deleted,
            resolution.name()
        );

        Ok(total_deleted)
    }
}

/// In-memory rollup persistence for testing
pub struct InMemoryRollupPersistence {
    /// Stored rollups by (resolution, series_key, timestamp)
    data: RwLock<HashMap<(DownsampleResolution, String), BTreeMap<i64, RollupPoint>>>,
}

impl InMemoryRollupPersistence {
    /// Create a new in-memory persistence layer
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryRollupPersistence {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RollupPersistence for InMemoryRollupPersistence {
    async fn flush_rollups(
        &self,
        series_key: &str,
        resolution: DownsampleResolution,
        aggregates: &BTreeMap<i64, RollupPoint>,
    ) -> Result<usize> {
        if resolution == DownsampleResolution::Raw {
            return Ok(0);
        }

        let mut data = self.data.write().await;
        let key = (resolution, series_key.to_string());
        let entry = data.entry(key).or_insert_with(BTreeMap::new);

        let count = aggregates.len();
        for (ts, point) in aggregates {
            entry.insert(*ts, point.clone());
        }

        Ok(count)
    }

    async fn load_rollups(
        &self,
        series_key: &str,
        resolution: DownsampleResolution,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<AggregatedMetric>> {
        if resolution == DownsampleResolution::Raw {
            return Ok(Vec::new());
        }

        let data = self.data.read().await;
        let key = (resolution, series_key.to_string());

        let rollups = data
            .get(&key)
            .map(|btree| {
                btree
                    .range(start_ns..=end_ns)
                    .map(|(ts, point)| AggregatedMetric {
                        name: point.name.clone(),
                        timestamp_ns: *ts,
                        min: point.min,
                        max: point.max,
                        avg: point.avg(),
                        sum: point.sum,
                        count: point.count,
                        labels: point.labels.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(rollups)
    }

    async fn delete_before(
        &self,
        resolution: DownsampleResolution,
        cutoff_ns: i64,
    ) -> Result<usize> {
        if resolution == DownsampleResolution::Raw {
            return Ok(0);
        }

        let mut data = self.data.write().await;
        let mut total_deleted = 0;

        for ((res, _), btree) in data.iter_mut() {
            if *res == resolution {
                let to_remove: Vec<i64> = btree
                    .keys()
                    .filter(|ts| **ts < cutoff_ns)
                    .cloned()
                    .collect();

                total_deleted += to_remove.len();
                for ts in to_remove {
                    btree.remove(&ts);
                }
            }
        }

        Ok(total_deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_rollup_point(name: &str, value: f64) -> RollupPoint {
        RollupPoint {
            min: value - 1.0,
            max: value + 1.0,
            sum: value * 3.0,
            count: 3,
            name: name.to_string(),
            labels: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_in_memory_persistence_flush_and_load() {
        let persistence = InMemoryRollupPersistence::new();

        let mut aggregates = BTreeMap::new();
        aggregates.insert(1000, make_rollup_point("cpu", 50.0));
        aggregates.insert(2000, make_rollup_point("cpu", 60.0));
        aggregates.insert(3000, make_rollup_point("cpu", 70.0));

        // Flush
        let count = persistence
            .flush_rollups("cpu:{}", DownsampleResolution::Minute, &aggregates)
            .await
            .unwrap();
        assert_eq!(count, 3);

        // Load
        let loaded = persistence
            .load_rollups("cpu:{}", DownsampleResolution::Minute, 0, 5000)
            .await
            .unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[tokio::test]
    async fn test_in_memory_persistence_time_range() {
        let persistence = InMemoryRollupPersistence::new();

        let mut aggregates = BTreeMap::new();
        aggregates.insert(1000, make_rollup_point("cpu", 50.0));
        aggregates.insert(2000, make_rollup_point("cpu", 60.0));
        aggregates.insert(3000, make_rollup_point("cpu", 70.0));

        persistence
            .flush_rollups("cpu:{}", DownsampleResolution::Minute, &aggregates)
            .await
            .unwrap();

        // Load partial range
        let loaded = persistence
            .load_rollups("cpu:{}", DownsampleResolution::Minute, 1500, 2500)
            .await
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].timestamp_ns, 2000);
    }

    #[tokio::test]
    async fn test_in_memory_persistence_delete_before() {
        let persistence = InMemoryRollupPersistence::new();

        let mut aggregates = BTreeMap::new();
        aggregates.insert(1000, make_rollup_point("cpu", 50.0));
        aggregates.insert(2000, make_rollup_point("cpu", 60.0));
        aggregates.insert(3000, make_rollup_point("cpu", 70.0));

        persistence
            .flush_rollups("cpu:{}", DownsampleResolution::Minute, &aggregates)
            .await
            .unwrap();

        // Delete before 2500
        let deleted = persistence
            .delete_before(DownsampleResolution::Minute, 2500)
            .await
            .unwrap();
        assert_eq!(deleted, 2);

        // Only one should remain
        let loaded = persistence
            .load_rollups("cpu:{}", DownsampleResolution::Minute, 0, 5000)
            .await
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].timestamp_ns, 3000);
    }

    #[tokio::test]
    async fn test_raw_resolution_skipped() {
        let persistence = InMemoryRollupPersistence::new();

        let mut aggregates = BTreeMap::new();
        aggregates.insert(1000, make_rollup_point("cpu", 50.0));

        // Raw resolution should be skipped
        let count = persistence
            .flush_rollups("cpu:{}", DownsampleResolution::Raw, &aggregates)
            .await
            .unwrap();
        assert_eq!(count, 0);

        let loaded = persistence
            .load_rollups("cpu:{}", DownsampleResolution::Raw, 0, 5000)
            .await
            .unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_resolutions() {
        let persistence = InMemoryRollupPersistence::new();

        let mut minute_aggregates = BTreeMap::new();
        minute_aggregates.insert(60000000000, make_rollup_point("cpu", 50.0));

        let mut hour_aggregates = BTreeMap::new();
        hour_aggregates.insert(3600000000000, make_rollup_point("cpu", 55.0));

        persistence
            .flush_rollups("cpu:{}", DownsampleResolution::Minute, &minute_aggregates)
            .await
            .unwrap();
        persistence
            .flush_rollups("cpu:{}", DownsampleResolution::Hour, &hour_aggregates)
            .await
            .unwrap();

        // Load minute
        let minute_loaded = persistence
            .load_rollups("cpu:{}", DownsampleResolution::Minute, 0, 100000000000)
            .await
            .unwrap();
        assert_eq!(minute_loaded.len(), 1);

        // Load hour
        let hour_loaded = persistence
            .load_rollups("cpu:{}", DownsampleResolution::Hour, 0, 5000000000000)
            .await
            .unwrap();
        assert_eq!(hour_loaded.len(), 1);
    }

    #[tokio::test]
    async fn test_file_persistence_flush_and_load() {
        let dir = tempdir().unwrap();
        let persistence = FileRollupPersistence::new(dir.path().to_str().unwrap());

        let mut aggregates = BTreeMap::new();
        aggregates.insert(60000000000, make_rollup_point("cpu", 50.0));
        aggregates.insert(120000000000, make_rollup_point("cpu", 60.0));

        // Flush
        let count = persistence
            .flush_rollups(
                "cpu:{host=server1}",
                DownsampleResolution::Minute,
                &aggregates,
            )
            .await
            .unwrap();
        assert_eq!(count, 2);

        // Load
        let loaded = persistence
            .load_rollups(
                "cpu:{host=server1}",
                DownsampleResolution::Minute,
                0,
                200000000000,
            )
            .await
            .unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "cpu");
    }

    #[tokio::test]
    async fn test_file_persistence_delete_before() {
        let dir = tempdir().unwrap();
        let persistence = FileRollupPersistence::new(dir.path().to_str().unwrap());

        let mut aggregates = BTreeMap::new();
        aggregates.insert(1000, make_rollup_point("cpu", 50.0));
        aggregates.insert(2000, make_rollup_point("cpu", 60.0));
        aggregates.insert(3000, make_rollup_point("cpu", 70.0));

        persistence
            .flush_rollups("cpu:{}", DownsampleResolution::Minute, &aggregates)
            .await
            .unwrap();

        // Delete before 2500
        let deleted = persistence
            .delete_before(DownsampleResolution::Minute, 2500)
            .await
            .unwrap();
        assert_eq!(deleted, 2);

        // Only one should remain
        let loaded = persistence
            .load_rollups("cpu:{}", DownsampleResolution::Minute, 0, 5000)
            .await
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].timestamp_ns, 3000);
    }

    #[test]
    fn test_rollup_point_avg() {
        let point = RollupPoint {
            min: 10.0,
            max: 20.0,
            sum: 45.0,
            count: 3,
            name: "test".to_string(),
            labels: HashMap::new(),
        };
        assert!((point.avg() - 15.0).abs() < 0.001);
    }

    #[test]
    fn test_rollup_point_avg_zero_count() {
        let point = RollupPoint {
            min: 0.0,
            max: 0.0,
            sum: 0.0,
            count: 0,
            name: "test".to_string(),
            labels: HashMap::new(),
        };
        assert!((point.avg() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_sanitize_series_key() {
        let key = "cpu:{host=server1,env=prod}";
        let sanitized = FileRollupPersistence::sanitize_series_key(key);
        assert!(!sanitized.contains(':'));
        assert!(!sanitized.contains('{'));
        assert!(!sanitized.contains('}'));
    }
}
