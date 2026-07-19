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

//! # ProximaDB AXIS Index Vector Storage
//!
//! Concrete packed / zero-copy vector storage representations for AXIS index
//! implementations, extracted from the root crate (`src/index/axis/`) as part
//! of the root-crate decomposition track.
//!
//! - [`zero_overhead_vector`]: collection-level-config-cached vector storage
//!   (no per-vector metadata) for maximal density.
//! - [`compact_vector`]: packed id+vector representation enabling zero-copy
//!   access patterns.

// Crate-level lint allowances mirror the root crate (this code was extracted
// verbatim from src/index/axis and was written under those suppressions).
#![allow(clippy::missing_docs_in_private_items)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::result_large_err)]

pub mod compact_vector;
pub mod zero_overhead_vector;
