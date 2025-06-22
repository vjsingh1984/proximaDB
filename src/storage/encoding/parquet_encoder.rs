use crate::core::VectorRecord;
use crate::storage::encoding::{CompressionType, Encoder};
use arrow_array::{ArrayRef, Float32Array, StringArray, TimestampNanosecondArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::sync::Arc;

pub struct ParquetEncoder {
    compression: CompressionType,
    row_group_size: usize,
}

impl ParquetEncoder {
    pub fn new(compression: CompressionType, row_group_size: usize) -> Self {
        Self {
            compression,
            row_group_size,
        }
    }

    fn create_schema(dimension: usize) -> Schema {
        let mut fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "timestamp",
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                false,
            ),
            Field::new("metadata_json", DataType::Utf8, true),
        ];

        // Add vector dimensions as separate columns for better compression
        for i in 0..dimension {
            fields.push(Field::new(&format!("vec_{}", i), DataType::Float32, false));
        }

        Schema::new(fields)
    }

    fn records_to_batch(&self, records: &[VectorRecord]) -> crate::Result<RecordBatch> {
        if records.is_empty() {
            return Err("Cannot create batch from empty records".into());
        }

        let dimension = records[0].vector.len();
        let schema = Arc::new(Self::create_schema(dimension));

        // Extract data into arrays
        let ids: Vec<String> = records.iter().map(|r| r.id.to_string()).collect();
        let collection_ids: Vec<String> = records.iter().map(|r| r.collection_id.clone()).collect();
        let timestamps: Vec<i64> = records
            .iter()
            .map(|r| r.timestamp.timestamp_nanos_opt().unwrap_or(0))
            .collect();
        let metadata_json: Vec<Option<String>> = records
            .iter()
            .map(|r| Some(serde_json::to_string(&r.metadata).ok()?))
            .collect();

        // Create arrays
        let mut arrays: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(StringArray::from(collection_ids)),
            Arc::new(TimestampNanosecondArray::from(timestamps)),
            Arc::new(StringArray::from(metadata_json)),
        ];

        // Add vector dimension arrays
        for dim in 0..dimension {
            let values: Vec<f32> = records.iter().map(|r| r.vector[dim]).collect();
            arrays.push(Arc::new(Float32Array::from(values)));
        }

        let batch = RecordBatch::try_new(schema, arrays)?;
        Ok(batch)
    }
}

impl Encoder for ParquetEncoder {
    fn encode(&self, records: &[VectorRecord]) -> crate::Result<Vec<u8>> {
        let batch = self.records_to_batch(records)?;

        // Configure compression based on type
        let props = WriterProperties::builder()
            .set_max_row_group_size(self.row_group_size)
            .build();

        let mut buffer = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), Some(props))?;
            writer.write(&batch)?;
            writer.close()?;
        }

        Ok(buffer)
    }

    fn decode(&self, _data: &[u8]) -> crate::Result<Vec<VectorRecord>> {
        // TODO: Implement Parquet decoding using Arrow reader
        // This would read the Parquet file and convert back to VectorRecord structs
        unimplemented!("Parquet decoding not yet implemented")
    }
}
