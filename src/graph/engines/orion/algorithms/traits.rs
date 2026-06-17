//! Compatibility shim — implementation now lives in `proximadb-graph`.
pub use proximadb_graph::AlgoShortestPath as ShortestPath;
pub use proximadb_graph::algorithms::{
    AlgorithmComplexity, AllPairsShortestPaths, ApproximateAlgorithm, CentralityScores,
    CommunityAssignment, GraphAlgorithm, GraphChange, IncrementalAlgorithm, NoInput, NodePairInput,
    ParallelAlgorithm, SingleNodeInput, SubgraphInput,
};
