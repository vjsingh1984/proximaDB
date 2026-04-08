//! # TST WAL Recovery Module
//!
//! Provides Write-Ahead Log (WAL) integration for the Time-Series Storage Engine (TST).
//! This module enables crash recovery by replaying WAL records to rebuild in-memory
//! partitions after a restart.
//!
//! ## Design
//!
//! The TST engine uses the unified WAL infrastructure (`UnifiedWALWriter`/`UnifiedWALReader`)
//! with `TimeSeriesOperation` variants for time-series-specific operations. On recovery,
//! WAL entries are replayed in sequence order to reconstruct partition state.

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::persistence::write_ahead_log::unified_operations::{
    TimeSeriesOperation, UnifiedWALOperation, UnifiedWALReader, UnifiedWALWriter,
};

use super::TimeSeriesEngine;

/// WAL recovery handler for the TST engine.
///
/// Reads WAL entries from disk and replays time-series operations
/// to rebuild in-memory partitions after a crash or restart.
pub struct TstWalRecovery {
    /// Path to the WAL directory for this TST engine instance
    wal_path: PathBuf,
}

/// Statistics from a WAL recovery operation
#[derive(Debug, Clone, Default)]
pub struct TstRecoveryStats {
    /// Total WAL entries read from disk
    pub entries_read: usize,
    /// Time-series entries successfully replayed
    pub entries_replayed: usize,
    /// Entries skipped (non-time-series or invalid)
    pub entries_skipped: usize,
    /// Partitions created during recovery
    pub partitions_created: usize,
    /// OHLC bars recovered
    pub ohlc_bars_recovered: usize,
    /// Records recovered
    pub records_recovered: usize,
}

impl TstWalRecovery {
    /// Create a new TST WAL recovery handler
    pub fn new(wal_path: PathBuf) -> Self {
        Self { wal_path }
    }

    /// Recover TST engine state from WAL entries.
    ///
    /// Reads all WAL segments and replays `TimeSeriesOperation` entries
    /// to rebuild the engine's in-memory partitions.
    pub async fn recover(&self, engine: &mut TimeSeriesEngine) -> Result<TstRecoveryStats> {
        let wal_path_str = self
            .wal_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid WAL path: {:?}", self.wal_path))?
            .to_string();

        info!("TST WAL recovery starting from: {}", wal_path_str);

        let reader = UnifiedWALReader::new(wal_path_str).await?;
        let entries = reader.read_all().await?;

        let mut stats = TstRecoveryStats {
            entries_read: entries.len(),
            ..Default::default()
        };

        // Sort entries by sequence number to ensure correct replay order
        let mut sorted_entries = entries;
        sorted_entries.sort_by_key(|e| e.sequence_number);

        for entry in &sorted_entries {
            if !entry.verify_checksum() {
                warn!(
                    "Checksum mismatch for WAL entry seq={}, skipping",
                    entry.sequence_number
                );
                stats.entries_skipped += 1;
                continue;
            }

            match &entry.operation {
                UnifiedWALOperation::TimeSeriesOp(ts_op) => {
                    match self.replay_timeseries_op(engine, ts_op).await {
                        Ok(()) => {
                            stats.entries_replayed += 1;
                            match ts_op {
                                TimeSeriesOperation::InsertRecord { .. } => {
                                    stats.records_recovered += 1;
                                }
                                TimeSeriesOperation::InsertOHLC { .. } => {
                                    stats.ohlc_bars_recovered += 1;
                                }
                                TimeSeriesOperation::CreatePartition { .. } => {
                                    stats.partitions_created += 1;
                                }
                                TimeSeriesOperation::DropPartition { .. } => {}
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to replay TST WAL entry seq={}: {}",
                                entry.sequence_number, e
                            );
                            stats.entries_skipped += 1;
                        }
                    }
                }
                _ => {
                    // Skip non-time-series operations
                    stats.entries_skipped += 1;
                }
            }
        }

        info!(
            "TST WAL recovery complete: {} entries read, {} replayed, {} skipped, {} records, {} OHLC bars",
            stats.entries_read,
            stats.entries_replayed,
            stats.entries_skipped,
            stats.records_recovered,
            stats.ohlc_bars_recovered,
        );

        Ok(stats)
    }

    /// Replay a single time-series WAL operation against the engine
    async fn replay_timeseries_op(
        &self,
        engine: &mut TimeSeriesEngine,
        op: &TimeSeriesOperation,
    ) -> Result<()> {
        match op {
            TimeSeriesOperation::InsertRecord {
                collection_id,
                timestamp_ms,
                record,
            } => {
                let timestamp =
                    DateTime::from_timestamp_millis(*timestamp_ms).ok_or_else(|| {
                        anyhow::anyhow!("Invalid timestamp_ms in WAL record: {}", timestamp_ms)
                    })?;
                debug!(
                    "Replaying InsertRecord: collection={}, ts={}",
                    collection_id, timestamp
                );
                engine
                    .insert_record(collection_id, timestamp, record.clone())
                    .await?;
            }
            TimeSeriesOperation::InsertOHLC {
                collection_id,
                symbol,
                timestamp_ms,
                open,
                high,
                low,
                close,
                volume,
            } => {
                let timestamp =
                    DateTime::from_timestamp_millis(*timestamp_ms).ok_or_else(|| {
                        anyhow::anyhow!("Invalid timestamp_ms in WAL OHLC record: {}", timestamp_ms)
                    })?;
                debug!(
                    "Replaying InsertOHLC: collection={}, symbol={}, ts={}",
                    collection_id, symbol, timestamp
                );
                engine
                    .insert_ohlc(
                        collection_id,
                        symbol,
                        timestamp,
                        *open,
                        *high,
                        *low,
                        *close,
                        *volume,
                    )
                    .await?;
            }
            TimeSeriesOperation::CreatePartition {
                collection_id,
                partition_key_ms,
            } => {
                let partition_key =
                    DateTime::from_timestamp_millis(*partition_key_ms).ok_or_else(|| {
                        anyhow::anyhow!(
                            "Invalid partition_key_ms in WAL record: {}",
                            partition_key_ms
                        )
                    })?;
                debug!(
                    "Replaying CreatePartition: collection={}, key={}",
                    collection_id, partition_key
                );
                // Ensure partition exists by calling get_or_create
                engine
                    .ensure_partition(collection_id, partition_key)
                    .await?;
            }
            TimeSeriesOperation::DropPartition {
                collection_id,
                partition_key_ms,
            } => {
                let partition_key =
                    DateTime::from_timestamp_millis(*partition_key_ms).ok_or_else(|| {
                        anyhow::anyhow!(
                            "Invalid partition_key_ms in WAL DropPartition: {}",
                            partition_key_ms
                        )
                    })?;
                debug!(
                    "Replaying DropPartition: collection={}, key={}",
                    collection_id, partition_key
                );
                engine.remove_partition(&partition_key);
            }
        }
        Ok(())
    }
}

/// Wrapper around `UnifiedWALWriter` for TST-specific WAL writes.
///
/// Provides convenience methods for logging time-series operations
/// to the unified WAL before they are applied to the engine.
pub struct TstWalWriter {
    writer: Arc<Mutex<UnifiedWALWriter>>,
}

impl TstWalWriter {
    /// Create a new TST WAL writer at the given path
    pub async fn new(wal_path: &Path) -> Result<Self> {
        let wal_path_str = wal_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid WAL path: {:?}", wal_path))?
            .to_string();

        let writer = UnifiedWALWriter::new(wal_path_str).await?;
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
        })
    }

    /// Log a time-series record insert to the WAL
    pub async fn log_insert_record(
        &self,
        collection_id: &str,
        timestamp: DateTime<Utc>,
        record: &VectorRecord,
    ) -> Result<u64> {
        let op = UnifiedWALOperation::TimeSeriesOp(TimeSeriesOperation::InsertRecord {
            collection_id: collection_id.to_string(),
            timestamp_ms: timestamp.timestamp_millis(),
            record: record.clone(),
        });
        let mut writer = self.writer.lock().await;
        let seq = writer.append(op).await?;
        Ok(seq)
    }

    /// Log an OHLC bar insert to the WAL
    pub async fn log_insert_ohlc(
        &self,
        collection_id: &str,
        symbol: &str,
        timestamp: DateTime<Utc>,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    ) -> Result<u64> {
        let op = UnifiedWALOperation::TimeSeriesOp(TimeSeriesOperation::InsertOHLC {
            collection_id: collection_id.to_string(),
            symbol: symbol.to_string(),
            timestamp_ms: timestamp.timestamp_millis(),
            open,
            high,
            low,
            close,
            volume,
        });
        let mut writer = self.writer.lock().await;
        let seq = writer.append(op).await?;
        Ok(seq)
    }

    /// Log a partition creation to the WAL
    pub async fn log_create_partition(
        &self,
        collection_id: &str,
        partition_key: DateTime<Utc>,
    ) -> Result<u64> {
        let op = UnifiedWALOperation::TimeSeriesOp(TimeSeriesOperation::CreatePartition {
            collection_id: collection_id.to_string(),
            partition_key_ms: partition_key.timestamp_millis(),
        });
        let mut writer = self.writer.lock().await;
        let seq = writer.append(op).await?;
        Ok(seq)
    }

    /// Log a partition drop to the WAL
    pub async fn log_drop_partition(
        &self,
        collection_id: &str,
        partition_key: DateTime<Utc>,
    ) -> Result<u64> {
        let op = UnifiedWALOperation::TimeSeriesOp(TimeSeriesOperation::DropPartition {
            collection_id: collection_id.to_string(),
            partition_key_ms: partition_key.timestamp_millis(),
        });
        let mut writer = self.writer.lock().await;
        let seq = writer.append(op).await?;
        Ok(seq)
    }

    /// Flush all pending WAL writes to disk
    pub async fn flush(&self) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::tst::TimeSeriesConfig;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_tst_wal_writer_and_recovery() {
        let temp_dir = TempDir::new().unwrap();
        let wal_dir = temp_dir.path().join("wal");
        std::fs::create_dir_all(&wal_dir).unwrap();
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let ts = DateTime::parse_from_rfc3339("2024-06-15T10:30:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Phase 1: Write WAL entries
        {
            let writer = TstWalWriter::new(&wal_dir).await.unwrap();

            // Log a record insert
            let record = VectorRecord {
                id: "ts_rec_1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: Default::default(),
                timestamp: Some(ts.timestamp_millis()),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            };
            let seq = writer
                .log_insert_record("test_coll", ts, &record)
                .await
                .unwrap();
            assert_eq!(seq, 0);

            // Log an OHLC insert
            let ts2 = DateTime::parse_from_rfc3339("2024-06-15T11:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            let seq2 = writer
                .log_insert_ohlc(
                    "test_coll",
                    "AAPL",
                    ts2,
                    150.0,
                    155.0,
                    149.0,
                    153.0,
                    1000000,
                )
                .await
                .unwrap();
            assert_eq!(seq2, 1);

            writer.flush().await.unwrap();
        }

        // Phase 2: Recover from WAL into a fresh engine
        {
            let config = TimeSeriesConfig {
                base_path: data_dir.clone(),
                ..Default::default()
            };
            let mut engine = TimeSeriesEngine::with_config(config).unwrap();

            let recovery = TstWalRecovery::new(wal_dir.clone());
            let stats = recovery.recover(&mut engine).await.unwrap();

            assert_eq!(stats.entries_read, 2);
            assert_eq!(stats.entries_replayed, 2);
            assert_eq!(stats.entries_skipped, 0);
            assert_eq!(stats.records_recovered, 1);
            assert_eq!(stats.ohlc_bars_recovered, 1);

            // Verify data was recovered
            let engine_stats = engine.stats();
            assert!(
                engine_stats.total_partitions > 0,
                "Expected at least one partition after recovery"
            );
        }
    }

    #[tokio::test]
    async fn test_tst_wal_recovery_skips_non_ts_entries() {
        let temp_dir = TempDir::new().unwrap();
        let wal_dir = temp_dir.path().join("wal");
        std::fs::create_dir_all(&wal_dir).unwrap();
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        // Write a non-time-series WAL entry
        {
            let wal_path_str = wal_dir.to_str().unwrap().to_string();
            let mut writer = UnifiedWALWriter::new(wal_path_str).await.unwrap();

            use crate::storage::persistence::write_ahead_log::unified_operations::VectorOperation;
            let vector_op = UnifiedWALOperation::VectorOp(VectorOperation::AddVector {
                collection_id: "vec_coll".to_string(),
                vector: VectorRecord {
                    id: "v1".to_string(),
                    vector: vec![0.1, 0.2],
                    metadata: Default::default(),
                    timestamp: Some(0),
                    updated_at: None,
                    expires_at: None,
                    version: None,
                    source: None,
                },
            });
            writer.append(vector_op).await.unwrap();
            writer.flush().await.unwrap();
        }

        // Recovery should skip non-TS entries
        {
            let config = TimeSeriesConfig {
                base_path: data_dir,
                ..Default::default()
            };
            let mut engine = TimeSeriesEngine::with_config(config).unwrap();

            let recovery = TstWalRecovery::new(wal_dir);
            let stats = recovery.recover(&mut engine).await.unwrap();

            assert_eq!(stats.entries_read, 1);
            assert_eq!(stats.entries_replayed, 0);
            assert_eq!(stats.entries_skipped, 1);
        }
    }

    #[tokio::test]
    async fn test_tst_wal_partition_create_and_drop() {
        let temp_dir = TempDir::new().unwrap();
        let wal_dir = temp_dir.path().join("wal");
        std::fs::create_dir_all(&wal_dir).unwrap();
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let partition_key = DateTime::parse_from_rfc3339("2024-06-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Write create + drop partition WAL entries
        {
            let writer = TstWalWriter::new(&wal_dir).await.unwrap();
            writer
                .log_create_partition("test_coll", partition_key)
                .await
                .unwrap();
            writer
                .log_drop_partition("test_coll", partition_key)
                .await
                .unwrap();
            writer.flush().await.unwrap();
        }

        // Recovery should create then drop the partition
        {
            let config = TimeSeriesConfig {
                base_path: data_dir,
                ..Default::default()
            };
            let mut engine = TimeSeriesEngine::with_config(config).unwrap();

            let recovery = TstWalRecovery::new(wal_dir);
            let stats = recovery.recover(&mut engine).await.unwrap();

            assert_eq!(stats.entries_replayed, 2);
            assert_eq!(stats.partitions_created, 1);
            // After create + drop, partition count should be 0
            assert_eq!(engine.stats().total_partitions, 0);
        }
    }
}
