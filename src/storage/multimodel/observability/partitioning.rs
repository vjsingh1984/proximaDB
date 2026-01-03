//! # Time-Based Partitioning
//!
//! Manages time-based partitions for logs and metrics.
//! Enables efficient range queries and automatic retention.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Partition granularity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionGranularity {
    /// Hourly partitions (for high-volume logs)
    Hourly,
    /// Daily partitions (default for most use cases)
    Daily,
    /// Weekly partitions (for lower volume data)
    Weekly,
    /// Monthly partitions (for archival)
    Monthly,
}

impl PartitionGranularity {
    /// Get duration in seconds
    pub fn duration_secs(&self) -> i64 {
        match self {
            PartitionGranularity::Hourly => 3600,
            PartitionGranularity::Daily => 86400,
            PartitionGranularity::Weekly => 604800,
            PartitionGranularity::Monthly => 2592000, // 30 days
        }
    }

    /// Get partition name suffix format
    pub fn format_suffix(&self, timestamp_secs: i64) -> String {
        use chrono::{DateTime, Utc};

        let dt = DateTime::<Utc>::from(UNIX_EPOCH + Duration::from_secs(timestamp_secs as u64));

        match self {
            PartitionGranularity::Hourly => dt.format("%Y%m%d_%H").to_string(),
            PartitionGranularity::Daily => dt.format("%Y%m%d").to_string(),
            PartitionGranularity::Weekly => dt.format("%Y_W%W").to_string(),
            PartitionGranularity::Monthly => dt.format("%Y%m").to_string(),
        }
    }
}

/// Configuration for time partitioning
#[derive(Debug, Clone)]
pub struct PartitionConfig {
    /// Partition granularity
    pub granularity: PartitionGranularity,
    /// Retention period in seconds
    pub retention_secs: i64,
    /// Maximum number of partitions to keep
    pub max_partitions: usize,
    /// Pre-create partitions ahead of time
    pub pre_create_count: usize,
    /// Enable automatic cleanup
    pub auto_cleanup: bool,
}

impl Default for PartitionConfig {
    fn default() -> Self {
        Self {
            granularity: PartitionGranularity::Daily,
            retention_secs: 7 * 86400, // 7 days
            max_partitions: 30,
            pre_create_count: 3,
            auto_cleanup: true,
        }
    }
}

/// Time range for a partition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionRange {
    /// Start timestamp (inclusive) in nanoseconds
    pub start_ns: i64,
    /// End timestamp (exclusive) in nanoseconds
    pub end_ns: i64,
}

impl PartitionRange {
    /// Create a new partition range
    pub fn new(start_ns: i64, end_ns: i64) -> Self {
        Self { start_ns, end_ns }
    }

    /// Check if timestamp is within range
    pub fn contains(&self, timestamp_ns: i64) -> bool {
        timestamp_ns >= self.start_ns && timestamp_ns < self.end_ns
    }

    /// Check if this range overlaps with another
    pub fn overlaps(&self, other: &PartitionRange) -> bool {
        self.start_ns < other.end_ns && other.start_ns < self.end_ns
    }

    /// Duration in nanoseconds
    pub fn duration_ns(&self) -> i64 {
        self.end_ns - self.start_ns
    }
}

/// A single partition
#[derive(Debug, Clone)]
pub struct Partition {
    /// Partition name/ID
    pub name: String,
    /// Time range
    pub range: PartitionRange,
    /// Number of records in this partition
    pub record_count: u64,
    /// Size in bytes
    pub size_bytes: u64,
    /// Is partition compressed
    pub is_compressed: bool,
    /// Storage path
    pub path: String,
    /// Creation timestamp
    pub created_at_secs: i64,
}

impl Partition {
    /// Create a new partition
    pub fn new(name: String, range: PartitionRange, path: String) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        Self {
            name,
            range,
            record_count: 0,
            size_bytes: 0,
            is_compressed: false,
            path,
            created_at_secs: now,
        }
    }

    /// Check if partition is expired
    pub fn is_expired(&self, retention_secs: i64) -> bool {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Partition end time in seconds
        let end_secs = self.range.end_ns / 1_000_000_000;

        now_secs - end_secs > retention_secs
    }
}

/// Time partitioner manages partitions for a namespace
pub struct TimePartitioner {
    /// Namespace name
    namespace: String,
    /// Configuration
    config: PartitionConfig,
    /// Active partitions indexed by start time
    partitions: RwLock<BTreeMap<i64, Partition>>,
    /// Total record count
    total_records: std::sync::atomic::AtomicU64,
}

impl TimePartitioner {
    /// Create a new time partitioner
    pub fn new(namespace: String, config: PartitionConfig) -> Self {
        Self {
            namespace,
            config,
            partitions: RwLock::new(BTreeMap::new()),
            total_records: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Get or create partition for a timestamp
    pub async fn get_or_create_partition(&self, timestamp_ns: i64) -> Result<Partition> {
        // Calculate partition start time
        let granularity_ns = self.config.granularity.duration_secs() * 1_000_000_000;
        let partition_start_ns = (timestamp_ns / granularity_ns) * granularity_ns;
        let partition_end_ns = partition_start_ns + granularity_ns;

        // Check if partition exists
        {
            let partitions = self.partitions.read().await;
            if let Some(partition) = partitions.get(&partition_start_ns) {
                return Ok(partition.clone());
            }
        }

        // Create new partition
        let range = PartitionRange::new(partition_start_ns, partition_end_ns);
        let suffix = self.config.granularity.format_suffix(partition_start_ns / 1_000_000_000);
        let name = format!("{}_{}", self.namespace, suffix);
        let path = format!("/data/{}/{}", self.namespace, suffix);

        let partition = Partition::new(name.clone(), range, path);

        // Insert partition
        {
            let mut partitions = self.partitions.write().await;

            // Check max partitions limit
            if partitions.len() >= self.config.max_partitions {
                // Remove oldest partition
                if let Some(&oldest_key) = partitions.keys().next() {
                    partitions.remove(&oldest_key);
                    debug!("Removed oldest partition for namespace {}", self.namespace);
                }
            }

            partitions.insert(partition_start_ns, partition.clone());
        }

        info!("Created partition {} for namespace {}", name, self.namespace);
        Ok(partition)
    }

    /// Find partitions that overlap with a time range
    pub async fn find_partitions(&self, start_ns: i64, end_ns: i64) -> Vec<Partition> {
        let query_range = PartitionRange::new(start_ns, end_ns);
        let partitions = self.partitions.read().await;

        partitions
            .values()
            .filter(|p| p.range.overlaps(&query_range))
            .cloned()
            .collect()
    }

    /// Get all active partitions
    pub async fn list_partitions(&self) -> Vec<Partition> {
        let partitions = self.partitions.read().await;
        partitions.values().cloned().collect()
    }

    /// Run cleanup to remove expired partitions
    pub async fn cleanup_expired(&self) -> Result<usize> {
        if !self.config.auto_cleanup {
            return Ok(0);
        }

        let mut partitions = self.partitions.write().await;
        let initial_count = partitions.len();

        partitions.retain(|_, p| !p.is_expired(self.config.retention_secs));

        let removed = initial_count - partitions.len();
        if removed > 0 {
            info!(
                "Cleaned up {} expired partitions for namespace {}",
                removed, self.namespace
            );
        }

        Ok(removed)
    }

    /// Update partition statistics
    pub async fn update_partition_stats(
        &self,
        partition_start_ns: i64,
        added_records: u64,
        added_bytes: u64,
    ) -> Result<()> {
        let mut partitions = self.partitions.write().await;

        if let Some(partition) = partitions.get_mut(&partition_start_ns) {
            partition.record_count += added_records;
            partition.size_bytes += added_bytes;
            self.total_records.fetch_add(added_records, std::sync::atomic::Ordering::Relaxed);
        }

        Ok(())
    }

    /// Get total record count
    pub fn total_records(&self) -> u64 {
        self.total_records.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get configuration
    pub fn config(&self) -> &PartitionConfig {
        &self.config
    }

    /// Get namespace name
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_config_default() {
        let config = PartitionConfig::default();
        assert_eq!(config.granularity, PartitionGranularity::Daily);
        assert!(config.auto_cleanup);
    }

    #[test]
    fn test_partition_range_contains() {
        let range = PartitionRange::new(1000, 2000);
        assert!(range.contains(1000));
        assert!(range.contains(1500));
        assert!(!range.contains(999));
        assert!(!range.contains(2000));
    }

    #[test]
    fn test_partition_range_overlaps() {
        let range1 = PartitionRange::new(1000, 2000);
        let range2 = PartitionRange::new(1500, 2500);
        let range3 = PartitionRange::new(2000, 3000);

        assert!(range1.overlaps(&range2));
        assert!(!range1.overlaps(&range3)); // Adjacent but not overlapping
    }

    #[test]
    fn test_granularity_format() {
        let timestamp_secs = 1704067200; // 2024-01-01 00:00:00 UTC

        assert_eq!(
            PartitionGranularity::Hourly.format_suffix(timestamp_secs),
            "20240101_00"
        );
        assert_eq!(
            PartitionGranularity::Daily.format_suffix(timestamp_secs),
            "20240101"
        );
        assert_eq!(
            PartitionGranularity::Monthly.format_suffix(timestamp_secs),
            "202401"
        );
    }

    #[tokio::test]
    async fn test_partitioner_create() {
        let config = PartitionConfig {
            granularity: PartitionGranularity::Daily,
            ..Default::default()
        };

        let partitioner = TimePartitioner::new("logs".to_string(), config);

        // Create a partition for today
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;

        let partition = partitioner.get_or_create_partition(now_ns).await.unwrap();

        assert!(partition.name.starts_with("logs_"));
        assert!(partition.range.contains(now_ns));
    }

    #[tokio::test]
    async fn test_find_partitions() {
        let config = PartitionConfig {
            granularity: PartitionGranularity::Daily,
            ..Default::default()
        };

        let partitioner = TimePartitioner::new("metrics".to_string(), config);

        // Create partitions for 3 days
        let day_ns = 86400_000_000_000i64;
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;

        for i in 0..3 {
            partitioner.get_or_create_partition(now_ns - i * day_ns).await.unwrap();
        }

        // Query spanning 2 days
        let partitions = partitioner.find_partitions(now_ns - day_ns, now_ns).await;
        assert!(partitions.len() >= 2);
    }

    #[tokio::test]
    async fn test_max_partitions_limit() {
        let config = PartitionConfig {
            granularity: PartitionGranularity::Daily,
            max_partitions: 3,
            ..Default::default()
        };

        let partitioner = TimePartitioner::new("test".to_string(), config);

        // Create more partitions than the limit
        let day_ns = 86400_000_000_000i64;
        let now_ns = 1704067200_000_000_000i64; // Fixed timestamp

        for i in 0..5 {
            partitioner.get_or_create_partition(now_ns + i * day_ns).await.unwrap();
        }

        let partitions = partitioner.list_partitions().await;
        assert_eq!(partitions.len(), 3);
    }
}
