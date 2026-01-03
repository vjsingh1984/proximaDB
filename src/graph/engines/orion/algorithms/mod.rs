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

//! Graph algorithms module
//!
//! Provides implementations of classic and modern graph algorithms:
//! - **Centrality**: PageRank, betweenness, closeness, harmonic, eigenvector
//! - **Community Detection**: Louvain, label propagation, modularity optimization
//! - **Pathfinding**: Dijkstra, A*, Floyd-Warshall, all-pairs shortest paths
//! - **Embeddings**: Node2Vec, DeepWalk, graph neural network features
//! - **Traversal**: BFS, DFS, topological sort, cycle detection
//!
//! # Design Principles
//!
//! 1. **Reuse**: All algorithms reuse existing CSR storage, no duplication
//! 2. **Trait-Based**: Extensible via GraphAlgorithm trait hierarchy
//! 3. **Parallelism**: Leverage Rayon for multi-threaded execution
//! 4. **SIMD**: Use hardware acceleration where possible
//! 5. **Incremental**: Support for streaming updates on dynamic graphs
//!
//! # Example
//!
//! ```rust
//! use proximadb::graph::engines::orion::algorithms::centrality::PageRank;
//! use proximadb::graph::engines::orion::algorithms::traits::GraphAlgorithm;
//!
//! let pagerank = PageRank::new(graph.csr(), 0.85, 100, 1e-6);
//! let scores = pagerank.execute(())?;
//! ```

pub mod centrality;
pub mod community;
pub mod embeddings;
pub mod pathfinding;
pub mod traits;

// Re-export core traits for convenience
pub use traits::{
    AlgorithmComplexity, AllPairsShortestPaths, ApproximateAlgorithm, CentralityScores,
    CommunityAssignment, GraphAlgorithm, GraphChange, IncrementalAlgorithm, NoInput, NodePairInput,
    ParallelAlgorithm, ShortestPath, SingleNodeInput, SubgraphInput,
};
