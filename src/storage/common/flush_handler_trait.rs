/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Unified flush handler trait for all storage engines
//! 
//! This trait provides a common interface for checking if files can be compacted
//! and performing compaction operations across all storage engines.

use anyhow::Result;
use async_trait::async_trait;

/// Trait for storage engine flush and compaction handlers
#[async_trait]
pub trait FlushHandler: Send + Sync {
    /// Check if the given files can be compacted
    async fn can_compact_files(&self, collection_id: &str, files: &[String]) -> Result<bool>;
    
    /// Perform compaction on the given files
    async fn compact_files(&self, collection_id: &str, files: &[String], output_path: &str) -> Result<()>;
    
    /// Get the engine name
    fn engine_name(&self) -> &'static str;
}

/// Factory for creating flush handlers based on storage engine type
pub struct FlushHandlerFactory;

impl FlushHandlerFactory {
    /// Create a flush handler for the specified engine type
    pub fn create(engine_type: super::compaction_utils::StorageEngineType) -> Box<dyn FlushHandler> {
        match engine_type {
            super::compaction_utils::StorageEngineType::SST => {
                Box::new(SstFlushHandlerAdapter::new())
            }
            super::compaction_utils::StorageEngineType::VIPER => {
                Box::new(ViperFlushHandlerAdapter::new())
            }
            super::compaction_utils::StorageEngineType::NOVA => {
                Box::new(NovaFlushHandler::new())
            }
            super::compaction_utils::StorageEngineType::SWIFT => {
                Box::new(SwiftFlushHandler::new())
            }
            super::compaction_utils::StorageEngineType::PRISM => {
                Box::new(PrismFlushHandler::new())
            }
            super::compaction_utils::StorageEngineType::RAPTOR => {
                Box::new(RaptorFlushHandler::new())
            }
        }
    }
}

// Adapter for SST engine
struct SstFlushHandlerAdapter {
    inner: crate::storage::engines::impls::sst::flush_eventlog_integration::SstFlushHandler,
}

impl SstFlushHandlerAdapter {
    fn new() -> Self {
        Self {
            inner: crate::storage::engines::impls::sst::flush_eventlog_integration::SstFlushHandler::new(),
        }
    }
}

#[async_trait]
impl FlushHandler for SstFlushHandlerAdapter {
    async fn can_compact_files(&self, collection_id: &str, files: &[String]) -> Result<bool> {
        // SST handler takes &[String] and returns bool
        Ok(self.inner.can_compact_files(collection_id, files).await)
    }
    
    async fn compact_files(&self, _collection_id: &str, _files: &[String], _output_path: &str) -> Result<()> {
        // TODO: Implement actual compaction
        Ok(())
    }
    
    fn engine_name(&self) -> &'static str {
        "SST"
    }
}

// Adapter for VIPER engine  
struct ViperFlushHandlerAdapter {
    inner: crate::storage::engines::impls::viper::eventlog_flush::ViperFlushHandler,
}

impl ViperFlushHandlerAdapter {
    fn new() -> Self {
        Self {
            inner: crate::storage::engines::impls::viper::eventlog_flush::ViperFlushHandler::new(),
        }
    }
}

#[async_trait]
impl FlushHandler for ViperFlushHandlerAdapter {
    async fn can_compact_files(&self, collection_id: &str, files: &[String]) -> Result<bool> {
        // ViperFlushNotifier's can_compact_files returns bool
        Ok(self.inner.can_compact_files(collection_id, files).await)
    }
    
    async fn compact_files(&self, _collection_id: &str, _files: &[String], _output_path: &str) -> Result<()> {
        // TODO: Implement actual compaction
        Ok(())
    }
    
    fn engine_name(&self) -> &'static str {
        "VIPER"
    }
}

// Nova engine handler
struct NovaFlushHandler;

impl NovaFlushHandler {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FlushHandler for NovaFlushHandler {
    async fn can_compact_files(&self, _collection_id: &str, _files: &[String]) -> Result<bool> {
        // Nova uses columnar format similar to VIPER
        // For now, all files are compactable
        Ok(true)
    }
    
    async fn compact_files(&self, _collection_id: &str, _files: &[String], _output_path: &str) -> Result<()> {
        // TODO: Implement Nova-specific compaction
        Ok(())
    }
    
    fn engine_name(&self) -> &'static str {
        "NOVA"
    }
}

// Swift engine handler
struct SwiftFlushHandler;

impl SwiftFlushHandler {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FlushHandler for SwiftFlushHandler {
    async fn can_compact_files(&self, _collection_id: &str, _files: &[String]) -> Result<bool> {
        // Swift uses row-based format with superblocks
        // For now, all files are compactable
        Ok(true)
    }
    
    async fn compact_files(&self, _collection_id: &str, _files: &[String], _output_path: &str) -> Result<()> {
        // TODO: Implement Swift-specific compaction
        Ok(())
    }
    
    fn engine_name(&self) -> &'static str {
        "SWIFT"
    }
}

// Prism engine handler
struct PrismFlushHandler;

impl PrismFlushHandler {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FlushHandler for PrismFlushHandler {
    async fn can_compact_files(&self, _collection_id: &str, _files: &[String]) -> Result<bool> {
        // Prism uses memory-optimized LSM tree
        // For now, all files are compactable
        Ok(true)
    }
    
    async fn compact_files(&self, _collection_id: &str, _files: &[String], _output_path: &str) -> Result<()> {
        // TODO: Implement Prism-specific compaction
        Ok(())
    }
    
    fn engine_name(&self) -> &'static str {
        "PRISM"
    }
}

// Raptor engine handler
struct RaptorFlushHandler;

impl RaptorFlushHandler {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FlushHandler for RaptorFlushHandler {
    async fn can_compact_files(&self, _collection_id: &str, _files: &[String]) -> Result<bool> {
        // Raptor uses adaptive matrix format
        // For now, all files are compactable
        Ok(true)
    }
    
    async fn compact_files(&self, _collection_id: &str, _files: &[String], _output_path: &str) -> Result<()> {
        // TODO: Implement Raptor-specific compaction
        Ok(())
    }
    
    fn engine_name(&self) -> &'static str {
        "RAPTOR"
    }
}