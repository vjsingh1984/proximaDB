//! Time Partition Module
//!
//! Manages individual time partitions for the TST engine.
//! Each partition stores data for a specific time window (e.g., one day).

use anyhow::Result;
use arrow::array::{Array, Float32Array, Float64Array, Int64Array, StringArray};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::OHLCBar;
use crate::proto::proximadb_v1::VectorRecord;

/// Time partition storing data for a specific time window
pub struct TimePartition {
    /// Partition identifier (start time of this partition)
    pub key: DateTime<Utc>,

    /// Collection ID this partition belongs to
    pub collection_id: String,

    /// Time-series records indexed by timestamp
    /// Using BTreeMap for efficient time-range queries
    records: BTreeMap<DateTime<Utc>, VectorRecord>,

    /// OHLC bars indexed by symbol and timestamp
    /// Structure: symbol -> timestamp -> OHLC bar
    ohlc_bars: BTreeMap<String, BTreeMap<DateTime<Utc>, OHLCBar>>,

    /// Partition metadata
    metadata: PartitionMetadata,

    /// In-memory flag
    in_memory: bool,
}

/// Partition metadata
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct PartitionMetadata {
    /// Number of records in partition
    pub record_count: usize,

    /// Partition size in bytes
    pub size_bytes: usize,

    /// Oldest timestamp in partition
    pub min_timestamp: Option<DateTime<Utc>>,

    /// Newest timestamp in partition
    pub max_timestamp: Option<DateTime<Utc>>,

    /// Last flush time
    pub last_flush: Option<DateTime<Utc>>,
}


impl TimePartition {
    /// Create a new empty time partition
    pub fn new(key: DateTime<Utc>, collection_id: String) -> Result<Self> {
        Ok(Self {
            key,
            collection_id,
            records: BTreeMap::new(),
            ohlc_bars: BTreeMap::new(),
            metadata: PartitionMetadata::default(),
            in_memory: true,
        })
    }

    /// Get the size in bytes
    pub fn size_bytes(&self) -> usize {
        self.metadata.size_bytes
    }

    /// Get the number of records
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Insert a record into this partition
    pub async fn insert(&mut self, timestamp: DateTime<Utc>, record: VectorRecord) -> Result<()> {
        self.records.insert(timestamp, record);
        self.metadata.record_count = self.records.len();

        // Update timestamp bounds
        if self.metadata.min_timestamp.is_none() || Some(timestamp) < self.metadata.min_timestamp {
            self.metadata.min_timestamp = Some(timestamp);
        }
        if self.metadata.max_timestamp.is_none() || Some(timestamp) > self.metadata.max_timestamp {
            self.metadata.max_timestamp = Some(timestamp);
        }

        Ok(())
    }

    /// Insert an OHLC bar
    pub async fn insert_ohlc(&mut self, bar: OHLCBar) -> Result<()> {
        let symbol_bars = self
            .ohlc_bars
            .entry(bar.symbol.clone())
            .or_default();

        let timestamp = bar.timestamp;
        symbol_bars.insert(timestamp, bar);

        // Update metadata
        self.metadata.record_count += 1;

        if self.metadata.min_timestamp.is_none() || Some(timestamp) < self.metadata.min_timestamp {
            self.metadata.min_timestamp = Some(timestamp);
        }
        if self.metadata.max_timestamp.is_none() || Some(timestamp) > self.metadata.max_timestamp {
            self.metadata.max_timestamp = Some(timestamp);
        }

        Ok(())
    }

    /// Query records within a time range
    pub async fn query_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<VectorRecord>> {
        Ok(self
            .records
            .range(start..=end)
            .map(|(_, record)| record.clone())
            .collect())
    }

    /// Query OHLC bars for a symbol within a time range
    pub async fn query_ohlc(
        &self,
        symbol: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OHLCBar>> {
        if let Some(symbol_bars) = self.ohlc_bars.get(symbol) {
            Ok(symbol_bars
                .range(start..=end)
                .map(|(_, bar)| bar.clone())
                .collect())
        } else {
            Ok(Vec::new())
        }
    }

    /// Get all records in this partition
    pub async fn all_records(&self) -> Result<Vec<VectorRecord>> {
        Ok(self.records.values().cloned().collect())
    }

    /// Get all OHLC bars in this partition
    pub async fn all_ohlc_bars(&self) -> Result<Vec<OHLCBar>> {
        let mut all_bars = Vec::new();
        for symbol_bars in self.ohlc_bars.values() {
            for bar in symbol_bars.values() {
                all_bars.push(bar.clone());
            }
        }
        Ok(all_bars)
    }

    /// Flush this partition to disk
    pub async fn flush_to_disk(&self, path: &PathBuf) -> Result<()> {
        use arrow::array::{Float32Array, StringArray, TimestampMillisecondArray};
        use arrow::ipc::writer::FileWriter;
        use arrow_schema::{DataType, Field, Schema};

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create Arrow schema for time-series data
        let schema = Schema::new(vec![
            Field::new(
                "timestamp",
                DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
                false,
            ),
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
                true,
            ),
        ]);

        // Create file writer
        let file = std::fs::File::create(path)?;
        let mut writer = FileWriter::try_new(file, &schema)?;

        // Build arrays from records
        let mut timestamps = Vec::new();
        let mut ids = Vec::new();
        let mut vectors: Vec<Vec<f32>> = Vec::new();

        for (ts, record) in &self.records {
            timestamps.push(ts.timestamp_millis());
            ids.push(record.id.clone());
            vectors.push(record.vector.clone());
        }

        // Create Arrow arrays
        let timestamp_array = TimestampMillisecondArray::from(timestamps);
        let id_array = StringArray::from(ids);

        // Create vector array (list of floats)
        let vector_data: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        let vector_array =
            Float32Array::from_iter(vector_data.iter().flat_map(|v| v.iter().cloned()));
        let vector_offsets: Vec<i32> = std::iter::once(0)
            .chain(vectors.iter().scan(0, |acc, v| {
                *acc += v.len() as i32;
                Some(*acc)
            }))
            .collect();

        // Create ListArray correctly for Arrow 57.x
        let vector_list_array = arrow::array::ListArray::try_new(
            Field::new_list_field(DataType::Float32, true).into(),
            arrow::buffer::OffsetBuffer::new(vector_offsets.into()),
            std::sync::Arc::new(vector_array) as std::sync::Arc<dyn arrow_array::Array>,
            None,
        )?;

        // Create record batch and write
        let batch = arrow::record_batch::RecordBatch::try_new(
            schema.clone().into(),
            vec![
                std::sync::Arc::new(timestamp_array),
                std::sync::Arc::new(id_array),
                std::sync::Arc::new(vector_list_array),
            ],
        )?;

        writer.write(&batch)?;
        writer.finish()?;

        // Update metadata
        let mut _updated_metadata = self.metadata.clone();
        _updated_metadata.last_flush = Some(chrono::Utc::now());

        Ok(())
    }

    /// Load partition from disk
    pub async fn load_from_disk(path: &PathBuf) -> Result<Self> {
        use arrow::ipc::reader::FileReader;
        use arrow::record_batch::RecordBatch;

        if !path.exists() {
            return Err(anyhow::anyhow!("Partition file not found: {:?}", path));
        }

        // Open file and create reader
        let file = std::fs::File::open(path)?;
        let reader = FileReader::try_new(file, None)?;

        if !reader.num_batches() > 0 {
            return Err(anyhow::anyhow!("No data batches in partition file"));
        }

        // Create empty partition
        let mut partition = Self::new(chrono::Utc::now(), "loaded_from_disk".to_string())?;

        // Read all batches and reconstruct records
        for batch_result in reader {
            let batch: RecordBatch = batch_result?;

            // Extract columns
            let timestamp_array = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::TimestampMillisecondArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp column"))?;

            let id_array = batch
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid id column"))?;

            let vector_list_array = batch
                .column(2)
                .as_any()
                .downcast_ref::<arrow::array::ListArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid vector column"))?;

            let vector_values = vector_list_array
                .values()
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| anyhow::anyhow!("Invalid vector values"))?;

            // Reconstruct records
            for i in 0..batch.num_rows() {
                let timestamp_millis = timestamp_array.value(i);
                let timestamp = DateTime::<Utc>::from_timestamp(
                    timestamp_millis / 1000,
                    ((timestamp_millis % 1000) * 1_000_000) as u32,
                )
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;

                let id = id_array.value(i).to_string();

                let vector = if let Some(start) = vector_list_array.offsets().get(i) {
                    let len_i32 = vector_values.len() as i32;
                    let end = vector_list_array.offsets().get(i + 1).unwrap_or(&len_i32);
                    let start_i32 = *start;
                    let end_i32 = *end;
                    let count = end_i32.saturating_sub(start_i32);
                    vector_values
                        .iter()
                        .skip(start_i32 as usize)
                        .take(count as usize)
                        .map(|v| v.unwrap_or(0.0))
                        .collect()
                } else {
                    Vec::new()
                };

                let record = VectorRecord {
                    id,
                    vector,
                    timestamp: Some(timestamp_millis),
                    ..Default::default()
                };

                partition.records.insert(timestamp, record);
            }
        }

        partition.metadata.last_flush = Some(chrono::Utc::now());

        Ok(partition)
    }

    /// Get partition metadata
    pub fn metadata(&self) -> &PartitionMetadata {
        &self.metadata
    }

    /// Check if partition is in memory
    pub fn is_in_memory(&self) -> bool {
        self.in_memory
    }
}

/// Columnar partition for efficient storage
///
/// This is a more advanced version that stores data in columnar format
/// (similar to Parquet) for better compression and query performance.
pub struct ColumnarPartition {
    /// Partition identifier
    pub key: DateTime<Utc>,

    /// Collection ID
    pub collection_id: String,

    /// Columnar data storage
    /// Each column stores data for a specific field
    pub columns: ColumnarData,

    /// Partition metadata
    pub metadata: PartitionMetadata,
}

/// Columnar data storage
#[derive(Debug, Clone, Default)]
pub struct ColumnarData {
    /// Timestamps for all records
    pub timestamps: Vec<DateTime<Utc>>,

    /// Vector IDs
    pub ids: Vec<String>,

    /// Vector embeddings
    pub vectors: Vec<Vec<f32>>,

    /// Metadata fields
    pub metadata_fields: BTreeMap<String, Column>,

    /// OHLC data (if present)
    pub ohlc_data: Option<OHLCColumnData>,
}

/// Column data with typed values
#[derive(Debug, Clone)]
pub enum Column {
    String(Vec<String>),
    Float32(Vec<f32>),
    Float64(Vec<f64>),
    Int64(Vec<i64>),
    Boolean(Vec<bool>),
}

/// OHLC data in columnar format
#[derive(Debug, Clone)]
pub struct OHLCColumnData {
    /// Symbols for each bar
    pub symbols: Vec<String>,

    /// Open prices
    pub opens: Vec<f64>,

    /// High prices
    pub highs: Vec<f64>,

    /// Low prices
    pub lows: Vec<f64>,

    /// Close prices
    pub closes: Vec<f64>,

    /// Volumes
    pub volumes: Vec<i64>,
}

impl ColumnarPartition {
    /// Create a new empty columnar partition
    pub fn new(key: DateTime<Utc>, collection_id: String) -> Self {
        Self {
            key,
            collection_id,
            columns: ColumnarData::default(),
            metadata: PartitionMetadata::default(),
        }
    }

    /// Add a record to this partition
    pub fn add_record(&mut self, timestamp: DateTime<Utc>, record: VectorRecord) -> Result<()> {
        self.columns.timestamps.push(timestamp);
        self.columns.ids.push(record.id.clone());

        if !record.vector.is_empty() {
            self.columns.vectors.push(record.vector.clone());
        }

        // Extract metadata fields into columns
        // Deferred: Implement proper SqlValue extraction
        // For now, just skip metadata extraction
        let _ = &record.metadata;

        Ok(())
    }

    /// Query records by time range
    pub fn query_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<VectorRecord>> {
        let mut results = Vec::new();

        for (idx, timestamp) in self.columns.timestamps.iter().enumerate() {
            if *timestamp >= start && *timestamp <= end {
                let ts_i64 = timestamp.timestamp();
                let record = VectorRecord {
                    id: self
                        .columns
                        .ids
                        .get(idx)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing ID at index {}", idx))?,
                    vector: self
                        .columns
                        .vectors
                        .get(idx)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing vector at index {}", idx))?,
                    timestamp: Some(ts_i64),
                    // Reconstruct metadata from columns
                    metadata: std::collections::HashMap::new(), // Deferred: Reconstruct from columnar data
                    ..Default::default()
                };
                results.push(record);
            }
        }

        Ok(results)
    }

    /// Flush to disk
    pub async fn flush_to_disk(&self, path: &PathBuf) -> Result<()> {
        use arrow::array::{
            Float32Array, Float64Array, Int64Array, StringArray, TimestampMillisecondArray,
        };
        use arrow::ipc::writer::FileWriter;
        use arrow_schema::{DataType, Field, Schema};

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create Arrow schema for columnar time-series data
        let mut fields = vec![
            Field::new(
                "timestamp",
                DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
                false,
            ),
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
                true,
            ),
        ];

        // Add OHLC fields if present
        let ohlc_data = self.columns.ohlc_data.as_ref();
        if ohlc_data.is_some() {
            fields.extend(vec![
                Field::new("symbol", DataType::Utf8, false),
                Field::new("open", DataType::Float64, false),
                Field::new("high", DataType::Float64, false),
                Field::new("low", DataType::Float64, false),
                Field::new("close", DataType::Float64, false),
                Field::new("volume", DataType::Int64, false),
            ]);
        }

        let schema = Schema::new(fields);

        // Create file writer
        let file = std::fs::File::create(path)?;
        let mut writer = FileWriter::try_new(file, &schema)?;

        // Build arrays from columnar data
        let timestamps: Vec<i64> = self
            .columns
            .timestamps
            .iter()
            .map(|ts| ts.timestamp_millis())
            .collect();
        let timestamp_array = TimestampMillisecondArray::from(timestamps);

        let id_array = StringArray::from(self.columns.ids.clone());

        // Create vector array (list of floats)
        let vector_offsets: Vec<i32> = std::iter::once(0)
            .chain(self.columns.vectors.iter().scan(0, |acc, v| {
                *acc += v.len() as i32;
                Some(*acc)
            }))
            .collect();
        let vector_values: Vec<f32> = self
            .columns
            .vectors
            .iter()
            .flat_map(|v| v.iter().cloned())
            .collect();
        let vector_array = Float32Array::from(vector_values);
        let vector_list_array = arrow::array::ListArray::try_new(
            Field::new_list_field(DataType::Float32, true).into(),
            arrow::buffer::OffsetBuffer::new(vector_offsets.into()),
            std::sync::Arc::new(vector_array) as std::sync::Arc<dyn arrow_array::Array>,
            None,
        )?;

        let mut columns = vec![
            std::sync::Arc::new(timestamp_array) as std::sync::Arc<dyn arrow::array::Array>,
            std::sync::Arc::new(id_array) as std::sync::Arc<dyn arrow::array::Array>,
            std::sync::Arc::new(vector_list_array) as std::sync::Arc<dyn arrow::array::Array>,
        ];

        // Add OHLC columns if present
        if let Some(ohlc) = &self.columns.ohlc_data {
            columns.push(std::sync::Arc::new(StringArray::from(ohlc.symbols.clone())));
            columns.push(std::sync::Arc::new(Float64Array::from(ohlc.opens.clone())));
            columns.push(std::sync::Arc::new(Float64Array::from(ohlc.highs.clone())));
            columns.push(std::sync::Arc::new(Float64Array::from(ohlc.lows.clone())));
            columns.push(std::sync::Arc::new(Float64Array::from(ohlc.closes.clone())));
            columns.push(std::sync::Arc::new(Int64Array::from(ohlc.volumes.clone())));
        }

        // Create record batch and write
        let batch = arrow::record_batch::RecordBatch::try_new(schema.into(), columns)?;
        writer.write(&batch)?;
        writer.finish()?;

        Ok(())
    }

    /// Load from disk
    pub async fn load_from_disk(path: &PathBuf) -> Result<Self> {
        use arrow::ipc::reader::FileReader;

        if !path.exists() {
            return Err(anyhow::anyhow!(
                "Columnar partition file not found: {:?}",
                path
            ));
        }

        // Open file and create reader
        let file = std::fs::File::open(path)?;
        let reader = FileReader::try_new(file, None)?;

        if !reader.num_batches() > 0 {
            return Err(anyhow::anyhow!(
                "No data batches in columnar partition file"
            ));
        }

        // Create empty partition
        let mut partition = Self::new(chrono::Utc::now(), "loaded_from_disk".to_string());

        // Read first batch to populate columns
        if let Some(Ok(batch)) = reader.into_iter().next() {
            // Extract timestamps
            let timestamp_array = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::TimestampMillisecondArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp column"))?;

            for i in 0..(timestamp_array.len()) {
                let ts_millis = timestamp_array.value(i);
                if let Some(dt) = DateTime::<Utc>::from_timestamp(
                    ts_millis / 1000,
                    ((ts_millis % 1000) * 1_000_000) as u32,
                ) {
                    partition.columns.timestamps.push(dt);
                }
            }

            // Extract IDs
            let id_array = batch
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid id column"))?;

            // Use Array trait's len() method
            use arrow::array::Array;
            for i in 0..id_array.len() {
                partition.columns.ids.push(id_array.value(i).to_string());
            }

            // Extract vectors
            let vector_list_array = batch
                .column(2)
                .as_any()
                .downcast_ref::<arrow::array::ListArray>()
                .ok_or_else(|| anyhow::anyhow!("Invalid vector column"))?;

            let vector_values = vector_list_array
                .values()
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| anyhow::anyhow!("Invalid vector values"))?;

            for i in 0..(vector_list_array.len()) {
                let offsets = vector_list_array.offsets();
                let start = offsets[i] as usize;
                let end = offsets
                    .get(i + 1)
                    .map_or(vector_values.len(), |&v| v as usize);
                let vector = vector_values
                    .iter()
                    .skip(start)
                    .take(end - start)
                    .map(|v| v.unwrap_or(0.0))
                    .collect();
                partition.columns.vectors.push(vector);
            }

            // Extract OHLC data if present (columns 3-8)
            if batch.num_columns() >= 9 {
                let symbols = batch
                    .column(3)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i).to_string()).collect())
                    .ok_or_else(|| anyhow::anyhow!("Invalid symbol column"))?;

                let opens = batch
                    .column(4)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect())
                    .ok_or_else(|| anyhow::anyhow!("Invalid open column"))?;

                let highs = batch
                    .column(5)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect())
                    .ok_or_else(|| anyhow::anyhow!("Invalid high column"))?;

                let lows = batch
                    .column(6)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect())
                    .ok_or_else(|| anyhow::anyhow!("Invalid low column"))?;

                let closes = batch
                    .column(7)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect())
                    .ok_or_else(|| anyhow::anyhow!("Invalid close column"))?;

                let volumes = batch
                    .column(8)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect())
                    .ok_or_else(|| anyhow::anyhow!("Invalid volume column"))?;

                partition.columns.ohlc_data = Some(OHLCColumnData {
                    symbols,
                    opens,
                    highs,
                    lows,
                    closes,
                    volumes,
                });
            }

            // Update metadata
            partition.metadata.record_count = partition.columns.timestamps.len();
        }

        Ok(partition)
    }
}

/// Partition key for indexing
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionKey {
    /// Collection ID
    pub collection_id: String,

    /// Partition start time
    pub start_time: DateTime<Utc>,
}

impl PartitionKey {
    /// Create a new partition key
    pub fn new(collection_id: String, start_time: DateTime<Utc>) -> Self {
        Self {
            collection_id,
            start_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_insert() {
        let mut partition = TimePartition::new(
            DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .expect("valid timestamp")
                .with_timezone(&Utc),
            "test_collection".to_string(),
        )
        .expect("failed to create partition");

        let timestamp = DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);

        let record = VectorRecord {
            id: "test_id".to_string(),
            timestamp: Some(timestamp.timestamp_millis()),
            ..Default::default()
        };

        tokio::runtime::Runtime::new()
            .expect("failed to create runtime")
            .block_on(async {
                partition
                    .insert(timestamp, record)
                    .await
                    .expect("failed to insert record");
                assert_eq!(partition.record_count(), 1);
            });
    }

    #[test]
    fn test_partition_query_time_range() {
        let mut partition = TimePartition::new(
            DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .expect("valid timestamp")
                .with_timezone(&Utc),
            "test_collection".to_string(),
        )
        .expect("failed to create partition");

        let dt1 = DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let dt2 = DateTime::parse_from_rfc3339("2024-01-01T14:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);

        tokio::runtime::Runtime::new()
            .expect("failed to create runtime")
            .block_on(async {
                partition
                    .insert(
                        dt1,
                        VectorRecord {
                            id: "test1".to_string(),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("failed to insert record");

                partition
                    .insert(
                        dt2,
                        VectorRecord {
                            id: "test2".to_string(),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("failed to insert record");

                let start = DateTime::parse_from_rfc3339("2024-01-01T09:00:00Z")
                    .expect("valid timestamp")
                    .with_timezone(&Utc);
                let end = DateTime::parse_from_rfc3339("2024-01-01T11:00:00Z")
                    .expect("valid timestamp")
                    .with_timezone(&Utc);

                let results = partition
                    .query_time_range(start, end)
                    .await
                    .expect("failed to query time range");
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].id, "test1");
            });
    }

    #[test]
    fn test_columnar_partition() {
        let mut partition = ColumnarPartition::new(
            DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .expect("valid timestamp")
                .with_timezone(&Utc),
            "test_collection".to_string(),
        );

        let timestamp = DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);

        let record = VectorRecord {
            id: "test_id".to_string(),
            timestamp: Some(timestamp.timestamp_millis()),
            ..Default::default()
        };

        tokio::runtime::Runtime::new()
            .expect("failed to create runtime")
            .block_on(async {
                partition
                    .add_record(timestamp, record)
                    .expect("failed to add record");
                assert_eq!(partition.columns.timestamps.len(), 1);
                assert_eq!(partition.columns.ids.len(), 1);
            });
    }
}
