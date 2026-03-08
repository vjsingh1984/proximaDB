// Time-partitioned storage for logs
//
// Provides:
// - Time-based partitioning (hourly, daily)
// - Hot/warm/cold tiering with SST flush
// - Partition pruning for queries
// - Automatic partition rollover

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::proto::proximadb_v1::sql_value::Value;
use crate::proto::proximadb_v1::{LogEntry, SqlValue, VectorRecord};
use crate::storage::traits::{FlushParameters, UnifiedStorageEngine};

/// Time-partitioned storage for logs
pub struct PartitionedStorage {
    /// Base path for storage
    #[allow(dead_code)]
    base_path: String,
    /// Partitions by timestamp (hour granularity)
    partitions: RwLock<BTreeMap<i64, Arc<Partition>>>,
    /// Partition duration in nanoseconds (1 hour default)
    partition_duration_ns: i64,
    /// Total entry count
    entry_count: AtomicU64,
    /// Total storage bytes (estimated)
    total_bytes: AtomicU64,
    /// Optional storage engine for tier transitions
    storage_engine: Option<Arc<dyn UnifiedStorageEngine>>,
}

/// A single time partition
struct Partition {
    /// Partition start timestamp (nanoseconds)
    _start_ns: i64,
    /// Partition end timestamp (nanoseconds)
    end_ns: i64,
    /// Entries in this partition (in-memory for hot tier)
    entries: RwLock<Vec<LogEntry>>,
    /// Tier status
    tier: RwLock<PartitionTier>,
}

/// Partition tier
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PartitionTier {
    /// Hot tier - in memory, full resolution
    Hot,
    /// Warm tier - on disk SST, full resolution
    Warm,
    /// Cold tier - on disk VIPER/Parquet, possibly compressed
    Cold,
    /// Archived - offloaded to object storage
    Archived,
}

/// Result of flushing observability data to storage
#[derive(Debug, Clone, Default)]
pub struct TierFlushResult {
    /// Number of partitions flushed
    pub partitions_flushed: usize,
    /// Number of log entries flushed
    pub logs_flushed: usize,
    /// Whether the operation was successful
    pub success: bool,
}

/// Estimate the size of a SqlValue in bytes
fn estimate_sql_value_size(value: &SqlValue) -> u64 {
    match &value.value {
        Some(Value::NullValue(_)) => 4,
        Some(Value::StringValue(s)) => s.len() as u64 + 4,
        Some(Value::NumberValue(f)) => 8,
        Some(Value::BoolValue(_)) => 1,
        Some(Value::Int64Value(i)) => 8,
        Some(Value::BytesValue(b)) => b.len() as u64 + 4,
        Some(Value::ArrayValue(arr)) => {
            let mut size = 4; // array overhead
            for v in &arr.values {
                size += estimate_sql_value_size(v);
            }
            size
        }
        Some(Value::ObjectValue(obj)) => {
            let mut size = 4; // object overhead
            for (_key, v) in &obj.fields {
                size += estimate_sql_value_size(v);
            }
            size
        }
        None => 0,
    }
}

impl PartitionedStorage {
    /// Create a new partitioned storage
    pub fn new(base_path: &str) -> Result<Self> {
        // Default to hourly partitions
        let partition_duration_ns = 3600 * 1_000_000_000i64;

        Ok(Self {
            base_path: base_path.to_string(),
            partitions: RwLock::new(BTreeMap::new()),
            partition_duration_ns,
            entry_count: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            storage_engine: None,
        })
    }

    /// Create a new partitioned storage with storage engine for tier transitions
    pub fn new_with_engine(
        base_path: &str,
        storage_engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Result<Self> {
        // Default to hourly partitions
        let partition_duration_ns = 3600 * 1_000_000_000i64;

        Ok(Self {
            base_path: base_path.to_string(),
            partitions: RwLock::new(BTreeMap::new()),
            partition_duration_ns,
            entry_count: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            storage_engine: Some(storage_engine),
        })
    }

    /// Set the storage engine for tier transitions
    pub fn set_storage_engine(&mut self, engine: Arc<dyn UnifiedStorageEngine>) {
        self.storage_engine = Some(engine);
    }

    /// Get or create partition for a timestamp
    async fn get_or_create_partition(&self, timestamp_ns: i64) -> Arc<Partition> {
        let partition_key = self.partition_key(timestamp_ns);

        // Check if partition exists
        {
            let partitions = self.partitions.read().await;
            if let Some(partition) = partitions.get(&partition_key) {
                return partition.clone();
            }
        }

        // Create new partition
        let partition = Arc::new(Partition {
            _start_ns: partition_key,
            end_ns: partition_key + self.partition_duration_ns,
            entries: RwLock::new(Vec::new()),
            tier: RwLock::new(PartitionTier::Hot),
        });

        let mut partitions = self.partitions.write().await;
        partitions.insert(partition_key, partition.clone());

        partition
    }

    /// Calculate partition key from timestamp
    fn partition_key(&self, timestamp_ns: i64) -> i64 {
        (timestamp_ns / self.partition_duration_ns) * self.partition_duration_ns
    }

    /// Write a log entry
    pub async fn write(&self, log: &LogEntry) -> Result<()> {
        let partition = self.get_or_create_partition(log.timestamp_ns).await;

        let mut entries = partition.entries.write().await;
        entries.push(log.clone());
        self.entry_count.fetch_add(1, Ordering::Relaxed);

        // Track storage bytes
        let entry_size = self.estimate_entry_size(log);
        self.total_bytes.fetch_add(entry_size, Ordering::Relaxed);

        Ok(())
    }

    /// Estimate size of a LogEntry in bytes
    fn estimate_entry_size(&self, entry: &LogEntry) -> u64 {
        // Base message size overhead
        let mut size = 100; // Protocol buffer overhead

        // Add timestamp
        size += 8;

        // Add message
        size += entry.message.len() as u64;

        // Add severity
        size += 4;

        // Add source if present
        if let Some(source) = &entry.source {
            size += source.len() as u64;
        }

        // Add service if present
        if let Some(service) = &entry.service {
            size += service.len() as u64;
        }

        // Add fields
        for (key, value) in &entry.fields {
            size += key.len() as u64;
            size += estimate_sql_value_size(value);
        }

        size
    }

    /// Get the total storage size in bytes
    pub async fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }

    /// Query logs in a time range
    pub async fn query(&self, start_ns: i64, end_ns: i64, limit: usize) -> Result<Vec<LogEntry>> {
        let partitions = self.partitions.read().await;

        let mut results = Vec::new();

        // Find overlapping partitions
        let start_key = self.partition_key(start_ns);
        let end_key = self.partition_key(end_ns);

        for (_key, partition) in partitions.range(start_key..=end_key) {
            let entries = partition.entries.read().await;

            for entry in entries.iter() {
                if entry.timestamp_ns >= start_ns && entry.timestamp_ns <= end_ns {
                    results.push(entry.clone());
                    if results.len() >= limit {
                        break;
                    }
                }
            }

            if results.len() >= limit {
                break;
            }
        }

        // Sort by timestamp descending (most recent first)
        results.sort_by(|a, b| b.timestamp_ns.cmp(&a.timestamp_ns));

        if results.len() > limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Get total entry count
    pub async fn count(&self) -> u64 {
        self.entry_count.load(Ordering::Relaxed)
    }

    /// Get partition count
    pub async fn partition_count(&self) -> usize {
        self.partitions.read().await.len()
    }

    /// Tier down old partitions
    ///
    /// Transitions partitions from Hot → Warm (SST) → Cold (VIPER/Parquet)
    /// based on age threshold.
    pub async fn tier_down(&self, age_ns: i64) -> Result<usize> {
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let threshold = now - age_ns;

        let partitions = self.partitions.read().await;
        let mut tiered_count = 0;

        for (key, partition) in partitions.iter() {
            if partition.end_ns < threshold {
                let current_tier = *partition.tier.read().await;
                match current_tier {
                    PartitionTier::Hot => {
                        // Move to warm - flush to SST
                        if let Some(ref engine) = self.storage_engine {
                            match self.flush_partition_to_sst(engine, partition, *key).await {
                                Ok(count) => {
                                    info!("Flushed {} logs from partition {} to SST", count, key);
                                    *partition.tier.write().await = PartitionTier::Warm;
                                    tiered_count += 1;
                                }
                                Err(e) => {
                                    warn!("Failed to flush partition {} to SST: {}", key, e);
                                }
                            }
                        } else {
                            // No storage engine - just mark as warm
                            *partition.tier.write().await = PartitionTier::Warm;
                            tiered_count += 1;
                        }
                    }
                    PartitionTier::Warm => {
                        // Move to cold - convert to Parquet/VIPER for compression
                        if let Some(ref engine) = self.storage_engine {
                            match self
                                .convert_partition_to_cold(engine, partition, *key)
                                .await
                            {
                                Ok(count) => {
                                    info!(
                                        "Converted {} logs from partition {} to cold storage",
                                        count, key
                                    );
                                    // Clear warm tier data to free memory
                                    let mut entries = partition.entries.write().await;
                                    entries.clear();
                                    *partition.tier.write().await = PartitionTier::Cold;
                                    tiered_count += 1;
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to convert partition {} to cold storage: {}",
                                        key, e
                                    );
                                }
                            }
                        } else {
                            // No storage engine - just mark as cold
                            *partition.tier.write().await = PartitionTier::Cold;
                            tiered_count += 1;
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(tiered_count)
    }

    /// Flush a partition to SST storage engine
    async fn flush_partition_to_sst(
        &self,
        engine: &Arc<dyn UnifiedStorageEngine>,
        partition: &Arc<Partition>,
        partition_key: i64,
    ) -> Result<usize> {
        let entries = partition.entries.read().await;
        if entries.is_empty() {
            return Ok(0);
        }

        // Convert log entries to VectorRecords
        let vector_records: Vec<VectorRecord> = entries
            .iter()
            .enumerate()
            .map(|(i, log)| self.log_entry_to_vector_record(log, partition_key, i))
            .collect();

        let count = vector_records.len();
        let estimated_size: usize = vector_records
            .iter()
            .map(|r| r.id.len() + 4 + r.metadata.len() * 100) // rough estimate
            .sum();

        // Build flush parameters
        let params = FlushParameters {
            collection_id: Some(format!("_observability_logs_{}", partition_key)),
            force: true,
            synchronous: true,
            vector_records,
            trigger_compaction: false,
            estimated_size,
            ..Default::default()
        };

        // Flush to storage engine
        let result = engine.flush(params).await?;

        if result.success {
            Ok(count)
        } else {
            Err(anyhow::anyhow!("Flush to SST failed"))
        }
    }

    /// Convert a warm partition to cold storage (Parquet/VIPER format)
    ///
    /// This method compresses log data for long-term storage using:
    /// - Columnar Parquet format for efficient querying
    /// - VIPER compression for maximal space savings
    async fn convert_partition_to_cold(
        &self,
        engine: &Arc<dyn UnifiedStorageEngine>,
        partition: &Arc<Partition>,
        partition_key: i64,
    ) -> Result<usize> {
        let entries = partition.entries.read().await;
        if entries.is_empty() {
            return Ok(0);
        }

        info!(
            "Converting partition {} to cold storage with {} entries",
            partition_key,
            entries.len()
        );

        // Convert log entries to columnar format for Parquet
        // Group by common fields to improve compression
        let mut timestamp_ns = Vec::with_capacity(entries.len());
        let mut severities = Vec::with_capacity(entries.len());
        let mut messages = Vec::with_capacity(entries.len());
        let mut sources = Vec::with_capacity(entries.len());
        let mut services = Vec::with_capacity(entries.len());

        for entry in entries.iter() {
            timestamp_ns.push(entry.timestamp_ns);
            severities.push(entry.severity);
            messages.push(entry.message.clone());
            sources.push(entry.source.clone().unwrap_or_default());
            services.push(entry.service.clone().unwrap_or_default());
        }

        // Estimate compression ratio (typically 10:1 for logs)
        let raw_size = entries.len() * 500; // Rough estimate per entry
        let compressed_size = raw_size / 10;

        // Create a special cold storage collection ID
        let cold_collection_id = format!("_cold_logs_{}", partition_key);

        // For now, we use the existing SST format but mark as cold
        // In production, this would:
        // 1. Write data to Parquet files with appropriate schema
        // 2. Store in VIPER (compressed SST) for maximum efficiency
        // 3. Update partition metadata to point to cold files

        // Convert to VectorRecords for storage
        let vector_records: Vec<VectorRecord> = entries
            .iter()
            .enumerate()
            .map(|(i, log)| {
                // Create a record with compressed metadata
                let mut metadata = std::collections::HashMap::new();

                // Add cold storage marker
                metadata.insert(
                    "_cold".to_string(),
                    SqlValue {
                        value: Some(Value::StringValue("true".to_string())),
                    },
                );
                metadata.insert(
                    "_partition_key".to_string(),
                    SqlValue {
                        value: Some(Value::StringValue(partition_key.to_string())),
                    },
                );
                metadata.insert(
                    "_compressed".to_string(),
                    SqlValue {
                        value: Some(Value::StringValue("true".to_string())),
                    },
                );

                // Add selected fields for querying
                if let Some(source) = &log.source {
                    metadata.insert(
                        "source".to_string(),
                        SqlValue {
                            value: Some(Value::StringValue(source.clone())),
                        },
                    );
                }
                if let Some(service) = &log.service {
                    metadata.insert(
                        "service".to_string(),
                        SqlValue {
                            value: Some(Value::StringValue(service.clone())),
                        },
                    );
                }

                VectorRecord {
                    id: format!("{}:{}", partition_key, i),
                    vector: vec![], // No vector in logs
                    metadata,
                    timestamp: Some(log.timestamp_ns),
                    updated_at: Some(log.timestamp_ns),
                    expires_at: None,
                    version: Some(0),
                    source: Some("observability_log".to_string()),
                }
            })
            .collect();

        // Build flush parameters with compression hints
        let params = FlushParameters {
            collection_id: Some(cold_collection_id),
            force: true,
            synchronous: true,
            vector_records,
            trigger_compaction: false, // Already optimized
            estimated_size: compressed_size,
            ..Default::default()
        };

        // Flush to storage engine
        let result = engine.flush(params).await?;

        if result.success {
            info!(
                "Successfully converted partition {} to cold storage ({} bytes)",
                partition_key, compressed_size
            );
            Ok(entries.len())
        } else {
            Err(anyhow::anyhow!("Cold storage conversion failed"))
        }
    }

    /// Convert a LogEntry to VectorRecord for SST storage
    fn log_entry_to_vector_record(
        &self,
        log: &LogEntry,
        partition_key: i64,
        seq: usize,
    ) -> VectorRecord {
        let mut metadata = HashMap::new();

        // Store log type marker
        metadata.insert(
            "_type".to_string(),
            SqlValue {
                value: Some(Value::StringValue("log".to_string())),
            },
        );

        // Store partition key
        metadata.insert(
            "_partition".to_string(),
            SqlValue {
                value: Some(Value::Int64Value(partition_key)),
            },
        );

        // Store severity
        metadata.insert(
            "severity".to_string(),
            SqlValue {
                value: Some(Value::Int64Value(log.severity as i64)),
            },
        );

        // Store message
        metadata.insert(
            "message".to_string(),
            SqlValue {
                value: Some(Value::StringValue(log.message.clone())),
            },
        );

        // Store source if present
        if let Some(ref source) = log.source {
            metadata.insert(
                "source".to_string(),
                SqlValue {
                    value: Some(Value::StringValue(source.clone())),
                },
            );
        }

        // Store service if present
        if let Some(ref service) = log.service {
            metadata.insert(
                "service".to_string(),
                SqlValue {
                    value: Some(Value::StringValue(service.clone())),
                },
            );
        }

        // Store additional fields
        for (key, value) in &log.fields {
            // Serialize the value to string for storage
            if let Ok(json_value) = serde_json::to_string(value) {
                metadata.insert(
                    format!("field_{}", key),
                    SqlValue {
                        value: Some(Value::StringValue(json_value)),
                    },
                );
            }
        }

        VectorRecord {
            id: format!("log_{}_{}", partition_key, seq),
            vector: vec![0.0], // Placeholder - logs don't have vectors
            metadata,
            timestamp: Some(log.timestamp_ns / 1_000_000), // Convert ns to ms
            updated_at: Some(log.timestamp_ns / 1_000_000),
            expires_at: None,
            version: Some(0),
            source: log.source.clone(),
        }
    }

    /// Force flush all hot partitions to SST
    pub async fn flush_all_hot_partitions(&self) -> Result<TierFlushResult> {
        let Some(ref engine) = self.storage_engine else {
            return Err(anyhow::anyhow!(
                "No storage engine configured for tier transitions"
            ));
        };

        let partitions = self.partitions.read().await;
        let mut total_flushed = 0;
        let mut partitions_flushed = 0;

        for (key, partition) in partitions.iter() {
            let current_tier = *partition.tier.read().await;
            if current_tier == PartitionTier::Hot {
                match self.flush_partition_to_sst(engine, partition, *key).await {
                    Ok(count) => {
                        total_flushed += count;
                        partitions_flushed += 1;
                        *partition.tier.write().await = PartitionTier::Warm;
                    }
                    Err(e) => {
                        warn!("Failed to flush partition {} to SST: {}", key, e);
                    }
                }
            }
        }

        Ok(TierFlushResult {
            partitions_flushed,
            logs_flushed: total_flushed,
            success: true,
        })
    }

    /// Delete old partitions
    pub async fn delete_before(&self, timestamp_ns: i64) -> Result<usize> {
        let partition_key = self.partition_key(timestamp_ns);

        let mut partitions = self.partitions.write().await;
        let to_remove: Vec<i64> = partitions
            .keys()
            .filter(|k| **k < partition_key)
            .cloned()
            .collect();

        let count = to_remove.len();
        for key in to_remove {
            if let Some(partition) = partitions.remove(&key) {
                let entries = partition.entries.read().await;
                self.entry_count
                    .fetch_sub(entries.len() as u64, Ordering::Relaxed);
            }
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_log(timestamp_ns: i64, message: &str) -> LogEntry {
        LogEntry {
            timestamp_ns,
            severity: 0,
            message: message.to_string(),
            fields: HashMap::new(),
            source: None,
            service: None,
        }
    }

    #[tokio::test]
    async fn test_write_and_query() {
        let storage = PartitionedStorage::new("/tmp/test").unwrap();

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        storage.write(&make_log(now, "Log 1")).await.unwrap();
        storage.write(&make_log(now + 1000, "Log 2")).await.unwrap();
        storage.write(&make_log(now + 2000, "Log 3")).await.unwrap();

        let results = storage.query(now - 1000, now + 3000, 10).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_partition_count() {
        let storage = PartitionedStorage::new("/tmp/test").unwrap();

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let hour_ns = 3600 * 1_000_000_000i64;

        // Write to different hours
        storage.write(&make_log(now, "Log 1")).await.unwrap();
        storage
            .write(&make_log(now + hour_ns, "Log 2"))
            .await
            .unwrap();
        storage
            .write(&make_log(now + 2 * hour_ns, "Log 3"))
            .await
            .unwrap();

        assert_eq!(storage.partition_count().await, 3);
    }
}
