//! VIPER Vector Processing with Template Method Pattern
//! 
//! This module implements the Template Method pattern for vector record processing,
//! allowing for configurable preprocessing, conversion, and postprocessing steps.

use anyhow::Result;
use arrow::array::RecordBatch;
use arrow::datatypes::Schema;
use std::sync::Arc;

use crate::core::VectorRecord;
use super::adapter::VectorRecordSchemaAdapter;
use super::flusher::{ViperParquetFlusher, FlushResult};
use super::schema::SchemaGenerationStrategy;

/// Template Method Pattern: Defines the algorithm for vector record processing
pub trait VectorProcessor {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()>;
    fn convert_to_batch(&self, records: &[VectorRecord], schema: &Arc<Schema>) -> Result<RecordBatch>;
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch>;
}

/// Template Method Pattern implementation for vector processing
pub struct VectorRecordProcessor<'a> {
    strategy: &'a dyn SchemaGenerationStrategy,
}

impl<'a> VectorRecordProcessor<'a> {
    pub fn new(strategy: &'a dyn SchemaGenerationStrategy) -> Self {
        Self { strategy }
    }
    
    /// Template method defining the processing algorithm
    pub async fn flush_to_parquet(
        &self,
        flusher: &ViperParquetFlusher,
        mut records: Vec<VectorRecord>,
        output_url: &str,
    ) -> Result<FlushResult> {
        if records.is_empty() {
            return Ok(FlushResult {
                entries_flushed: 0,
                bytes_written: 0,
                segments_created: 0,
                collections_affected: vec![],
                flush_duration_ms: 0,
            });
        }
        
        let start_time = std::time::Instant::now();
        
        tracing::info!("🎯 Processing {} vector records with VIPER strategy for collection '{}'", 
                      records.len(), self.strategy.get_collection_id());
        
        // Step 1: Preprocess (Template Method hook)
        self.preprocess_records(&mut records)?;
        
        // Step 2: Generate schema
        let schema = self.strategy.generate_schema()?;
        
        // Step 3: Convert to batch (Template Method hook)
        let record_batch = self.convert_to_batch(&records, &schema)?;
        
        // Step 4: Postprocess (Template Method hook)
        let final_batch = self.postprocess_batch(record_batch)?;
        
        // Step 5: Write to Parquet
        let bytes_written = flusher.write_parquet(&final_batch, output_url).await?;
        
        let flush_duration = start_time.elapsed().as_millis() as u64;
        
        tracing::info!("✅ VIPER vector processing completed: {} records, {} bytes, {}ms", 
                      records.len(), bytes_written, flush_duration);
        
        Ok(FlushResult {
            entries_flushed: records.len() as u64,
            bytes_written,
            segments_created: 1,
            collections_affected: vec![self.strategy.get_collection_id().clone()],
            flush_duration_ms: flush_duration,
        })
    }
}

impl<'a> VectorProcessor for VectorRecordProcessor<'a> {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()> {
        // Default preprocessing: Sort by timestamp for better compression
        records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        
        tracing::debug!("🔧 Preprocessed {} records (sorted by timestamp)", records.len());
        Ok(())
    }
    
    fn convert_to_batch(&self, records: &[VectorRecord], schema: &Arc<Schema>) -> Result<RecordBatch> {
        let adapter = VectorRecordSchemaAdapter::new(self.strategy);
        adapter.adapt_records_to_schema(records, schema)
    }
    
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch> {
        // Default postprocessing: Return as-is
        // Future extensions could add compression hints, column reordering, etc.
        Ok(batch)
    }
}

/// Specialized processor for time-series optimized processing
pub struct TimeSeriesVectorProcessor<'a> {
    base_processor: VectorRecordProcessor<'a>,
    time_window_seconds: u64,
}

impl<'a> TimeSeriesVectorProcessor<'a> {
    pub fn new(strategy: &'a dyn SchemaGenerationStrategy, time_window_seconds: u64) -> Self {
        Self {
            base_processor: VectorRecordProcessor::new(strategy),
            time_window_seconds,
        }
    }
}

impl<'a> VectorProcessor for TimeSeriesVectorProcessor<'a> {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()> {
        // Time-series specific preprocessing: Group by time windows
        records.sort_by(|a, b| {
            let window_a = a.timestamp.timestamp() / self.time_window_seconds as i64;
            let window_b = b.timestamp.timestamp() / self.time_window_seconds as i64;
            window_a.cmp(&window_b).then_with(|| a.timestamp.cmp(&b.timestamp))
        });
        
        tracing::debug!("🕐 Time-series preprocessed {} records (grouped by {}-second windows)", 
                       records.len(), self.time_window_seconds);
        Ok(())
    }
    
    fn convert_to_batch(&self, records: &[VectorRecord], schema: &Arc<Schema>) -> Result<RecordBatch> {
        self.base_processor.convert_to_batch(records, schema)
    }
    
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch> {
        // Could add time-series specific optimizations here
        self.base_processor.postprocess_batch(batch)
    }
}

/// Specialized processor for similarity-optimized processing  
pub struct SimilarityVectorProcessor<'a> {
    base_processor: VectorRecordProcessor<'a>,
    cluster_threshold: f32,
}

impl<'a> SimilarityVectorProcessor<'a> {
    pub fn new(strategy: &'a dyn SchemaGenerationStrategy, cluster_threshold: f32) -> Self {
        Self {
            base_processor: VectorRecordProcessor::new(strategy),
            cluster_threshold,
        }
    }
}

impl<'a> VectorProcessor for SimilarityVectorProcessor<'a> {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()> {
        // Similarity-based preprocessing: Sort by vector similarity clusters
        // This is a simplified version - in production you'd use proper clustering
        records.sort_by(|a, b| {
            let sum_a: f32 = a.vector.iter().sum();
            let sum_b: f32 = b.vector.iter().sum();
            sum_a.partial_cmp(&sum_b).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        tracing::debug!("🎯 Similarity-based preprocessed {} records (clustered by similarity)", 
                       records.len());
        Ok(())
    }
    
    fn convert_to_batch(&self, records: &[VectorRecord], schema: &Arc<Schema>) -> Result<RecordBatch> {
        self.base_processor.convert_to_batch(records, schema)
    }
    
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch> {
        self.base_processor.postprocess_batch(batch)
    }
}