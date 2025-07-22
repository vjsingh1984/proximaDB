/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Storage optimization utilities
//! 
//! Provides optimizations for flush and compaction operations to improve
//! compression ratios and query performance.

pub mod metadata_sorter;

pub use metadata_sorter::{MetadataSorter, MetadataSortConfig, SortConfigBuilder, SortingStats};