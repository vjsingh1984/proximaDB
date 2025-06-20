/*
 * Copyright 2025 Vijaykumar Singh
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

//! Write Strategy Factory - Optimized Write Patterns for Different Use Cases

use crate::storage::filesystem::{FileSystem, FileOptions, FsResult, TempStrategy};

/// Write strategy factory for different optimization patterns
pub struct WriteStrategyFactory;

impl WriteStrategyFactory {
    /// Create metadata-optimized write strategy
    pub fn create_metadata_strategy(
        _fs: &dyn FileSystem,
        temp_directory: Option<&str>,
    ) -> FsResult<MetadataWriteStrategy> {
        let temp_strategy = if let Some(temp_dir) = temp_directory {
            TempStrategy::ConfiguredTemp {
                temp_dir: Some(temp_dir.to_string()),
            }
        } else {
            TempStrategy::SameDirectory
        };
        
        Ok(MetadataWriteStrategy {
            temp_strategy,
        })
    }
}

/// Metadata-optimized write strategy
pub struct MetadataWriteStrategy {
    temp_strategy: TempStrategy,
}

impl MetadataWriteStrategy {
    /// Create optimized file options for atomic writes
    pub fn create_file_options(
        &self,
        fs: &dyn FileSystem,
        final_path: &str,
    ) -> FsResult<FileOptions> {
        let temp_path = if fs.supports_atomic_writes() {
            // Direct write for local filesystem
            None
        } else {
            // Use temp strategy for object stores
            Some(fs.generate_temp_path(final_path, &self.temp_strategy)?)
        };
        
        Ok(FileOptions {
            create_dirs: true,
            overwrite: true,
            temp_path,
            ..Default::default()
        })
    }
}