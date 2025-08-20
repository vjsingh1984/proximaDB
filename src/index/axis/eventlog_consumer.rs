/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! AXIS EventLog consumer that processes flush and compaction events
//! This runs as a background task and builds/updates indexes asynchronously

use anyhow::{Result, Context};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, debug, warn, error};
use dashmap::DashMap;

use crate::index::axis::eventlog::{
    EventLogService,
    IndexEvent,
    EventType,
    StorageEngineType,
    ExtractionMode,
};
use crate::index::axis::AxisManager;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::proto::proximadb::Collection;

/// AXIS EventLog consumer configuration
#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    /// How often to poll for new events (milliseconds)
    pub poll_interval_ms: u64,
    
    /// Maximum events to process in a single batch
    pub batch_size: usize,
    
    /// Whether to enable concurrent processing
    pub concurrent_processing: bool,
    
    /// Maximum concurrent index operations
    pub max_concurrent_ops: usize,
}

impl Default for ConsumerConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 100,     // Poll every 100ms for low latency
            batch_size: 10,            // Process up to 10 events at once
            concurrent_processing: true,
            max_concurrent_ops: 4,     // Max 4 concurrent index operations
        }
    }
}

/// AXIS EventLog consumer
pub struct AxisEventLogConsumer {
    /// Configuration
    config: ConsumerConfig,
    
    /// EventLog service to consume from
    event_log: Arc<dyn EventLogService>,
    
    /// AXIS manager for index operations
    axis_manager: Arc<AxisManager>,
    
    /// Filesystem factory for reading data files
    filesystem_factory: Arc<FilesystemFactory>,
    
    /// Collection cache
    collection_cache: Arc<DashMap<String, Arc<Collection>>>,
    
    /// Shutdown signal
    shutdown: tokio::sync::watch::Receiver<bool>,
}

impl AxisEventLogConsumer {
    /// Create new consumer
    pub fn new(
        config: ConsumerConfig,
        event_log: Arc<dyn EventLogService>,
        axis_manager: Arc<AxisManager>,
        filesystem_factory: Arc<FilesystemFactory>,
        collection_cache: Arc<DashMap<String, Arc<Collection>>>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            config,
            event_log,
            axis_manager,
            filesystem_factory,
            collection_cache,
            shutdown,
        }
    }
    
    /// Start consuming events
    pub async fn run(mut self) {
        info!("Starting AXIS EventLog consumer");
        
        loop {
            // Check for shutdown
            if *self.shutdown.borrow() {
                info!("AXIS EventLog consumer shutting down");
                break;
            }
            
            // Process next batch of events
            match self.process_batch().await {
                Ok(processed) => {
                    if processed > 0 {
                        debug!("Processed {} events from EventLog", processed);
                    }
                }
                Err(e) => {
                    warn!("Error processing EventLog batch: {}", e);
                }
            }
            
            // Sleep before next poll
            sleep(Duration::from_millis(self.config.poll_interval_ms)).await;
        }
        
        info!("AXIS EventLog consumer stopped");
    }
    
    /// Process a batch of events
    async fn process_batch(&self) -> Result<usize> {
        // Get next batch of events
        let events = self.event_log
            .get_next_batch(self.config.batch_size)
            .await
            .context("Failed to get event batch")?;
        
        if events.is_empty() {
            return Ok(0);
        }
        
        let count = events.len();
        debug!("Processing {} events from EventLog", count);
        
        // Process events concurrently if enabled
        if self.config.concurrent_processing {
            self.process_concurrent(events).await?;
        } else {
            self.process_sequential(events).await?;
        }
        
        Ok(count)
    }
    
    /// Process events sequentially
    async fn process_sequential(&self, events: Vec<IndexEvent>) -> Result<()> {
        for event in events {
            self.process_event(event).await?;
        }
        Ok(())
    }
    
    /// Process events concurrently
    async fn process_concurrent(&self, events: Vec<IndexEvent>) -> Result<()> {
        use futures::stream::{self, StreamExt};
        
        let results = stream::iter(events)
            .map(|event| self.process_event(event))
            .buffer_unordered(self.config.max_concurrent_ops)
            .collect::<Vec<_>>()
            .await;
        
        // Check for errors
        for result in results {
            result?;
        }
        
        Ok(())
    }
    
    /// Process a single event
    async fn process_event(&self, event: IndexEvent) -> Result<()> {
        let start_time = std::time::Instant::now();
        let event_id = event.event_id.clone();
        let event_type = match event.operation {
            EventType::Flush => "flush",
            EventType::Compaction => "compaction_info",
            EventType::Delete => "delete",
        };
        
        debug!(
            "[AXIS Consumer] Starting processing of {} event {}:\n  Collection: {}\n  Files: {:?}\n  Vector Count: {}\n  Has Quantized: {}\n  Has FP32: {}\n  Storage Engine: {:?}\n  Created At: {:?}",
            event_type,
            event_id,
            event.collection_id,
            event.file_paths,
            event.vector_count,
            event.has_quantized,
            event.has_fp32,
            event.storage_engine,
            event.timestamp
        );
        
        // Get collection configuration
        debug!("[AXIS Consumer] Looking up collection {} in cache_info", event.collection_id);
        let collection = self.collection_cache
            .get(&event.collection_id)
            .ok_or_else(|| {
                error!("[AXIS Consumer] Collection {} not found in cache for event {}", event.collection_id, event_id);
                anyhow::anyhow!("Collection {} not found", event.collection_id)
            })?
            .clone();
        
        debug!(
            "[AXIS Consumer] Collection {} found: dimension={}, engine={:?}, quantization_enabled={}",
            event.collection_id,
            collection.config.as_ref().map(|c| c.dimension),
            collection.config.as_ref().map(|c| c.storage_engine),
            collection.config.as_ref()
                .and_then(|c| c.quantization.as_ref())
                .map(|q| q.enabled)
                
        );
        
        let result = match event.operation {
            EventType::Flush => {
                debug!("[AXIS Consumer] Routing to flush event processor for event {}", event_id);
                self.process_flush_event(event, collection).await
            }
            EventType::Compaction => {
                debug!("[AXIS Consumer] Routing to compaction event processor for event {}", event_id);
                self.process_compaction_event(event, collection).await
            }
            EventType::Delete => {
                debug!("[AXIS Consumer] Routing to delete event processor for event {}", event_id);
                self.process_delete_event(event, collection).await
            }
        };
        
        let duration = start_time.elapsed();
        
        match &result {
            Ok(()) => {
                debug!(
                    "[AXIS Consumer] Successfully processed {} event {} in {:.2}ms",
                    event_type,
                    event_id,
                    duration.as_secs_f64() * 1000.0
                );
            }
            Err(e) => {
                error!(
                    "[AXIS Consumer] Failed to process {} event {} after {:.2}ms: {}",
                    event_type,
                    event_id,
                    duration.as_secs_f64() * 1000.0,
                    e
                );
            }
        }
        
        result
    }
    
    /// Process flush event - build or update indexes
    async fn process_flush_event(
        &self,
        event: IndexEvent,
        collection: Arc<Collection>,
    ) -> Result<()> {
        let event_id = event.event_id.clone();
        let start_time = std::time::Instant::now();
        
        // Determine extraction mode based on index types
        let extraction_mode = self.determine_extraction_mode(&collection);
        let extraction_mode_str = match extraction_mode {
            ExtractionMode::Fp32Only => "FP32Only",
            ExtractionMode::QuantizedOnly => "QuantizedOnly",
            ExtractionMode::Both => "Both",
            ExtractionMode::Auto => "Auto",
        };
        
        debug!(
            "[AXIS Consumer] Flush event {} extraction mode determined: {} (quantization_enabled={}, has_fp32={}, has_quantized={})",
            event_id,
            extraction_mode_str,
            collection.config.as_ref()
                .and_then(|c| c.quantization.as_ref())
                .map(|q| q.enabled)
                ,
            event.has_fp32,
            event.has_quantized
        );
        
        info!(
            "[AXIS Consumer] Processing flush event {} for collection {} with {} vectors in {} mode",
            event_id,
            event.collection_id,
            event.vector_count,
            extraction_mode_str
        );
        
        // Read vectors from data files based on extraction mode
        let read_start = std::time::Instant::now();
        debug!(
            "[AXIS Consumer] Reading vectors from {} files for event {}: {:?}",
            event.file_paths.len(),
            event_id,
            event.file_paths
        );
        
        let vectors = self.read_vectors_from_files(
            &event.file_paths,
            extraction_mode,
            event.storage_engine,
        ).await?;
        
        let read_duration = read_start.elapsed();
        debug!(
            "[AXIS Consumer] Read {} vectors from files in {:.2}ms for event {}",
            vectors.len(),
            read_duration.as_secs_f64() * 1000.0,
            event_id
        );
        
        // Update AXIS indexes
        let update_start = std::time::Instant::now();
        debug!(
            "[AXIS Consumer] Updating AXIS indexes for collection {} with {} vectors from event {}",
            event.collection_id,
            vectors.len(),
            event_id
        );
        
        // Get index configuration for the collection
        let index_config = self.axis_manager
            .get_native_index_config(&event.collection_id)
            .await
            .unwrap_or_else(|e| {
                warn!("[AXIS Consumer] Failed to get index config for {}: {}, using defaults", event.collection_id, e);
                crate::index::config::IndexConfig::default()
            });
        
        // Update AXIS indexes using hybrid indexing (adapts based on batch size)
        self.axis_manager.index_vectors_hybrid(
            &event.collection_id,
            vectors.clone(),
            event.file_paths.clone(),
            &index_config,
        ).await.map_err(|e| {
            error!("[AXIS Consumer] Failed to update AXIS indexes for event {}: {}", event_id, e);
            e
        })?;
        
        let update_duration = update_start.elapsed();
        debug!(
            "[AXIS Consumer] AXIS index update completed in {:.2}ms for event {}",
            update_duration.as_secs_f64() * 1000.0,
            event_id
        );
        
        // Acknowledge processing completion
        let ack_start = std::time::Instant::now();
        debug!("[AXIS Consumer] Acknowledging event {} with EventLog", event_id);
        
        self.event_log.acknowledge_event(event.event_id.clone()).await?;
        
        let ack_duration = ack_start.elapsed();
        let total_duration = start_time.elapsed();
        
        info!(
            "[AXIS Consumer] Successfully processed flush event {} for collection {} \n  Total: {:.2}ms (read: {:.2}ms, update: {:.2}ms, ack: {:.2}ms)",
            event_id,
            event.collection_id,
            total_duration.as_secs_f64() * 1000.0,
            read_duration.as_secs_f64() * 1000.0,
            update_duration.as_secs_f64() * 1000.0,
            ack_duration.as_secs_f64() * 1000.0
        );
        
        Ok(())
    }
    
    /// Process compaction event - update file references
    async fn process_compaction_event(
        &self,
        event: IndexEvent,
        _collection: Arc<Collection>,
    ) -> Result<()> {
        let event_id = event.event_id.clone();
        let start_time = std::time::Instant::now();
        
        debug!(
            "[AXIS Consumer] Processing compaction event {} for collection {}:\n  Output Files: {:?}\n  Vector Count: {}\n  Storage Engine: {:?}",
            event_id,
            event.collection_id,
            event.file_paths,
            event.vector_count,
            event.storage_engine
        );
        
        info!(
            "[AXIS Consumer] Processing compaction event {} for collection {} with {} output files",
            event_id,
            event.collection_id,
            event.file_paths.len()
        );
        
        // For compaction, we need to update the indexes to reflect the new file structure
        // The vectors are the same but in different files now
        let update_start = std::time::Instant::now();
        debug!(
            "[AXIS Consumer] Rebuilding AXIS indexes after compaction for {} output files in event {}",
            event.file_paths.len(),
            event_id
        );
        
        // Rebuild indexes with the new compacted files
        // Empty deleted_files since compaction already removed the old files
        self.axis_manager.rebuild_indexes_after_compaction(
            &event.collection_id,
            &event.file_paths,  // New compacted files
            &[],                // Old files already deleted by compaction
        ).await.map_err(|e| {
            error!("[AXIS Consumer] Failed to rebuild indexes after compaction for event {}: {}", event_id, e);
            e
        })?;
        
        let update_duration = update_start.elapsed();
        debug!(
            "[AXIS Consumer] File reference update completed in {:.2}ms for event {}",
            update_duration.as_secs_f64() * 1000.0,
            event_id
        );
        
        // Acknowledge processing
        let ack_start = std::time::Instant::now();
        debug!("[AXIS Consumer] Acknowledging compaction event {} with EventLog", event_id);
        
        self.event_log.acknowledge_event(event.event_id.clone()).await?;
        
        let ack_duration = ack_start.elapsed();
        let total_duration = start_time.elapsed();
        
        info!(
            "[AXIS Consumer] Successfully processed compaction event {} for collection {}\n  Total: {:.2}ms (update: {:.2}ms, ack: {:.2}ms)",
            event_id,
            event.collection_id,
            total_duration.as_secs_f64() * 1000.0,
            update_duration.as_secs_f64() * 1000.0,
            ack_duration.as_secs_f64() * 1000.0
        );
        
        Ok(())
    }
    
    /// Process delete event - remove vectors from indexes
    async fn process_delete_event(
        &self,
        event: IndexEvent,
        _collection: Arc<Collection>,
    ) -> Result<()> {
        let event_id = event.event_id.clone();
        
        debug!(
            "[AXIS Consumer] Processing delete event {} for collection {}",
            event_id,
            event.collection_id
        );
        
        // For now, just acknowledge the event
        // TODO: Implement actual deletion logic when needed
        warn!("[AXIS Consumer] Delete event processing not yet implemented for event {}", event_id);
        
        // Acknowledge processing
        self.event_log.acknowledge_event(event.event_id.clone()).await?;
        
        Ok(())
    }
    
    /// Determine extraction mode based on collection's index configuration
    fn determine_extraction_mode(&self, collection: &Collection) -> ExtractionMode {
        // Check if collection has quantization enabled
        let has_quantization = collection.config.as_ref()
            .and_then(|c| c.quantization.as_ref())
            .map(|q| q.enabled)
            ;
        
        // Get index algorithm if configured
        let index_algorithm = collection.config.as_ref()
            .and_then(|c| c.index_config.as_ref())
            .and_then(|i| i.algorithm.as_ref())
            .map(|a| format!("{:?}", a))
            .unwrap_or_else(|| "None".to_string());
        
        debug!(
            "[AXIS Consumer] Determining extraction mode for collection {}:\n  Quantization Enabled: {}\n  Index Algorithm: {}",
            collection.id,
            has_quantization,
            index_algorithm
        );
        
        // Check index types to determine what data we need
        // HNSW typically needs FP32 for accuracy
        // IVF can work with quantized data
        // PQ indexes prefer quantized data
        
        let mode = if !has_quantization {
            // No quantization, must use FP32
            debug!("[AXIS Consumer] No quantization enabled, using FP32Only mode");
            ExtractionMode::Fp32Only
        } else {
            // For now, extract both and let AXIS decide
            // In future, we can be smarter based on index type
            debug!("[AXIS Consumer] Quantization enabled, using Both mode for flexibility");
            ExtractionMode::Both
        };
        
        mode
    }
    
    /// Read vectors from data files
    async fn read_vectors_from_files(
        &self,
        files: &[String],
        extraction_mode: ExtractionMode,
        storage_engine: StorageEngineType,
    ) -> Result<Vec<crate::proto::proximadb::VectorRecord>> {
        let start_time = std::time::Instant::now();
        
        debug!(
            "[AXIS Consumer] Starting vector extraction:\n  Files: {:?}\n  Count: {}\n  Storage Engine: {:?}\n  Extraction Mode: {:?}",
            files,
            files.len(),
            storage_engine,
            extraction_mode
        );
        
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(Default::default()).await
                .context("Failed to create filesystem factory")?
        );
        let filesystem = filesystem_factory.get_filesystem("file:///tmp")
            .context("Failed to get filesystem")?;
        let mut all_vectors = Vec::new();
        
        match storage_engine {
            StorageEngineType::SST => {
                debug!("[AXIS Consumer] Using SST reader for vector extraction");
                
                // Use unified SST reader for efficient access
                use crate::storage::engines::sst::readers::unified_sstable_reader::UnifiedSstableReader;
                use crate::core::VectorRecord;
                
                for file_path in files {
                    debug!("[AXIS Consumer] Reading SST file: {}", file_path);
                    let file_start = std::time::Instant::now();
                    
                    // Read all records from the SST file
                    let reader = UnifiedSstableReader::new(filesystem_factory.clone());
                    let records = reader.read_all_records_for_compaction(&[file_path.clone()]).await
                        .map_err(|e| {
                            error!("[AXIS Consumer] Failed to read SST file {}: {}", file_path, e);
                            e
                        })?;
                    
                    let mut file_vector_count = 0;
                    
                    for vector_record in records {
                        // Check if we should extract this record based on mode
                        let should_extract = match extraction_mode {
                            ExtractionMode::Fp32Only => {
                                // Only extract if we have FP32 data
                                !vector_record.vector.is_empty()
                            }
                            ExtractionMode::QuantizedOnly => {
                                // Only extract if we have quantized data
                                vector_record.quantized_vector.as_ref().map_or(false, |v| !v.is_empty())
                            }
                            ExtractionMode::Both => {
                                // Extract if we have either type
                                !vector_record.vector.is_empty() || vector_record.quantized_vector.as_ref().map_or(false, |v| !v.is_empty())
                            }
                            ExtractionMode::Auto => {
                                // Auto mode: extract if we have any data
                                !vector_record.vector.is_empty() || vector_record.quantized_vector.as_ref().map_or(false, |v| !v.is_empty())
                            }
                        };
                        
                        if should_extract {
                            // Use the vector_record directly (it's already a VectorRecord)
                            let output_vector_record = crate::proto::proximadb::VectorRecord {
                                id: vector_record.id.clone(),
                                vector: match extraction_mode {
                                    ExtractionMode::QuantizedOnly => vec![],
                                    _ => vector_record.vector.clone(),
                                },
                                metadata: vector_record.metadata.clone(),
                                timestamp: vector_record.timestamp,
                                updated_at: vector_record.updated_at,
                                expires_at: vector_record.expires_at,
                                version: vector_record.version,
                                quantized_vector: if matches!(extraction_mode, ExtractionMode::QuantizedOnly | ExtractionMode::Both) {
                                    vector_record.quantized_vector.clone()
                                } else {
                                    None
                                },
                            };
                            
                            all_vectors.push(output_vector_record);
                            file_vector_count += 1;
                        }
                    }
                    
                    let file_duration = file_start.elapsed();
                    debug!(
                        "[AXIS Consumer] Extracted {} vectors from {} in {:.2}ms",
                        file_vector_count,
                        file_path,
                        file_duration.as_secs_f64() * 1000.0
                    );
                }
            }
            StorageEngineType::VIPER => {
                debug!("[AXIS Consumer] Using Parquet reader for vector extraction");
                
                // Temporarily disabled due to arrow-arith compilation conflicts - TODO: Re-enable when resolved
                // use arrow_array::{Array, StringArray, Int64Array, Float32Array};
                // use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
                
                for file_path in files {
                    debug!("[AXIS Consumer] Reading Parquet file: {}", file_path);
                    let file_start = std::time::Instant::now();
                    
                    // Read the Parquet file
                    let fs = filesystem_factory.get_filesystem(file_path)?;
                    let file_data = fs.read(file_path).await
                        .map_err(|e| {
                            error!("[AXIS Consumer] Failed to read Parquet file {}: {}", file_path, e);
                            e
                        })?;
                    
                    // Arrow crates disabled - commenting out parquet processing
                    // let parquet_bytes = bytes::Bytes::from(file_data);
                    // let builder = arrow::parquet::arrow::async_reader::ParquetRecordBatchStreamBuilder::new(parquet_bytes)?;
                    // let mut reader = builder.build()?;
                    
                    let mut file_vector_count = 0;
                    
                    // TODO: Re-enable when arrow crates are restored
                    // Process each record batch
                    // while let Some(batch_result) = reader.next() {
//                         let batch = batch_result?;
//                         
//                         // Extract columns we need
//                         // Arrow disabled - commenting out array operations
//                         // let id_array = batch.column_by_name("id")
//                         //     .and_then(|col| col.as_any().downcast_ref::<StringArray>());
//                         
//                         let vector_array = batch.column_by_name("vector");
//                         let quantized_array = batch.column_by_name("quantized_vector");
//                         
//                         // let version_array = batch.column_by_name("version")
//                         //     .and_then(|col| col.as_any().downcast_ref::<Int64Array>());
//                         
//                         // let timestamp_array = batch.column_by_name("timestamp")
//                         //     .and_then(|col| col.as_any().downcast_ref::<Int64Array>());
//                         
//                         // Process each row in the batch
//                         // TODO: Restore Arrow processing when enabled
//                         /*
//                         for row_idx in 0..batch.num_rows() {
//                             // Extract ID
//                             let id = id_array
//                                 .and_then(|arr| {
//                                     if arr.is_null(row_idx) {
//                                         None
//                                     } else {
//                                         Some(arr.value(row_idx).to_string())
//                                     }
//                                 })
//                                 .unwrap_or_else(|| format!("row_{}", row_idx));
//                         */
//                             
//                             /*
//                             // Check extraction mode
//                             let has_fp32 = vector_array.is_some() && !vector_array.unwrap().is_null(row_idx);
//                             let has_quantized = quantized_array.is_some() && !quantized_array.unwrap().is_null(row_idx);
//                             
//                             let should_extract = match extraction_mode {
//                                 ExtractionMode::Fp32Only => has_fp32,
//                                 ExtractionMode::QuantizedOnly => has_quantized,
//                                 ExtractionMode::Both => has_fp32 || has_quantized,
//                             };
//                             
//                             if should_extract {
//                                 // Extract vector data based on mode
//                                 let vector = if extraction_mode != ExtractionMode::QuantizedOnly && has_fp32 {
//                                     // Extract FP32 vector from List<Float32> column
//                                     // Arrow disabled - using stub check
//                                     // if let Some(list_array) = vector_array
//                                     //     .and_then(|col| col.as_any().downcast_ref::<arrow_array::ListArray>()) {
//                                     if false { // Stub condition since Arrow is disabled
//                                         if !list_array.is_null(row_idx) {
//                                             let values = list_array.value(row_idx);
//                                             if let Some(float_array) = values.as_any().downcast_ref::<Float32Array>() {
//                                                 (0..float_array.len())
//                                                     .map(|i| float_array.value(i))
//                                                     .collect()
//                                             } else {
//                                                 vec![]
//                                             }
//                                         } else {
//                                             vec![]
//                                         }
//                                     } else {
//                                         vec![]
//                                     }
//                                 } else {
//                                     vec![]
//                                 };
//                                 
//                                 // Create VectorRecord
//                                 let vector_record = crate::proto::proximadb::VectorRecord {
//                                     id: Some(id),
//                                     vector,
//                                     metadata: vec![], // Metadata extraction would be more complex
//                                     version: version_array
//                                         .and_then(|arr| {
//                                             if arr.is_null(row_idx) {
//                                                 None
//                                             } else {
//                                                 Some(arr.value(row_idx) as u32)
//                                             }
//                                         }),
//                                     timestamp: timestamp_array
//                                         .and_then(|arr| {
//                                             if arr.is_null(row_idx) {
//                                                 None
//                                             } else {
//                                                 Some(arr.value(row_idx) as u32)
//                                             }
//                                         })
//                                         ,
//                                     updated_at: None,
//                                     expires_at: None,
//                                     // rank removed -  None,
//                                     similarity: None,
//                                     similarity: None,
//                                 };
//                                 
//                                 all_vectors.push(vector_record);
//                                 file_vector_count += 1;
//                             }
                        // }
                    // End of disabled arrow processing block
                    
                    let file_duration = file_start.elapsed();
                    debug!(
                        "[AXIS Consumer] Extracted {} vectors from {} in {:.2}ms",
                        file_vector_count,
                        file_path,
                        file_duration.as_secs_f64() * 1000.0
                    );
                }
            }
        }
        
        let duration = start_time.elapsed();
        
        if all_vectors.is_empty() {
            warn!(
                "[AXIS Consumer] Vector extraction returned empty after {:.2}ms (implementation pending)",
                duration.as_secs_f64() * 1000.0
            );
        } else {
            info!(
                "[AXIS Consumer] Vector extraction completed in {:.2}ms: extracted {} vectors from {} files",
                duration.as_secs_f64() * 1000.0,
                all_vectors.len(),
                files.len()
            );
        }
        
        Ok(all_vectors)
    }
}

/// Start the AXIS EventLog consumer
pub async fn start_axis_consumer(
    event_log: Arc<dyn EventLogService>,
    axis_manager: Arc<AxisManager>,
    filesystem_factory: Arc<FilesystemFactory>,
    collection_cache: Arc<DashMap<String, Arc<Collection>>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let config = ConsumerConfig::default();
    
    let consumer = AxisEventLogConsumer::new(
        config,
        event_log,
        axis_manager,
        filesystem_factory,
        collection_cache,
        shutdown,
    );
    
    tokio::spawn(async move {
        consumer.run().await;
    })
}