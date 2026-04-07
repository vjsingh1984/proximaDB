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

//! Distributed Query Coordination
//!
//! This module provides distributed query execution across a cluster of ProximaDB nodes.
//! It integrates with the cluster infrastructure (ClusterManager, RoutingService, ShardManager)
//! to route queries to appropriate nodes and aggregate results.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                   DistributedQueryCoordinator                    │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐ │
//! │  │  Query Planner  │  │ Remote Executor │  │Result Aggregator│ │
//! │  │   (decompose)   │  │   (gRPC calls)  │  │   (merge)       │ │
//! │  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘ │
//! ├───────────┼─────────────────────┼─────────────────────┼─────────┤
//! │           │    ClusterManager   │                     │         │
//! │           ▼                     ▼                     ▼         │
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐ │
//! │  │RoutingService   │  │  ShardManager   │  │  NodeRegistry   │ │
//! │  │(shard routing)  │  │(shard placement)│  │ (node health)   │ │
//! │  └─────────────────┘  └─────────────────┘  └─────────────────┘ │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Query Flow
//!
//! 1. **Receive Query**: Accept multi-model query from client
//! 2. **Plan Distribution**: Analyze query to identify target shards/nodes
//! 3. **Route Subqueries**: Send subqueries to appropriate nodes via gRPC
//! 4. **Execute Locally**: Run local portion using ParallelExecutor
//! 5. **Aggregate Results**: Merge remote and local results using FusionEngine
//! 6. **Return Response**: Send unified response to client

pub mod aggregator;
pub mod coordinator;
pub mod planner;
pub mod remote;
pub mod shuffle;

pub use aggregator::{AggregationStrategy, ResultAggregator};
pub use coordinator::{DistributedQueryConfig, DistributedQueryCoordinator, QueryPlan, ShardInfo};
pub use planner::{DistributionStrategy, ShardedSubQuery};
pub use remote::{RemoteExecutor, RemoteQueryHandler, RemoteQueryResult};
pub use shuffle::{ShuffleConfig, ShuffleExchange, ShuffleKey, ShuffleStats};
