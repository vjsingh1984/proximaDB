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

//! # Auto-Tiering Policy Engine
//!
//! **Status**: Policy engine implemented. SST integration module available (opt-in).
//!
//! The tiering policy engine provides data movement logic between storage tiers.
//! Storage engine integration is available through the SST tiering integration module.
//!
//! ## What Works
//! - Policy definition and evaluation
//! - Access pattern tracking
//! - Migration task generation
//! - Retention policy management
//! - **SST Integration Module** (`storage::engines::sst::tiering_integration`)
//!
//! ## SST Tiering Integration
//!
//! The SST engine has a tiering integration module that provides:
//! - Access tracking hooks for the search path
//! - Flush tier determination for new data placement
//! - Compaction-time tier evaluation
//! - Policy-based migration task generation
//!
//! Enable via configuration:
//! ```toml
//! [storage.sst.tiering]
//! enabled = true
//! evaluation_interval_secs = 300
//! cold_age_threshold_days = 7
//! hot_access_threshold = 100
//! ```
//!
//! ## Remaining Integration Work
//!
//! The following work is documented for future implementation:
//! 1. Wire access tracking into SST search path (call `record_access()`)
//! 2. Wire tier determination into SST flush path (call `determine_flush_tier()`)
//! 3. Wire tier evaluation into SST compaction (call `evaluate_collection()`)
//! 4. Implement actual data movement between tier storage locations
//!
//! See `src/storage/engines/impls/sst/tiering_integration.rs` for detailed deferred comments.
//!
//! ---
//!
//! ## Legacy Documentation (Archived)
//!
//! Provides automatic data movement between storage tiers based on configurable policies.
//! Supports hot/warm/cold/archive tiers with rules based on:
//! - Age (time since last modification)
//! - Access patterns (frequency, recency)
//! - Size thresholds
//! - Cost optimization
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     TieringPolicyEngine                          │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
//! │  │   Policies  │  │  Evaluator  │  │  Migration Coordinator  │ │
//! │  │   (Rules)   │  │  (Scoring)  │  │     (Async Moves)       │ │
//! │  └──────┬──────┘  └──────┬──────┘  └───────────┬─────────────┘ │
//! ├─────────┼─────────────────┼─────────────────────┼───────────────┤
//! │         │                 │                     │               │
//! │   ┌─────▼─────┐     ┌─────▼─────┐         ┌─────▼─────┐        │
//! │   │ AccessLog │     │ TierCost  │         │ EnginePool │       │
//! │   │  Tracker  │     │ Calculator│         │ (SST,VIPER)│       │
//! │   └───────────┘     └───────────┘         └───────────┘        │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::tiering::{TieringPolicyEngine, TieringPolicy, TieringRule};
//!
//! let engine = TieringPolicyEngine::new(config)
//!     .with_policy(TieringPolicy::age_based("default", Duration::days(7), PerformanceTier::Cold))
//!     .with_policy(TieringPolicy::access_based("hot-keep", 100, PerformanceTier::Hot));
//!
//! engine.start().await?;
//! ```

pub mod engine;
pub mod migration;
pub mod policy;
pub mod retention;
pub mod tracker;

pub use engine::{TieringEngineConfig, TieringPolicyEngine, TieringStats};
pub use migration::{MigrationResult, MigrationStatus, MigrationTask};
pub use policy::{PerformanceTier, PolicyAction, PolicyCondition, TieringPolicy, TieringRule};
pub use retention::{
    ArchiveConfig, ArchiveDestinationType, RetentionAction, RetentionCondition, RetentionManager,
    RetentionManagerConfig, RetentionMetadata, RetentionPolicy, RetentionRule, RetentionStats,
};
pub use tracker::{AccessEvent, AccessPattern, AccessTracker};
