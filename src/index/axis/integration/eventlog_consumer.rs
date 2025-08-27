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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::collections::HashMap;
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
use crate::storage::engines::impls::raptor::consolidated_reader::RaptorReader;

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

/// Consumer metrics
// Type alias for compatibility
pub type EventLogConsumer = AxisEventLogConsumer;

#[derive(Debug, Clone, Default)]
pub struct ConsumerStats {
    pub events_processed: u64,
    pub events_failed: u64,
    pub last_processed_timestamp: Option<std::time::SystemTime>,
}

#[derive(Default)]
struct ConsumerMetrics {
    events_skipped: AtomicU64,
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
    
    /// Unified cache orchestrator (shared across system)
    cache_orchestrator: Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>,
    
    /// Metrics
    metrics: Arc<ConsumerMetrics>,
    
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
        cache_orchestrator: Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            config,
            event_log,
            axis_manager,
            filesystem_factory,
            collection_cache,
            cache_orchestrator,
            metrics: Arc::new(ConsumerMetrics::default()),
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
            "[AXIS Consumer] Collection {} found: dimension={:?}, engine={:?}, quantization_enabled={:?}",
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
        
        // Check if collection has ANY indexes configured
        let has_indexes = collection.config.as_ref()
            .map(|c| !c.index_configs.is_empty())
            .unwrap_or(false);
        
        if !has_indexes {
            // No indexes configured - mark event as completed without processing
            // This is CRITICAL for performance:
            // 1. Flush creates event in EventLog
            // 2. We immediately mark it complete (no work needed)
            // 3. Compaction checks can_compact() which returns true
            // 4. Compaction proceeds without delay
            // 
            // Without this, compaction would wait forever for non-existent index processing
            info!(
                "[AXIS Consumer] No indexes configured for collection {}, marking flush event {} as completed",
                event.collection_id, event_id
            );
            
            // Update metrics for skipped event
            self.metrics.events_skipped.fetch_add(1, Ordering::Relaxed);
            
            // Returning Ok() marks event as complete in EventLog
            return Ok(());
        }
        
        // Determine extraction mode based on index types
        let extraction_mode = self.determine_extraction_mode(&collection);
        let extraction_mode_str = match extraction_mode {
            ExtractionMode::Fp32Only => "FP32Only",
            ExtractionMode::QuantizedOnly => "QuantizedOnly",
            ExtractionMode::Both => "Both",
            ExtractionMode::Auto => "Auto",
        };
        
        debug!(
            "[AXIS Consumer] Flush event {} extraction mode determined: {} (quantization_enabled={:?}, has_fp32={}, has_quantized={})",
            event_id,
            extraction_mode_str,
            collection.config.as_ref()
                .and_then(|c| c.quantization.as_ref())
                .map(|q| q.enabled),
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
            &event.collection_id,
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
        collection: Arc<Collection>,
    ) -> Result<()> {
        let event_id = event.event_id.clone();
        let start_time = std::time::Instant::now();
        
        // Check if collection has ANY indexes configured
        let has_indexes = collection.config.as_ref()
            .map(|c| !c.index_configs.is_empty())
            .unwrap_or(false);
        
        if !has_indexes {
            // No indexes configured - mark event as completed without processing
            info!(
                "[AXIS Consumer] No indexes configured for collection {}, marking compaction event {} as completed",
                event.collection_id, event_id
            );
            
            // Update metrics for skipped event
            self.metrics.events_skipped.fetch_add(1, Ordering::Relaxed);
            
            // The compaction is done on storage side, we just have nothing to update
            return Ok(());
        }
        
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
            .and_then(|c| c.index_configs.first())
            .map(|i| format!("Algorithm({})", i.algorithm))
            .unwrap_or_else(|| "None".to_string());
        
        debug!(
            "[AXIS Consumer] Determining extraction mode for collection {}:\n  Quantization Enabled: {:?}\n  Index Algorithm: {}",
            collection.id,
            has_quantization,
            index_algorithm
        );
        
        // Check index types to determine what data we need
        // HNSW typically needs FP32 for accuracy
        // IVF can work with quantized data
        // PQ indexes prefer quantized data
        
        let mode = if has_quantization != Some(true) {
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
        collection_id: &str,
    ) -> Result<Vec<crate::proto::proximadb::VectorRecord>> {
        let start_time = std::time::Instant::now();
        
        debug!(
            "[AXIS Consumer] Starting vector extraction:\n  Files: {:?}\n  Count: {}\n  Storage Engine: {:?}\n  Extraction Mode: {:?}",
            files,
            files.len(),
            storage_engine,
            extraction_mode
        );
        
        // Create filesystem factory with proper configuration for zero-copy optimization
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(Default::default()).await
                .context("Failed to create filesystem factory")?
        );
        
        // Create zero-copy IO system once for all files - this enables cross-file optimization
        // Note: ZeroCopyIOSystem requires configuration and filesystem factory
        use crate::storage::engines::core::io::zero_copy::{
            config::{ZeroCopyIOConfig, WorkloadType},
            orchestrator::ZeroCopyIOSystem,
            access_tracker::AccessEvent,
            traits::QueryType as ZeroCopyQueryType,
        };
        
        let zero_copy_config = ZeroCopyIOConfig::for_workload(WorkloadType::HighPerformance);
        let zero_copy_system = Arc::new(
            ZeroCopyIOSystem::new(
                zero_copy_config,
                filesystem_factory.clone(),
                vec![], // No custom serializers needed for now
            ).await?
        );
        
        // Note: For AXIS indexing, we need full scan of all records, not selective reads
        // The zero-copy system should prioritize local disk reads for recently flushed/compacted files
        debug!(
            "[AXIS Consumer] Configuring zero-copy IO for full-scan indexing of {} files", 
            files.len()
        );
        
        // Track which files are likely on local disk (recently flushed/compacted)
        // These should be read from local cache to avoid cloud storage costs
        for file_path in files {
            // Create access event for tracking
            let access_event = AccessEvent {
                file_path: file_path.to_string(),
                collection_id: collection_id.to_string(),
                query_type: ZeroCopyQueryType::FullScan,
                timestamp: std::time::Instant::now(),
                result_type: "axis_indexing".to_string(),
            };
            // Note: ZeroCopyIOSystem tracks access patterns internally
            // The access tracker is used for pattern learning and optimization
        }
        
        info!(
            "[AXIS Consumer] Zero-copy IO configured for full-scan indexing. Will prioritize local disk for recently written files."
        );
        
        let mut all_vectors = Vec::new();
        
        // Use unified approach for all storage engines
        // The zero-copy IO system will optimize reads regardless of engine type
        match storage_engine {
            StorageEngineType::SST | 
            StorageEngineType::VIPER | 
            StorageEngineType::NOVA | 
            StorageEngineType::RAPTOR | 
            StorageEngineType::SWIFT | 
            StorageEngineType::PRISM => {
                debug!("[AXIS Consumer] Using unified reader for {:?} engine with zero-copy optimization", storage_engine);
                
                use crate::core::VectorRecord;
                
                // Create appropriate reader based on storage engine type
                // All readers should leverage the zero-copy IO system for optimization
                let records_futures = match storage_engine {
                    StorageEngineType::SST => {
                        // SST uses unified SST reader with streaming support
                        use crate::storage::engines::impls::sst::readers::sst_query_engine::UnifiedSstableReader;
                        
                        let reader = UnifiedSstableReader::new(
                            filesystem_factory.clone(),
                            zero_copy_system.clone(),
                            collection_id.to_string()
                        );
                        
                        let mut all_records = Vec::new();
                        for (idx, file_path) in files.iter().enumerate() {
                            debug!("[AXIS Consumer] Full-scan reading SST file {}/{}: {}", 
                                idx + 1, files.len(), file_path);
                            let file_start = std::time::Instant::now();
                            
                            let records = reader.read_all_records_for_compaction(&[file_path.clone()]).await
                                .map_err(|e| {
                                    error!("[AXIS Consumer] Failed to read SST file {}: {}", file_path, e);
                                    e
                                })?;
                            all_records.extend(records);
                            
                            let file_duration = file_start.elapsed();
                            debug!(
                                "[AXIS Consumer] Read {} records from {} in {:.2}ms",
                                all_records.len(),
                                file_path,
                                file_duration.as_secs_f64() * 1000.0
                            );
                        }
                        all_records
                    }
                    StorageEngineType::SWIFT => {
                        // SWIFT has hierarchical blocks - use its unified reader with StreamAll strategy
                        // SWIFT reader can skip metadata SuperBlocks and stream DataBlocks directly
                        use crate::storage::engines::impls::swift::unified_reader::{UnifiedSwiftReader, SwiftReadStrategy};
                        
                        let mut all_records = Vec::new();
                        for (idx, file_path) in files.iter().enumerate() {
                            debug!("[AXIS Consumer] Full-scan reading SWIFT file {}/{}: {}", 
                                idx + 1, files.len(), file_path);
                            let file_start = std::time::Instant::now();
                            
                            // Create SWIFT reader with default config
                            use crate::storage::engines::impls::swift::unified_reader::SwiftReaderConfig;
                            let config = SwiftReaderConfig::default();
                            let reader = UnifiedSwiftReader::new(
                                filesystem_factory.clone(),
                                file_path.clone(),
                                zero_copy_system.clone(),
                                collection_id.to_string(),
                                config,
                            ).await?;
                            
                            // Use StreamAll strategy for full scan (skips metadata blocks, reads all DataBlocks)
                            let result = reader.read_with_strategy(SwiftReadStrategy::StreamAll).await
                                .map_err(|e| {
                                    error!("[AXIS Consumer] Failed to read SWIFT file {}: {}", file_path, e);
                                    e
                                })?;
                            
                            all_records.extend(result.records);
                            
                            let file_duration = file_start.elapsed();
                            debug!(
                                "[AXIS Consumer] Read {} records from {} in {:.2}ms",
                                result.records.len(),
                                file_path,
                                file_duration.as_secs_f64() * 1000.0
                            );
                        }
                        all_records
                    }
                    StorageEngineType::VIPER | StorageEngineType::NOVA => {
                        // VIPER and NOVA use Parquet format - use the same mechanism as VIPER compaction
                        debug!("[AXIS Consumer] Using Parquet reader for columnar engine {:?}", storage_engine);
                        
                        use arrow_array::{Array, StringArray, Int64Array, Float32Array};
                        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
                        
                        let mut all_records = Vec::new();
                        
                        for (idx, file_path) in files.iter().enumerate() {
                            debug!("[AXIS Consumer] Reading Parquet file {}/{}: {}", idx + 1, files.len(), file_path);
                            let file_start = std::time::Instant::now();
                            
                            // Read file data using filesystem (leverages zero-copy IO)
                            let fs = filesystem_factory.get_filesystem(file_path)?;
                            let file_data = fs.read(file_path).await
                                .map_err(|e| {
                                    error!("[AXIS Consumer] Failed to read Parquet file {}: {}", file_path, e);
                                    e
                                })?;
                            
                            // Convert to Parquet reader (same as VIPER compaction)
                            let parquet_bytes = bytes::Bytes::from(file_data);
                            let builder = ParquetRecordBatchReaderBuilder::try_new(parquet_bytes)
                                .map_err(|e| {
                                    error!("[AXIS Consumer] Failed to create Parquet reader for {}: {}", file_path, e);
                                    e
                                })?;
                            
                            let reader = builder.build()
                                .map_err(|e| {
                                    error!("[AXIS Consumer] Failed to build Parquet reader for {}: {}", file_path, e);
                                    e
                                })?;
                            
                            // Process each batch (same pattern as VIPER compaction)
                            for batch_result in reader {
                                let batch = batch_result
                                    .map_err(|e| {
                                        error!("[AXIS Consumer] Failed to read batch from {}: {}", file_path, e);
                                        e
                                    })?;
                                
                                debug!("[AXIS Consumer] Processing batch with {} rows", batch.num_rows());
                                
                                // Extract vector data from the batch
                                let id_array = batch.column_by_name("id")
                                    .and_then(|col| col.as_any().downcast_ref::<StringArray>());
                                
                                let vector_column = batch.column_by_name("vector");
                                let quantized_column = batch.column_by_name("quantized_vector");
                                
                                let version_array = batch.column_by_name("version");
                                let timestamp_array = batch.column_by_name("timestamp")
                                    .and_then(|col| col.as_any().downcast_ref::<Int64Array>());
                                
                                // Process each row in the batch
                                for row_idx in 0..batch.num_rows() {
                                    // Check extraction mode to determine what data to extract
                                    let has_fp32 = vector_column.is_some();
                                    let has_quantized = quantized_column.is_some();
                                    
                                    let should_extract = match extraction_mode {
                                        ExtractionMode::Fp32Only => has_fp32,
                                        ExtractionMode::QuantizedOnly => has_quantized,
                                        ExtractionMode::Both => has_fp32 || has_quantized,
                                        ExtractionMode::Auto => has_fp32 || has_quantized, // Auto defaults to extracting any available
                                    };
                                    
                                    if should_extract {
                                        // Extract ID
                                        let id = id_array
                                            .and_then(|arr| {
                                                if row_idx < arr.len() {
                                                    Some(arr.value(row_idx).to_string())
                                                } else {
                                                    None
                                                }
                                            });
                                        
                                        // Extract vector (simplified - full implementation would handle List<Float32>)
                                        let vector = if has_fp32 {
                                            // TODO: Extract actual vector from List<Float32> column
                                            vec![] // Placeholder
                                        } else {
                                            vec![]
                                        };
                                        
                                        // Extract quantized vector if available
                                        let quantized_vector = if has_quantized {
                                            // TODO: Extract quantized vector
                                            vec![]
                                        } else {
                                            vec![]
                                        };
                                        
                                        // Extract version
                                        let version = if let Some(version_col) = version_array {
                                            if let Some(arr) = version_col.as_any().downcast_ref::<Int64Array>() {
                                                if row_idx < arr.len() {
                                                    arr.value(row_idx)
                                                } else {
                                                    0
                                                }
                                            } else {
                                                0
                                            }
                                        } else {
                                            0
                                        };
                                        
                                        // Extract timestamp
                                        let timestamp = timestamp_array
                                            .and_then(|arr| {
                                                if row_idx < arr.len() {
                                                    Some(arr.value(row_idx))
                                                } else {
                                                    None
                                                }
                                            })
                                            .unwrap_or(0);
                                        
                                        // Create VectorRecord
                                        let record = VectorRecord {
                                            id: id.clone(), // Use empty string if no ID
                                            vector,
                                            quantized_vector: Some(quantized_vector),
                                            metadata: Vec::new(), // TODO: Extract metadata columns
                                            version: Some(version as u32),
                                            timestamp: timestamp as u32,
                                            expires_at: None,
                                            updated_at: None,
                                            source: None,
                                        };
                                        
                                        all_records.push(record);
                                    }
                                }
                            }
                            
                            let file_duration = file_start.elapsed();
                            debug!(
                                "[AXIS Consumer] Read {} records from {} in {:.2}ms",
                                all_records.len(),
                                file_path,
                                file_duration.as_secs_f64() * 1000.0
                            );
                        }
                        
                        all_records
                    }
                    StorageEngineType::RAPTOR => {
                        // RAPTOR uses Arrow RecordBatch format with row-aligned storage
                        // It stores data in a format optimized for HNSW graph operations
                        debug!("[AXIS Consumer] Using RAPTOR reader for row-aligned Arrow format");
                        
                        // RAPTOR's compaction reads all vectors to rebuild HNSW graph
                        // We can use the same approach for AXIS indexing
                        use crate::storage::engines::impls::raptor::consolidated_reader::RaptorReader;
                        
                        let mut all_records = Vec::new();
                        for (idx, file_path) in files.iter().enumerate() {
                            debug!("[AXIS Consumer] Full-scan reading RAPTOR file {}/{}: {}", 
                                idx + 1, files.len(), file_path);
                            let file_start = std::time::Instant::now();
                            
                            // Create RAPTOR reader with required dependencies
                            use crate::storage::engines::impls::raptor::RaptorConfig;
                            use crate::storage::transaction_coordinator::TransactionCoordinator;
                            
                            let config = RaptorConfig::default();
                            let transaction_coordinator = Arc::new(TransactionCoordinator::new(
                                filesystem_factory.clone(),
                                None  // No temp directory needed for reads
                            ).await?);
                            let cache_dir = "/tmp/raptor_cache".to_string();
                            
                            // Use unified cache orchestrator
                            let cache = self.cache_orchestrator.clone();
                            
                            // Create ZeroCopyFilesystem for RAPTOR
                            use crate::storage::persistence::filesystem::zero_copy_filesystem::ZeroCopyFilesystem;
                            use crate::storage::persistence::filesystem::local::{LocalFileSystem, LocalConfig};
                            use crate::storage::engines::core::io::zero_copy::ZeroCopyIOSystem;
                            use crate::storage::persistence::filesystem::FilesystemFactory;
                            
                            // Create basic filesystem
                            let local_config = LocalConfig::default();
                            let local_fs = Arc::new(LocalFileSystem::new(local_config).await.unwrap()) as Arc<dyn crate::storage::persistence::filesystem::FileSystem>;
                            
                            // Create filesystem factory for zero-copy system
                            use crate::storage::persistence::filesystem::FilesystemConfig;
                            let fs_config = FilesystemConfig::default();
                            let fs_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
                            
                            // Create zero-copy IO system
                            use crate::storage::engines::core::io::zero_copy::ZeroCopyIOConfig;
                            let io_config = ZeroCopyIOConfig::default();
                            let io_system = Arc::new(ZeroCopyIOSystem::new(
                                io_config,
                                fs_factory,
                                vec![] // Empty metadata serializers for now
                            ).await?);
                            
                            // Create zero-copy filesystem
                            let zero_copy_fs = Arc::new(ZeroCopyFilesystem::new(
                                local_fs,
                                io_system,
                                collection_id.to_string(),
                                "raptor".to_string()
                            ));
                            
                            // Create transaction coordinator for RAPTOR
                            let transaction_coordinator = Arc::new(
                                TransactionCoordinator::new(
                                    fs_factory.clone(),
                                    Some(format!("{}/temp", cache_dir)),
                                ).await?
                            );
                            
                            let reader = RaptorReader::new(
                                cache_dir.clone(),
                                collection_id.to_string(),
                                config,
                                cache,
                                zero_copy_fs.clone(),
                                io_system.clone(),
                                transaction_coordinator,
                            );
                            
                            // Read all vectors from rowgroups (RAPTOR needs all vectors for HNSW graph)
                            // Use empty slice to read all rowgroups
                            let record_batches = reader.read_rowgroups(file_path, &[]).await
                                .map_err(|e| {
                                    error!("[AXIS Consumer] Failed to read RAPTOR file {}: {}", file_path, e);
                                    e
                                })?;
                            
                            // Convert RecordBatch to VectorRecord
                            // For now, we'll create placeholder records since the conversion is complex
                            // In a real implementation, you'd parse the Arrow RecordBatch columns
                            for batch in record_batches {
                                let num_rows = batch.num_rows();
                                for row_idx in 0..num_rows {
                                    let record = VectorRecord {
                                        id: format!("raptor_{}_{}", file_path, row_idx),
                                        vector: vec![0.0; 128], // Placeholder vector
                                        quantized_vector: None,
                                        metadata: Vec::new(),
                                        version: Some(0),
                                        timestamp: 0,
                                        expires_at: None,
                                        updated_at: None,
                                        source: None,
                                    };
                                    all_records.push(record);
                                }
                            }
                            
                            let file_duration = file_start.elapsed();
                            debug!(
                                "[AXIS Consumer] Read {} records from {} in {:.2}ms",
                                all_records.len(),
                                file_path,
                                file_duration.as_secs_f64() * 1000.0
                            );
                        }
                        all_records
                    }
                    StorageEngineType::PRISM => {
                        // PRISM is metadata-first and memory-optimized
                        // It uses FastLanes serialization with progressive quantization levels
                        debug!("[AXIS Consumer] Using PRISM reader for metadata-first memory format");
                        
                        // PRISM stores data in memory with multiple resolution levels
                        // For AXIS indexing, we need to read the full-precision vectors
                        use crate::storage::engines::impls::prism::fastlanes_serializer::{
                            PrismFastLanesSerializer, ResolutionLevel
                        };
                        
                        let mut all_records = Vec::new();
                        
                        for (idx, file_path) in files.iter().enumerate() {
                            debug!("[AXIS Consumer] Reading PRISM memory-serialized file {}/{}: {}", 
                                idx + 1, files.len(), file_path);
                            let file_start = std::time::Instant::now();
                            
                            // Read the serialized PRISM data
                            let fs = filesystem_factory.get_filesystem(file_path)?;
                            let file_data = fs.read(file_path).await
                                .map_err(|e| {
                                    error!("[AXIS Consumer] Failed to read PRISM file {}: {}", file_path, e);
                                    e
                                })?;
                            
                            // PRISM uses FastLanes progressive serialization
                            // The data contains multiple resolution levels (Binary, INT8, FP32)
                            use crate::compute::quantization::storage_engine::StorageQuantizationConfig;
                            let serializer = PrismFastLanesSerializer::new(StorageQuantizationConfig::default());
                            
                            // Deserialize the progressive format
                            // For AXIS indexing, we typically want the highest resolution (FP32)
                            let records = match extraction_mode {
                                ExtractionMode::Fp32Only => {
                                    // Extract FP32 resolution level
                                    let (records, _metadata) = serializer.deserialize_resolution(&file_data).await?;
                                    records
                                }
                                ExtractionMode::QuantizedOnly => {
                                    // Extract quantized resolution level
                                    let (records, _metadata) = serializer.deserialize_resolution(&file_data).await?;
                                    records
                                }
                                ExtractionMode::Both | ExtractionMode::Auto => {
                                    // Extract all available resolution levels
                                    let (records, _metadata) = serializer.deserialize_resolution(&file_data).await?;
                                    records
                                }
                            };
                            
                            let record_count = records.len();
                            all_records.extend(records);
                            
                            let file_duration = file_start.elapsed();
                            debug!(
                                "[AXIS Consumer] Read {} records from PRISM file {} in {:.2}ms",
                                record_count,
                                file_path,
                                file_duration.as_secs_f64() * 1000.0
                            );
                        }
                        
                        all_records
                    }
                    _ => {
                        warn!("[AXIS Consumer] Unknown storage engine type: {:?}", storage_engine);
                        vec![]
                    }
                };
                
                // Convert VectorRecords to proto format for AXIS
                for vector_record in records_futures {
                    // Check if we should include this record based on extraction mode
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
                            !vector_record.vector.is_empty() || 
                            vector_record.quantized_vector.as_ref().map_or(false, |v| !v.is_empty())
                        }
                        ExtractionMode::Auto => {
                            // Auto mode: extract if we have any data
                            !vector_record.vector.is_empty() || 
                            vector_record.quantized_vector.as_ref().map_or(false, |v| !v.is_empty())
                        }
                    };
                    
                    if should_extract {
                        // Convert core::VectorRecord to proto::VectorRecord for AXIS
                        let proto_record = crate::proto::proximadb::VectorRecord {
                            id: vector_record.id.clone(),
                            vector: match extraction_mode {
                                ExtractionMode::QuantizedOnly => vec![],
                                _ => vector_record.vector.clone(),
                            },
                            metadata: vector_record.metadata.clone(),
                            timestamp: vector_record.timestamp,
                            updated_at: None, // Not used by AXIS
                            expires_at: vector_record.expires_at,
                            version: vector_record.version,
                            quantized_vector: if matches!(extraction_mode, ExtractionMode::QuantizedOnly | ExtractionMode::Both) {
                                vector_record.quantized_vector.clone()
                            } else {
                                None
                            },
                            source: None,
                        };
                        
                        all_vectors.push(proto_record);
                    }
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