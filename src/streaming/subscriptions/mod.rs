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

//! Live Query Subscription System
//!
//! This module implements real-time query subscriptions that automatically
//! push updates to clients when relevant vectors are inserted, updated, or deleted.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
//! │  Vector Insert  │───▶│SubscriptionMgr  │───▶│ QueryEvaluator  │
//! └─────────────────┘    └─────────────────┘    └─────────────────┘
//!                               │                       │
//!                               ▼                       ▼
//!                        ┌─────────────────┐    ┌─────────────────┐
//!                        │  Subscription   │    │  Result Set     │
//!                        │     Index       │    │  Maintenance    │
//!                        └─────────────────┘    └─────────────────┘
//! ```
//!
//! ## Features
//!
//! - **Query Fingerprinting**: Deduplicate identical subscriptions
//! - **Incremental Evaluation**: Only evaluate against new vectors
//! - **Score Change Detection**: Detect when results change position
//! - **Connection Tracking**: Handle client disconnections gracefully

pub mod evaluator;
pub mod manager;
pub mod result_set;
pub mod subscription;

pub use evaluator::{EvaluationResult, QueryEvaluator, ScoreChange};
pub use manager::{SubscriptionHandle, SubscriptionManager};
pub use result_set::{ResultSet, ResultSetStats};
pub use subscription::{
    QueryFingerprint, QueryUpdate, ResultChange, ScoredResult, Subscription,
    SubscriptionConfig, SubscriptionId, SubscriptionState, SubscriptionStats, UpdateType,
};
