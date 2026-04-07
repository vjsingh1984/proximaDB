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

use crate::storage::persistence::filesystem::{FileOptions, FileSystem, FsResult, TempStrategy};

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

        Ok(MetadataWriteStrategy { temp_strategy })
    }
}

/// Metadata-optimized write strategy
pub struct MetadataWriteStrategy {
    temp_strategy: TempStrategy,
}

impl MetadataWriteStrategy {
    /// Create optimized file options for atomic writes.
    /// Always uses temp+rename for metadata files to prevent corruption on crash.
    /// A power failure during a direct truncate+write would leave a partially-written
    /// or empty file. Writing to a temp file then renaming is atomic at the filesystem level.
    pub fn create_file_options(
        &self,
        fs: &dyn FileSystem,
        final_path: &str,
    ) -> FsResult<FileOptions> {
        let temp_path = Some(fs.generate_temp_path(final_path, &self.temp_strategy)?);

        Ok(FileOptions {
            create_dirs: true,
            overwrite: true,
            temp_path,
            ..Default::default()
        })
    }
}
