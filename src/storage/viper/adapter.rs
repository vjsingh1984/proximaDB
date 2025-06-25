//! VIPER Record to Schema Adaptation
//!
//! This module implements the Adapter pattern for converting vector records
//! to different schema formats efficiently.

use anyhow::{Context, Result};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_array::builder::{StringBuilder, ListBuilder, Float32Builder, MapBuilder, TimestampMillisecondBuilder};
use arrow_schema::Schema;
use std::collections::HashMap;
use std::sync::Arc;

use super::schema::SchemaGenerationStrategy;
use crate::core::VectorRecord;

/// Adapter Pattern: Adapts vector records to different schema formats
pub struct VectorRecordSchemaAdapter<'a> {
    strategy: &'a dyn SchemaGenerationStrategy,
}

impl<'a> VectorRecordSchemaAdapter<'a> {
    pub fn new(strategy: &'a dyn SchemaGenerationStrategy) -> Self {
        Self { strategy }
    }

    pub fn adapt_records_to_schema(
        &self,
        records: &[VectorRecord],
        schema: &Arc<Schema>,
    ) -> Result<RecordBatch> {
        let mut id_builder = StringBuilder::new();
        let mut vectors_builder =
            ListBuilder::new(Float32Builder::new());

        // Dynamic metadata builders for filterable fields
        let mut meta_builders: HashMap<String, StringBuilder> = HashMap::new();
        for field_name in self.strategy.get_filterable_fields() {
            meta_builders.insert(field_name.clone(), StringBuilder::new());
        }

        // Extra metadata and timestamp builders
        let mut extra_meta_builder = MapBuilder::new(
            None,
            StringBuilder::new(),
            StringBuilder::new(),
        );
        let mut expires_at_builder = TimestampMillisecondBuilder::new();
        let mut created_at_builder = TimestampMillisecondBuilder::new();
        let mut updated_at_builder = TimestampMillisecondBuilder::new();

        for record in records {
            // ID
            id_builder.append_value(&record.id);

            // Vectors
            for &val in &record.vector {
                vectors_builder.values().append_value(val);
            }
            vectors_builder.append(true);

            // Filterable metadata fields
            for field_name in self.strategy.get_filterable_fields() {
                let builder = meta_builders.get_mut(field_name).unwrap();
                if let Some(value) = record.metadata.get(field_name) {
                    builder.append_value(&value.to_string());
                } else {
                    builder.append_null();
                }
            }

            // Extra metadata (non-filterable fields)
            let filterable_set: std::collections::HashSet<_> =
                self.strategy.get_filterable_fields().iter().collect();
            for (key, value) in &record.metadata {
                if !filterable_set.contains(key) {
                    extra_meta_builder.keys().append_value(key);
                    extra_meta_builder.values().append_value(&value.to_string());
                }
            }
            extra_meta_builder.append(true);

            // Timestamps
            if let Some(expires_at) = record.expires_at {
                expires_at_builder.append_value(expires_at);
            } else {
                expires_at_builder.append_null();
            }
            created_at_builder.append_value(record.timestamp);
            updated_at_builder.append_value(record.timestamp);
        }

        // Build arrays
        let mut arrays: Vec<ArrayRef> = Vec::new();
        arrays.push(Arc::new(id_builder.finish()));
        arrays.push(Arc::new(vectors_builder.finish()));

        // Add filterable metadata arrays
        for field_name in self.strategy.get_filterable_fields() {
            let mut builder = meta_builders.remove(field_name).unwrap();
            arrays.push(Arc::new(builder.finish()));
        }

        // Add extra metadata and timestamps
        arrays.push(Arc::new(extra_meta_builder.finish()));
        arrays.push(Arc::new(expires_at_builder.finish()));
        arrays.push(Arc::new(created_at_builder.finish()));
        arrays.push(Arc::new(updated_at_builder.finish()));

        RecordBatch::try_new(schema.clone(), arrays)
            .context("Failed to create RecordBatch from adapted records")
    }
}

/// Specialized adapter for time-series data
pub struct TimeSeriesAdapter<'a> {
    base_adapter: VectorRecordSchemaAdapter<'a>,
    time_precision: TimePrecision,
}

#[derive(Debug, Clone)]
pub enum TimePrecision {
    Milliseconds,
    Seconds,
    Minutes,
    Hours,
}

impl<'a> TimeSeriesAdapter<'a> {
    pub fn new(strategy: &'a dyn SchemaGenerationStrategy, time_precision: TimePrecision) -> Self {
        Self {
            base_adapter: VectorRecordSchemaAdapter::new(strategy),
            time_precision,
        }
    }

    pub fn adapt_with_time_bucketing(
        &self,
        records: &[VectorRecord],
        schema: &Arc<Schema>,
    ) -> Result<RecordBatch> {
        // Could implement time bucketing logic here
        // For now, delegate to base adapter
        self.base_adapter.adapt_records_to_schema(records, schema)
    }
}

/// Specialized adapter for compressed vector storage
pub struct CompressedVectorAdapter<'a> {
    base_adapter: VectorRecordSchemaAdapter<'a>,
    compression_type: VectorCompressionType,
}

#[derive(Debug, Clone)]
pub enum VectorCompressionType {
    None,
    Quantized8Bit,
    Quantized16Bit,
    DeltaEncoding,
}

impl<'a> CompressedVectorAdapter<'a> {
    pub fn new(
        strategy: &'a dyn SchemaGenerationStrategy,
        compression_type: VectorCompressionType,
    ) -> Self {
        Self {
            base_adapter: VectorRecordSchemaAdapter::new(strategy),
            compression_type,
        }
    }

    pub fn adapt_with_compression(
        &self,
        records: &[VectorRecord],
        schema: &Arc<Schema>,
    ) -> Result<RecordBatch> {
        // Could implement vector compression logic here
        // For now, delegate to base adapter
        self.base_adapter.adapt_records_to_schema(records, schema)
    }
}
