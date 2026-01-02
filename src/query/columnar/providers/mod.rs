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

//! ColumnarReadProvider implementations
//!
//! This module contains concrete implementations of the `ColumnarReadProvider` trait:
//!
//! - `ArrowInMemoryProvider`: Zero-cost access to cached RecordBatches
//! - `ParquetRangePrunedProvider`: Optimized Parquet I/O with predicate pushdown

pub mod arrow_memory;
pub mod parquet_pruned;

pub use arrow_memory::ArrowInMemoryProvider;
pub use parquet_pruned::ParquetRangePrunedProvider;
