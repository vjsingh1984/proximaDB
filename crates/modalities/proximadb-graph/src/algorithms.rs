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

//! Graph algorithm trait hierarchy
//!
//! Provides extensible trait-based framework for graph algorithms following SOLID principles:
//! - **Single Responsibility**: Each algorithm does one thing well
//! - **Open-Closed**: New algorithms added without modifying existing code
//! - **Liskov Substitution**: Algorithms can be used interchangeably via traits
//! - **Interface Segregation**: Specific traits for specific capabilities (incremental, parallel)
//! - **Dependency Inversion**: Algorithms depend on abstractions (traits), not concrete types

use anyhow::Result;
use proximadb_kernel::error::ProximaDBError;
use std::collections::HashMap;

/// Algorithm complexity estimate for cost-based optimization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlgorithmComplexity {
    /// O(V) - Linear in vertices
    LinearVertices,
    /// O(E) - Linear in edges
    LinearEdges,
    /// O(V + E) - Linear in graph size
    Linear,
    /// O(V log V) - Linearithmic in vertices
    Linearithmic,
    /// O(E log V) - Typical for Dijkstra-like algorithms
    ELogV,
    /// O(V²) - Quadratic in vertices
    QuadraticVertices,
    /// O(V³) - Cubic in vertices (e.g., Floyd-Warshall)
    CubicVertices,
    /// O(E²) - Quadratic in edges
    QuadraticEdges,
    /// NP-Hard or worse
    Exponential,
}

/// Base trait for all graph algorithms
///
/// This trait provides the common interface for executing graph algorithms.
/// All algorithms implement this trait to ensure consistency and composability.
///
/// # Design Pattern
///
/// Uses the Strategy pattern - algorithms are interchangeable strategies
/// for computing graph properties.
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::graph::engines::orion::algorithms::traits::GraphAlgorithm;
///
/// fn run_algorithm<A: GraphAlgorithm>(algo: &A, input: A::Input) -> Result<A::Output> {
///     algo.execute(input)
/// }
/// ```
pub trait GraphAlgorithm: Send + Sync {
    /// Input type for the algorithm
    type Input;

    /// Output type produced by the algorithm
    type Output;

    /// Execute the algorithm with the given input
    ///
    /// This method runs the algorithm and returns the result.
    /// Implementations should be deterministic when possible.
    ///
    /// # Arguments
    ///
    /// * `input` - Algorithm-specific input parameters
    ///
    /// # Returns
    ///
    /// The computed result or an error if execution fails
    fn execute(&self, input: Self::Input) -> Result<Self::Output, ProximaDBError>;

    /// Estimate the computational complexity of this algorithm
    ///
    /// Used for cost-based query optimization and algorithm selection.
    /// The estimate should be based on the current graph size.
    ///
    /// # Returns
    ///
    /// Complexity classification for this algorithm
    fn estimated_complexity(&self) -> AlgorithmComplexity;

    /// Get a human-readable name for this algorithm
    ///
    /// Used for logging, debugging, and query planning.
    ///
    /// # Returns
    ///
    /// Algorithm name (e.g., "PageRank", "Louvain", "Dijkstra")
    fn name(&self) -> &'static str;
}

/// Trait for algorithms that support incremental updates
///
/// Incremental algorithms can efficiently update their results when the graph
/// changes, rather than recomputing from scratch.
///
/// # Use Cases
///
/// - Real-time graph analytics with frequent updates
/// - Streaming graph processing
/// - Materialized view maintenance
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::graph::engines::orion::algorithms::traits::IncrementalAlgorithm;
///
/// // Update algorithm after adding an edge
/// algo.update(GraphChange::EdgeAdded { from: "A", to: "B", weight: 1.0 })?;
/// ```
pub trait IncrementalAlgorithm: GraphAlgorithm {
    /// Apply an incremental update to the algorithm's state
    ///
    /// This method efficiently updates the algorithm's internal state
    /// based on a graph modification.
    ///
    /// # Arguments
    ///
    /// * `change` - The graph modification that occurred
    ///
    /// # Returns
    ///
    /// Ok if the update succeeded, error otherwise
    ///
    /// # Performance
    ///
    /// Should be significantly faster than recomputing `execute()` from scratch.
    fn update(&mut self, change: GraphChange) -> Result<(), ProximaDBError>;

    /// Reset the algorithm's state to empty
    ///
    /// Useful when the graph has changed too much for incremental updates
    /// to be efficient.
    fn reset(&mut self);

    /// Check if incremental update is worth it vs. full recomputation
    ///
    /// Some changes (e.g., removing a central node) may invalidate large
    /// portions of the state, making full recomputation faster.
    ///
    /// # Arguments
    ///
    /// * `change` - The proposed graph change
    ///
    /// # Returns
    ///
    /// true if incremental update is expected to be faster than recomputation
    fn is_incremental_beneficial(&self, _change: &GraphChange) -> bool {
        // Default: always use incremental updates
        // Algorithms can override this with smarter logic
        true
    }
}

/// Graph modification event for incremental algorithms
#[derive(Debug, Clone)]
pub enum GraphChange {
    /// A node was added
    NodeAdded {
        /// Identifier of the newly added node.
        node_id: String,
        /// Key-value properties assigned to the new node.
        properties: HashMap<String, String>,
    },
    /// A node was removed
    NodeRemoved {
        /// Identifier of the removed node.
        node_id: String,
    },
    /// An edge was added
    EdgeAdded {
        /// Source node identifier.
        from: String,
        /// Target node identifier.
        to: String,
        /// Numeric weight of the new edge.
        weight: f64,
    },
    /// An edge was removed
    EdgeRemoved {
        /// Source node identifier.
        from: String,
        /// Target node identifier.
        to: String,
    },
    /// A node's properties were updated
    NodePropertiesUpdated {
        /// Identifier of the updated node.
        node_id: String,
        /// Updated key-value properties.
        properties: HashMap<String, String>,
    },
    /// An edge's weight was updated
    EdgeWeightUpdated {
        /// Source node identifier.
        from: String,
        /// Target node identifier.
        to: String,
        /// New weight value for the edge.
        new_weight: f64,
    },
}

/// Trait for algorithms that can leverage parallel execution
///
/// Parallel algorithms use Rayon or other parallelization libraries
/// to split work across multiple threads.
///
/// # Use Cases
///
/// - Large graphs that don't fit in L3 cache
/// - Computationally intensive algorithms (e.g., community detection)
/// - Batch processing scenarios
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::graph::engines::orion::algorithms::traits::ParallelAlgorithm;
/// use rayon::ThreadPoolBuilder;
///
/// let pool = ThreadPoolBuilder::new().num_threads(8).build()?;
/// let result = algo.execute_parallel(input, &pool)?;
/// ```
pub trait ParallelAlgorithm: GraphAlgorithm {
    /// Execute the algorithm using parallel computation
    ///
    /// This method splits the work across multiple threads using Rayon.
    ///
    /// # Arguments
    ///
    /// * `input` - Algorithm-specific input parameters
    /// * `thread_pool` - Rayon thread pool to use for parallel execution
    ///
    /// # Returns
    ///
    /// The computed result or an error if execution fails
    ///
    /// # Performance
    ///
    /// Should provide near-linear speedup on multi-core systems for large graphs.
    fn execute_parallel(
        &self,
        input: Self::Input,
        thread_pool: &rayon::ThreadPool,
    ) -> Result<Self::Output, ProximaDBError>;

    /// Estimate the parallel speedup factor
    ///
    /// Used for cost-based optimization to decide whether parallel
    /// execution is worth the overhead.
    ///
    /// # Arguments
    ///
    /// * `num_threads` - Number of threads available
    ///
    /// # Returns
    ///
    /// Expected speedup (e.g., 6.0 for 8 threads means 6x faster)
    fn estimated_speedup(&self, num_threads: usize) -> f64 {
        // Amdahl's Law with 10% serial portion (typical for graph algorithms)
        let serial_fraction = 0.1;
        1.0 / (serial_fraction + (1.0 - serial_fraction) / num_threads as f64)
    }

    /// Get the minimum graph size where parallel execution is beneficial
    ///
    /// For small graphs, the parallelization overhead exceeds the benefit.
    ///
    /// # Returns
    ///
    /// Minimum number of vertices for parallel execution to be worthwhile
    fn min_graph_size_for_parallel(&self) -> usize {
        // Default: 10K vertices
        // Algorithms can override based on their specific overhead
        10_000
    }
}

/// Trait for algorithms that support approximate computation
///
/// Approximate algorithms trade accuracy for speed by using sampling,
/// sketching, or other approximation techniques.
///
/// # Use Cases
///
/// - Real-time analytics where exact results aren't critical
/// - Exploratory data analysis
/// - Extremely large graphs where exact algorithms are infeasible
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::graph::engines::orion::algorithms::traits::ApproximateAlgorithm;
///
/// // Run with 95% confidence, 5% error
/// let result = algo.execute_approximate(input, 0.95, 0.05)?;
/// ```
pub trait ApproximateAlgorithm: GraphAlgorithm {
    /// Execute the algorithm with approximate computation
    ///
    /// # Arguments
    ///
    /// * `input` - Algorithm-specific input parameters
    /// * `confidence` - Desired confidence level (0.0-1.0, e.g., 0.95 for 95%)
    /// * `error_bound` - Maximum acceptable relative error (e.g., 0.05 for 5%)
    ///
    /// # Returns
    ///
    /// Approximate result or error if execution fails
    fn execute_approximate(
        &self,
        input: Self::Input,
        confidence: f64,
        error_bound: f64,
    ) -> Result<Self::Output, ProximaDBError>;

    /// Estimate the speedup from using approximation
    ///
    /// # Arguments
    ///
    /// * `confidence` - Desired confidence level
    /// * `error_bound` - Maximum acceptable relative error
    ///
    /// # Returns
    ///
    /// Expected speedup over exact algorithm (e.g., 10.0 means 10x faster)
    fn approximation_speedup(&self, confidence: f64, error_bound: f64) -> f64;
}

/// Common algorithm input types for convenience
#[derive(Debug, Clone)]
pub struct NoInput;

/// Input specifying a single node for algorithms like single-source shortest path.
#[derive(Debug, Clone)]
pub struct SingleNodeInput {
    /// Identifier of the target node.
    pub node_id: String,
}

/// Input specifying a source-target node pair for path-finding algorithms.
#[derive(Debug, Clone)]
pub struct NodePairInput {
    /// Source node identifier.
    pub source: String,
    /// Target node identifier.
    pub target: String,
}

/// Input specifying a subgraph by a set of node identifiers.
#[derive(Debug, Clone)]
pub struct SubgraphInput {
    /// Node identifiers defining the subgraph.
    pub node_ids: Vec<String>,
}

/// Common algorithm output types
/// Maps node identifiers to their centrality scores.
pub type CentralityScores = HashMap<String, f64>;
/// Maps node identifiers to their community/cluster assignments.
pub type CommunityAssignment = HashMap<String, usize>;
/// Ordered sequence of node identifiers forming a shortest path.
pub type ShortestPath = Vec<String>;
/// Maps (source, target) pairs to their shortest path distances.
pub type AllPairsShortestPaths = HashMap<(String, String), f64>;

#[cfg(test)]
mod tests {
    use super::*;

    // Mock algorithm for testing trait implementation
    struct MockAlgorithm;

    impl GraphAlgorithm for MockAlgorithm {
        type Input = NoInput;
        type Output = u64;

        fn execute(&self, _input: NoInput) -> Result<u64, ProximaDBError> {
            Ok(42)
        }

        fn estimated_complexity(&self) -> AlgorithmComplexity {
            AlgorithmComplexity::Linear
        }

        fn name(&self) -> &'static str {
            "MockAlgorithm"
        }
    }

    #[test]
    fn test_algorithm_trait_basic() {
        let algo = MockAlgorithm;
        assert_eq!(algo.execute(NoInput).unwrap(), 42);
        assert_eq!(algo.name(), "MockAlgorithm");
        assert_eq!(algo.estimated_complexity(), AlgorithmComplexity::Linear);
    }

    #[test]
    fn test_estimated_speedup_amdahls_law() {
        struct ParallelMock;
        impl GraphAlgorithm for ParallelMock {
            type Input = NoInput;
            type Output = u64;
            fn execute(&self, _: NoInput) -> Result<u64, ProximaDBError> {
                Ok(0)
            }
            fn estimated_complexity(&self) -> AlgorithmComplexity {
                AlgorithmComplexity::Linear
            }
            fn name(&self) -> &'static str {
                "ParallelMock"
            }
        }
        impl ParallelAlgorithm for ParallelMock {
            fn execute_parallel(
                &self,
                _input: NoInput,
                _thread_pool: &rayon::ThreadPool,
            ) -> Result<u64, ProximaDBError> {
                Ok(0)
            }
        }

        let algo = ParallelMock;
        // With 8 threads and 10% serial fraction:
        // Speedup = 1 / (0.1 + 0.9/8) = 1 / 0.2125 ≈ 4.71
        let speedup = algo.estimated_speedup(8);
        assert!((speedup - 4.71).abs() < 0.01);
    }
}
