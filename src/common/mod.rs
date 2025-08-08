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

//! Common infrastructure shared across ProximaDB components
//!
//! This module provides low-level infrastructure that can be reused by:
//! - Index implementations (for organizing vector data)
//! - Cache systems (for storing computed results) 
//! - Storage engines (for persistent data)
//! - Network services (for request/response handling)

pub mod concurrent_structures;
pub mod tier_policy_engine;
pub mod adaptive_structures;

pub use concurrent_structures::{
    ConcurrentStorage, ConcurrentMapping, AtomicMetrics, MetricsSnapshot,
    TypedStorage, AccessInfo
};

pub use tier_policy_engine::{
    GlobalTierManager, RuleBasedTierPolicy, ServerTierConfig, 
    SmartTierPolicy, StorageTier, WorkloadPattern, WorkloadMetrics
};

pub use adaptive_structures::{
    AdaptiveStore, UniversalTierManager, AdaptiveStoreFactory, 
    AdaptiveStoreConfig, BackendType, TierRebalanceResult
};