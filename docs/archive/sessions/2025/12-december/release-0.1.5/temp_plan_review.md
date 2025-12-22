# ProximaDB Graph Engine Enhancement Plan
## Best-in-Class Feature Parity Implementation

**Status**: Ready for Implementation
**Estimated Timeline**: 20-24 weeks (5 parallel phases)
**Complexity**: HIGH (Enterprise-grade distributed systems + advanced algorithms)

---

## Executive Summary

This plan brings ProximaDB's graph engines (ORION, PULSAR, QUASAR) to best-in-class parity with Neo4j, Amazon Neptune, TigerGraph, and ArangoDB through:

1. **Declarative Query Language** - openCypher support with cost-based optimization
2. **Enterprise Distributed Features** - RAFT consensus, distributed transactions, multi-region replication
3. **Advanced Algorithm Library** - 40+ new algorithms (Louvain, Node2Vec, all-pairs shortest path, etc.)
4. **Hybrid Vector-Graph** - Semantic traversal, vector-guided pathfinding, unified ranking

**Design Principles**:
- ✅ **Reuse existing infrastructure** (CSR storage, query planner, UnifiedDistanceCompute)
- ✅ **Trait-based extensibility** (GraphEngine, PhysicalOperator, CostModel)
- ✅ **SOLID principles** (single responsibility, open-closed, dependency inversion)
- ✅ **Protocol-first** (proto definitions drive implementation)
- ✅ **TDD approach** (tests written before/alongside implementation)

---

## Phase 1: Foundation - Query Language & Trait Architecture (5-6 weeks)

### 1.1 Trait-Based Query Execution Framework

**Objective**: Create extensible query execution layer reusing existing components

#### Core Traits (NEW)

**File**: `src/graph/query/execution_traits.rs`

```rust
/// Physical operator trait - Volcano iterator model
pub trait PhysicalOperator: Send + Sync {
    fn open(&mut self) -> Result<()>;
    fn next(&mut self) -> Result<Option<ResultTuple>>;
    fn close(&mut self) -> Result<()>;
    fn estimated_cardinality(&self) -> usize;
    fn schema(&self) -> &[ColumnSpec];
}

/// Query value enum supporting all graph types
pub enum QueryValue {
    Node(Arc<Node>),
    Edge(Arc<Edge>),
    Path(Vec<PathElement>),
    Property(PropertyValue),
    List(Vec<QueryValue>),
    Null,
}

/// Result tuple with named bindings
pub struct ResultTuple {
    pub bindings: HashMap<String, QueryValue>,
}
```

**Design Rationale**:
- **Single Responsibility**: Each operator does one thing (scan, filter, expand)
- **Open-Closed**: New operators without modifying existing code
- **Dependency Inversion**: Operators depend on trait, not concrete implementations

#### Operator Implementations (REUSE + EXTEND)

**File**: `src/graph/query/operators/mod.rs`

```rust
// Reuse existing engine capabilities
pub struct NodeScanOperator {
    engine: Arc<dyn GraphEngine>,  // ← REUSE GraphEngine trait
    label: Option<String>,
    filters: Vec<PropertyFilter>,
    variable_name: String,
    // State
    iterator: Option<Box<dyn Iterator<Item = Arc<Node>>>>,
}

pub struct ExpandOperator {
    input: Box<dyn PhysicalOperator>,
    engine: Arc<dyn GraphEngine>,  // ← REUSE GraphEngine trait
    // ... existing CSR traversal logic
}

pub struct FilterOperator {
    input: Box<dyn PhysicalOperator>,
    condition: FilterExpression,
    // ← REUSE existing PropertyFilter evaluation
}
```

**Integration Points**:
- Reuse `GraphEngine::get_nodes_by_label()` - no duplication
- Reuse `CsrStorage` for O(degree) neighbor access
- Reuse existing `PropertyFilter` evaluation logic

### 1.2 Enhanced Parser (EXTEND EXISTING)

**File**: `src/graph/query/parser.rs` (MODIFY existing ~500 lines)

**Current State**: Basic Cypher parsing with `nom`, supports simple `MATCH (n:Label) RETURN n`

**Enhancements**:
```rust
// ADD edge pattern parsing (reuse existing NodePattern infrastructure)
fn parse_edge_pattern(input: &str) -> IResult<&str, EdgePattern> {
    // Leverage existing pattern matching infrastructure
}

// ADD WHERE clause parsing
fn parse_where_clause(input: &str) -> IResult<&str, Vec<WhereClause>> {
    // Reuse existing PropertyFilter parsing
}

// ADD aggregation parsing
fn parse_aggregation(input: &str) -> IResult<&str, AggregateSpec> {
    // New: COUNT, SUM, AVG, MIN, MAX
}
```

**Design**: Extend, don't replace. Build on existing AST infrastructure.

### 1.3 Cost-Based Planner Enhancement (EXTEND EXISTING)

**File**: `src/graph/query/planner.rs` (MODIFY existing)

**Current State**: Good infrastructure with `CostModel`, `GraphStatistics`, plan caching

**Enhancements**:
```rust
// EXTEND existing CostModel trait
pub trait CostModel: Send + Sync {
    fn estimate_scan_cost(&self, cardinality: usize) -> f64;
    fn estimate_index_seek_cost(&self, cardinality: usize) -> f64;
    fn estimate_expand_cost(&self, avg_degree: f64) -> f64;
    // NEW: Pattern-specific costs
    fn estimate_pattern_cost(&self, pattern: &CompiledPattern) -> f64;
}

// EXTEND existing planner
impl QueryPlanner {
    // NEW: Pattern join order optimization
    pub fn plan_pattern_query(&self, pattern: &CompiledPattern) -> Result<QueryPlan> {
        // Reuse existing statistics infrastructure
        let stats = self.stats.read()?;
        // ... optimization logic
    }
}
```

**Integration**: Extend existing cost model, don't create parallel system.

### 1.4 Proto Definitions (EXTEND EXISTING)

**File**: `proto/proximadb/v1/graph.proto` (MODIFY existing)

```protobuf
// ADD query language support
message GraphQueryRequest {
  string graph_id = 1;
  QueryLanguage language = 2;
  string query = 3;
  map<string, PropertyValue> parameters = 4;
  optional uint32 timeout_ms = 5;
}

enum QueryLanguage {
  QUERY_LANGUAGE_NATIVE = 0;    // Existing property-based API
  QUERY_LANGUAGE_CYPHER = 1;     // NEW: openCypher
  QUERY_LANGUAGE_GREMLIN = 2;    // Future
}

message GraphQueryResponse {
  repeated ResultRow rows = 1;
  QueryExecutionStats stats = 2;
}

message ResultRow {
  map<string, QueryValue> columns = 1;
}
```

**Design**: Additive changes only. Existing APIs remain unchanged.

### 1.5 TDD: Unit Tests BEFORE Implementation

**File**: `src/graph/query/operators/tests.rs`

```rust
#[cfg(test)]
mod operator_tests {
    #[test]
    fn test_node_scan_with_label_filter() {
        // Create test graph
        // Execute NodeScanOperator
        // Verify correct nodes returned
    }

    #[test]
    fn test_expand_operator_respects_edge_types() {
        // Test edge type filtering during expansion
    }

    #[test]
    fn test_filter_operator_evaluates_complex_conditions() {
        // Test AND/OR/NOT combinations
    }
}
```

**Coverage Target**: 90%+ for all new query execution code

---

## Phase 2: Enterprise Distributed Features (8-10 weeks, HIGH PRIORITY)

### 2.1 Graph-Specific RAFT Consensus (REUSE + SPECIALIZE)

**Objective**: Adapt existing RAFT for graph operations

**File**: `src/graph/engines/pulsar/consensus/mod.rs` (NEW, but REUSE base)

**Base to Reuse**: `src/cluster/consensus.rs` (~1200 lines of RAFT implementation)

```rust
// SPECIALIZE existing RAFT for graph operations
pub enum GraphCommand {
    CreateNode { graph_id: String, node: Node },
    CreateEdge { graph_id: String, edge: Edge },
    UpdateNode { graph_id: String, node_id: String, properties: HashMap<String, PropertyValue> },
    DeleteNode { graph_id: String, node_id: String },
    DeleteEdge { graph_id: String, edge_id: String },
}

// EXTEND existing RaftNode for graph state machine
pub struct GraphRaftNode {
    base: RaftNode<GraphCommand>,  // ← REUSE existing RAFT
    shard: Arc<dyn GraphEngine>,   // ← Apply commands to shard
}

impl StateMachine for GraphRaftNode {
    type Command = GraphCommand;

    fn apply(&mut self, cmd: GraphCommand) -> Result<Vec<u8>> {
        // Apply graph operations to shard
        match cmd {
            GraphCommand::CreateNode { graph_id, node } => {
                self.shard.insert_node(node)?;
                Ok(vec![])
            }
            // ... other commands
        }
    }
}
```

**Integration with Existing Code**:
- Reuse `RaftNode`, `RaftState`, `LogEntry` from existing RAFT
- Reuse `ConsensusConfig` for election timeouts, etc.
- NEW: Graph-specific state machine and command enum

### 2.2 Distributed Transaction Coordinator (NEW LAYER)

**File**: `src/graph/engines/pulsar/transactions/mod.rs`

**Design**: Trait-based for future 3PC/Calvin support

```rust
/// Transaction coordinator trait - enables multiple protocols
pub trait TransactionCoordinator: Send + Sync {
    async fn begin_transaction(&self, participants: Vec<ShardId>) -> Result<TransactionId>;
    async fn execute_operation(&self, tx_id: TransactionId, op: GraphOperation) -> Result<()>;
    async fn commit(&self, tx_id: TransactionId) -> Result<()>;
    async fn abort(&self, tx_id: TransactionId) -> Result<()>;
}

/// Two-Phase Commit implementation
pub struct TwoPhaseCommitCoordinator {
    shards: Arc<HashMap<ShardId, Arc<dyn GraphEngine>>>,
    lock_manager: Arc<DistributedLockManager>,
    transaction_log: Arc<dyn TransactionLog>,  // ← REUSE existing WAL infrastructure
}

impl TransactionCoordinator for TwoPhaseCommitCoordinator {
    async fn commit(&self, tx_id: TransactionId) -> Result<()> {
        // Phase 1: PREPARE
        let votes = self.send_prepare_to_all_participants(tx_id).await?;

        if votes.iter().all(|v| v.is_commit()) {
            // Phase 2: COMMIT
            self.send_commit_to_all_participants(tx_id).await?;
            Ok(())
        } else {
            // ABORT
            self.send_abort_to_all_participants(tx_id).await?;
            Err(ProximaDBError::TransactionAborted)
        }
    }
}
```

**Integration with Existing WAL**:
- Reuse `src/storage/persistence/write_ahead_log/` for transaction logs
- Reuse existing flush/sync mechanisms
- NEW: Distributed coordination layer

### 2.3 Cross-Shard Query Optimization (EXTEND EXISTING)

**File**: `src/graph/engines/pulsar/coordinator.rs` (MODIFY existing ~400 lines)

**Current State**: Basic cross-shard BFS/DFS

**Enhancements**:
```rust
// EXTEND existing QueryCoordinator
impl QueryCoordinator {
    // NEW: Plan-based execution (reuse query planner)
    pub async fn execute_planned_query(
        &self,
        plan: QueryPlan,  // ← From enhanced query planner
    ) -> Result<Vec<ResultTuple>> {
        // Route plan steps to appropriate shards
        // Collect and merge results
    }

    // NEW: Cache cross-shard edge lists
    pub async fn get_cross_shard_edges_cached(
        &self,
        from_shard: ShardId,
        to_shard: ShardId,
    ) -> Result<Vec<Arc<Edge>>> {
        // Check LRU cache first
        // Fetch from remote shard if miss
    }
}
```

**Integration**: Build on existing coordinator, add caching layer.

### 2.4 Multi-Region Support (NEW MODULE)

**File**: `src/graph/engines/pulsar/regions/mod.rs`

**Design**: Trait-based region abstraction

```rust
/// Region abstraction for multi-region deployment
pub trait RegionManager: Send + Sync {
    async fn get_local_region(&self) -> Result<RegionId>;
    async fn get_peer_regions(&self) -> Result<Vec<RegionId>>;
    async fn replicate_to_region(&self, region: RegionId, ops: Vec<GraphCommand>) -> Result<()>;
    async fn promote_region(&self, region: RegionId) -> Result<()>;
}

pub struct MultiRegionCoordinator {
    local_region: RegionId,
    peer_regions: Vec<RegionConfig>,
    replication_lag_tracker: Arc<LagTracker>,
    // REUSE existing RAFT for region coordination
    consensus: Arc<GraphRaftNode>,
}
```

**Integration with Existing Infrastructure**:
- Reuse `UnifiedCachingFilesystem` for cross-region storage
- Reuse RAFT consensus for region failover
- NEW: Region-aware routing and replication

### 2.5 TDD: Integration Tests

**File**: `tests/graph_distributed_test.rs`

```rust
#[tokio::test]
async fn test_cross_shard_transaction_atomicity() {
    // Create 3-shard cluster
    // Execute transaction spanning 2 shards
    // Verify atomicity (all or nothing)
}

#[tokio::test]
async fn test_raft_leader_election_on_failure() {
    // Create 3-replica cluster
    // Kill leader
    // Verify new leader elected within 5s
}

#[tokio::test]
async fn test_multi_region_replication_lag() {
    // Create 2-region setup
    // Insert 1000 nodes in region 1
    // Verify replication to region 2 within 100ms
}
```

---

## Phase 3: Advanced Algorithm Library (6-8 weeks)

### 3.1 Algorithm Trait Hierarchy (SOLID Design)

**File**: `src/graph/engines/orion/algorithms/traits.rs`

```rust
/// Base algorithm trait - common interface
pub trait GraphAlgorithm: Send + Sync {
    type Input;
    type Output;

    fn execute(&self, input: Self::Input) -> Result<Self::Output>;
    fn estimated_complexity(&self) -> AlgorithmComplexity;
}

/// Incremental algorithm trait - supports streaming updates
pub trait IncrementalAlgorithm: GraphAlgorithm {
    fn update(&mut self, change: GraphChange) -> Result<()>;
}

/// Parallel algorithm trait - leverages Rayon
pub trait ParallelAlgorithm: GraphAlgorithm {
    fn execute_parallel(&self, input: Self::Input, thread_pool: &rayon::ThreadPool) -> Result<Self::Output>;
}
```

**Design**: Trait hierarchy enables algorithm composition and specialization.

### 3.2 Community Detection (REUSE CSR + Rayon)

**File**: `src/graph/engines/orion/algorithms/community.rs` (NEW)

```rust
pub struct LouvainCommunityDetection {
    csr: Arc<CsrStorage>,  // ← REUSE existing CSR
    resolution: f64,
    max_iterations: usize,
}

impl GraphAlgorithm for LouvainCommunityDetection {
    type Input = ();
    type Output = HashMap<NodeId, CommunityId>;

    fn execute(&self, _: ()) -> Result<HashMap<NodeId, CommunityId>> {
        // Phase 1: Local moving (parallel with Rayon)
        let communities = self.local_moving_phase()?;

        // Phase 2: Aggregation
        let aggregated = self.aggregate_communities(communities)?;

        Ok(aggregated)
    }
}

impl LouvainCommunityDetection {
    fn local_moving_phase(&self) -> Result<Vec<CommunityId>> {
        use rayon::prelude::*;  // ← REUSE existing Rayon parallelism

        // Parallel community assignment
        (0..self.csr.node_count())
            .into_par_iter()
            .map(|node_idx| self.find_best_community(node_idx))
            .collect()
    }
}
```

**Integration**:
- Reuse `CsrStorage` for O(degree) neighbor access
- Reuse Rayon thread pool
- Reuse existing graph statistics

### 3.3 Centrality Algorithms (REUSE + SIMD)

**File**: `src/graph/engines/orion/algorithms/centrality.rs` (EXTEND existing)

**Current State**: Basic PageRank, betweenness

**Enhancements**:
```rust
// ADD closeness centrality (reuse BFS infrastructure)
pub struct ClosenessCentrality {
    csr: Arc<CsrStorage>,
    normalized: bool,
}

impl GraphAlgorithm for ClosenessCentrality {
    type Input = ();
    type Output = HashMap<NodeId, f64>;

    fn execute(&self, _: ()) -> Result<HashMap<NodeId, f64>> {
        use rayon::prelude::*;

        // Parallel BFS from all nodes (reuse existing BFS)
        let distances: Vec<_> = (0..self.csr.node_count())
            .into_par_iter()
            .map(|source| self.bfs_distances(source))  // ← REUSE existing BFS
            .collect();

        // Compute centrality scores
        Ok(self.compute_scores(distances))
    }
}
```

**Integration**: Reuse existing BFS/DFS infrastructure, add parallel execution.

### 3.4 Pathfinding Algorithms (REUSE + EXTEND)

**File**: `src/graph/engines/orion/traversal.rs` (MODIFY existing)

**Current State**: Dijkstra, A*, basic K-shortest paths

**Enhancements**:
```rust
// ADD all-pairs shortest path (new algorithm)
pub struct FloydWarshallAPSP {
    csr: Arc<CsrStorage>,
    use_simd: bool,
}

impl FloydWarshallAPSP {
    pub fn execute(&self) -> Result<Vec<Vec<f64>>> {
        let n = self.csr.node_count();
        let mut dist = vec![vec![f64::INFINITY; n]; n];

        // Initialize distances (reuse CSR edge access)
        for i in 0..n {
            dist[i][i] = 0.0;
            for (neighbor_idx, edge_id) in self.csr.get_neighbors(i)? {
                dist[i][neighbor_idx] = 1.0;  // unweighted
            }
        }

        // Floyd-Warshall with SIMD optimization
        if self.use_simd {
            self.floyd_warshall_simd(&mut dist)?;  // ← REUSE existing SIMD infrastructure
        } else {
            self.floyd_warshall_scalar(&mut dist)?;
        }

        Ok(dist)
    }
}
```

**Integration**: Build on existing pathfinding, add SIMD acceleration.

### 3.5 Node Embeddings (REUSE Vector Engine)

**File**: `src/graph/engines/orion/algorithms/embeddings.rs` (NEW)

```rust
pub struct Node2VecEmbeddings {
    csr: Arc<CsrStorage>,
    walk_length: usize,
    num_walks: usize,
    embedding_dim: usize,
    // REUSE vector engine for embedding computation
    vector_engine: Arc<dyn UnifiedStorageEngine>,
}

impl Node2VecEmbeddings {
    pub async fn execute(&self) -> Result<HashMap<NodeId, Vec<f32>>> {
        // Generate random walks (parallel with Rayon)
        let walks = self.generate_walks_parallel()?;

        // Train Skip-Gram model (reuse vector engine)
        let embeddings = self.train_skipgram(&walks).await?;

        Ok(embeddings)
    }

    async fn train_skipgram(&self, walks: &[Vec<NodeId>]) -> Result<HashMap<NodeId, Vec<f32>>> {
        // REUSE UnifiedDistanceCompute for SIMD operations
        // Store embeddings via vector engine
        // Return trained embeddings
    }
}
```

**Integration**:
- Reuse `UnifiedStorageEngine` for storing embeddings
- Reuse `UnifiedDistanceCompute` for SIMD operations
- Store embeddings as node properties

### 3.6 TDD: Algorithm Tests

**File**: `src/graph/engines/orion/algorithms/tests.rs`

```rust
#[test]
fn test_louvain_karate_club() {
    // Zachary's karate club has known community structure
    let graph = create_karate_club_graph();
    let louvain = LouvainCommunityDetection::new(graph.csr(), 1.0, 10);
    let communities = louvain.execute(()).unwrap();

    // Verify 2 communities detected
    assert_eq!(count_communities(&communities), 2);
}

#[test]
fn test_closeness_centrality_star_graph() {
    // Star graph: center has highest closeness
    let graph = create_star_graph(10);
    let closeness = ClosenessCentrality::new(graph.csr(), true);
    let scores = closeness.execute(()).unwrap();

    let center_score = scores.get(&"center".to_string()).unwrap();
    assert!(*center_score > 0.9);  // Close to 1.0
}
```

---

## Phase 4: Hybrid Vector-Graph Features (4-5 weeks)

### 4.1 Semantic Traversal (REUSE Distance Engine)

**File**: `src/graph/hybrid/semantic_traversal.rs` (NEW)

```rust
pub struct SemanticBFSTraversal {
    graph_engine: Arc<dyn GraphEngine>,
    // REUSE existing distance computation
    distance_compute: Arc<UnifiedDistanceCompute>,
    similarity_threshold: f32,
    distance_metric: DistanceMetric,
}

impl SemanticBFSTraversal {
    pub async fn execute(
        &self,
        start_node: NodeId,
        query_embedding: &[f32],
        max_depth: u32,
    ) -> Result<Vec<(Arc<Node>, f32)>> {
        let mut results = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back((start_node, 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= max_depth { continue; }

            let node = self.graph_engine.get_node(&current_id).await?
                .ok_or(ProximaDBError::NotFound)?;

            // Compute similarity (REUSE SIMD distance computation)
            if let Some(embedding) = node.embedding.as_ref() {
                let similarity = self.distance_compute.compute_similarity(
                    query_embedding,
                    &embedding.vector,
                    self.distance_metric,
                )?;

                if similarity >= self.similarity_threshold {
                    results.push((node.clone(), similarity));
                }
            }

            // Expand to neighbors
            let neighbors = self.graph_engine.get_neighbors(&current_id, None).await?;
            for neighbor_id in neighbors {
                queue.push_back((neighbor_id, depth + 1));
            }
        }

        Ok(results)
    }
}
```

**Integration**:
- Reuse `UnifiedDistanceCompute` for SIMD similarity
- Reuse `GraphEngine` trait for traversal
- No duplication of distance computation logic

### 4.2 Vector-Guided Pathfinding (EXTEND A*)

**File**: `src/graph/engines/orion/traversal.rs` (MODIFY existing)

**Current A* Implementation**: Lines 500-700 with embedding heuristics

**Enhancement**:
```rust
// EXTEND existing A* implementation
impl OrionGraphEngine {
    pub async fn vector_guided_astar(
        &self,
        start: NodeId,
        target: NodeId,
        guide_embedding: &[f32],
        alpha: f64,  // 0.0 = pure graph, 1.0 = pure semantic
    ) -> Result<Option<(Vec<NodeId>, f64)>> {
        // REUSE existing A* infrastructure
        let mut open_set = BinaryHeap::new();
        let mut g_score = HashMap::new();

        // Hybrid heuristic (REUSE existing embedding heuristic)
        let heuristic = |node_id: &NodeId| -> f64 {
            let graph_distance = self.estimate_distance(node_id, &target);
            let semantic_distance = self.compute_semantic_distance(node_id, guide_embedding);

            (1.0 - alpha) * graph_distance + alpha * semantic_distance
        };

        // Standard A* algorithm (reuse existing implementation)
        // ...
    }
}
```

**Integration**: Extend existing A*, don't duplicate pathfinding logic.

### 4.3 Unified Ranking (NEW LAYER)

**File**: `src/graph/hybrid/ranking.rs` (NEW)

```rust
pub trait RankingStrategy: Send + Sync {
    fn compute_score(&self, node: &Node, context: &RankingContext) -> Result<f64>;
}

pub struct HybridRankingStrategy {
    vector_weight: f64,
    graph_weight: f64,
    // REUSE existing components
    distance_compute: Arc<UnifiedDistanceCompute>,
    centrality_cache: Arc<DashMap<NodeId, f64>>,
}

impl RankingStrategy for HybridRankingStrategy {
    fn compute_score(&self, node: &Node, context: &RankingContext) -> Result<f64> {
        // Vector similarity (REUSE)
        let vector_score = if let Some(embedding) = &node.embedding {
            self.distance_compute.compute_similarity(
                &context.query_embedding,
                &embedding.vector,
                context.distance_metric,
            )?
        } else {
            0.0
        };

        // Graph centrality (REUSE cached PageRank)
        let graph_score = self.centrality_cache
            .get(&node.id)
            .map(|v| *v)
            .unwrap_or(0.0);

        Ok(self.vector_weight * vector_score + self.graph_weight * graph_score)
    }
}
```

**Integration**: Compose existing vector and graph signals.

### 4.4 TDD: Hybrid Tests

**File**: `tests/graph_hybrid_test.rs`

```rust
#[tokio::test]
async fn test_semantic_bfs_finds_similar_nodes() {
    let engine = create_test_engine_with_embeddings().await;

    let query_embedding = vec![0.1; 768];
    let results = engine.semantic_bfs("start", &query_embedding, 0.8, 3).await.unwrap();

    // Verify all results have similarity >= 0.8
    for (node, sim) in results {
        assert!(sim >= 0.8);
    }
}

#[tokio::test]
async fn test_vector_guided_astar_finds_semantic_path() {
    // Create graph with embeddings
    // Find path guided by topic embedding
    // Verify path passes through semantically relevant nodes
}
```

---

## Phase 5: QUASAR Hot/Cold Tiering Completion (3-4 weeks)

### 5.1 Complete Migration Logic (FIX EXISTING)

**File**: `src/graph/engines/quasar/tiering.rs` (MODIFY lines 258-298)

**Current State**: All migration methods return errors (disabled)

**Fix**:
```rust
// ENABLE disabled migration logic
impl TieringManager {
    pub async fn migrate_node_to_cold(&self, node_id: &NodeId) -> Result<()> {
        // 1. Get node from hot tier (REUSE)
        let node = self.hot_tier.get_node(node_id).await?
            .ok_or(ProximaDBError::NotFound)?;

        // 2. Store to cold tier (REUSE storage backend)
        self.cold_backend.store_node(node.as_ref()).await?;

        // 3. Delete from hot tier (REUSE)
        self.hot_tier.delete_node(node_id).await?;

        // 4. Update access pattern cache
        self.access_cache.mark_cold(node_id);

        Ok(())
    }

    pub async fn promote_node_to_hot(&self, node_id: &NodeId) -> Result<()> {
        // 1. Get node from cold tier
        let node = self.cold_backend.load_node(node_id).await?;

        // 2. Insert into hot tier (REUSE)
        self.hot_tier.insert_node(node).await?;

        // 3. Update access pattern cache
        self.access_cache.mark_hot(node_id);

        Ok(())
    }
}
```

**Integration**: Reuse existing hot tier (ORION) and cold backend interfaces.

### 5.2 Cold Storage Backend Implementation (REUSE SST)

**File**: `src/graph/engines/quasar/storage_backend.rs` (MODIFY lines 495-531)

**Current State**: SST and Parquet backends are placeholders

**Implementation**:
```rust
pub struct SstColdBackend {
    // REUSE existing SST engine
    sst_engine: Arc<SstEngine>,
    collection_id: String,
}

impl ColdStorageBackend for SstColdBackend {
    async fn store_node(&self, node: &Node) -> Result<()> {
        // Serialize node to VectorRecord format
        let record = VectorRecord {
            id: node.id.clone(),
            vector: node.embedding.as_ref().map(|e| e.vector.clone()).unwrap_or_default(),
            metadata: self.serialize_node_properties(node),
            // ...
        };

        // REUSE SST flush mechanism
        let flush_params = FlushParameters {
            collection_id: Some(self.collection_id.clone()),
            vector_records: vec![record.into()],
            force: false,
            synchronous: false,
            // ...
        };

        self.sst_engine.do_flush(&flush_params).await?;
        Ok(())
    }

    async fn load_node(&self, node_id: &NodeId) -> Result<Arc<Node>> {
        // REUSE SST search mechanism
        let search_params = SearchParams {
            query_vectors: None,  // Exact ID lookup
            top_k: Some(1),
            metadata_filter: Some(vec![/* id filter */]),
            // ...
        };

        let results = self.sst_engine.search_vectors_unified(&ctx).await?;
        // Deserialize VectorRecord back to Node
        Ok(Arc::new(self.deserialize_node(results[0].clone())))
    }
}
```

**Integration**: Reuse entire SST engine infrastructure, no duplication.

### 5.3 TDD: Tiering Tests

**File**: `tests/graph_tiering_test.rs`

```rust
#[tokio::test]
async fn test_node_migration_to_cold_tier() {
    let quasar = create_quasar_engine().await;

    // Insert node in hot tier
    quasar.insert_node(test_node()).await.unwrap();

    // Fill hot tier to trigger eviction
    for i in 0..1000 {
        quasar.insert_node(generate_node(i)).await.unwrap();
    }

    // Verify node migrated to cold tier
    let node = quasar.get_node("test_node").await.unwrap();
    assert!(node.is_some());

    // Verify stored in cold backend
    assert!(quasar.cold_backend.exists("test_node").await.unwrap());
}
```

---

## Critical Files Summary

### Top 10 Files to Modify (Priority Order)

1. **`src/graph/query/execution_traits.rs`** (NEW) - Core trait definitions for extensible query execution
2. **`src/graph/query/operators/mod.rs`** (NEW) - PhysicalOperator implementations reusing GraphEngine
3. **`src/graph/query/parser.rs`** (MODIFY) - Extend with edge patterns, WHERE, aggregations
4. **`src/graph/query/planner.rs`** (MODIFY) - Add pattern query planning
5. **`src/graph/engines/pulsar/consensus/mod.rs`** (NEW) - Graph-specific RAFT reusing base consensus
6. **`src/graph/engines/pulsar/transactions/mod.rs`** (NEW) - Distributed transactions with trait-based design
7. **`src/graph/engines/orion/algorithms/community.rs`** (NEW) - Louvain, Label Propagation reusing CSR
8. **`src/graph/engines/orion/algorithms/centrality.rs`** (EXTEND) - Add closeness, harmonic, eigenvector
9. **`src/graph/hybrid/semantic_traversal.rs`** (NEW) - Semantic BFS reusing UnifiedDistanceCompute
10. **`src/graph/engines/quasar/tiering.rs`** (FIX) - Enable migration logic

---

## Test-Driven Development Strategy

### Test Pyramid

```
              /\
             /  \  E2E Tests (5%)
            /____\  - Multi-node clusters
           /      \  - Chaos testing
          /________\
         /          \ Integration Tests (25%)
        /____________\  - Cross-component
       /              \  - API tests
      /________________\
     /                  \ Unit Tests (70%)
    /____________________\  - Algorithm correctness
                             - Operator logic
                             - Parser validation
```

### Test Coverage Requirements

| Component | Target Coverage | Critical Tests |
|-----------|----------------|----------------|
| Query Parser | 95% | Edge cases, syntax errors |
| Query Operators | 90% | Empty results, large results |
| Algorithms | 95% | Known graphs, edge cases |
| RAFT Consensus | 85% | Leader election, split brain |
| Transactions | 90% | Atomicity, deadlocks |
| Hybrid Features | 85% | Embedding integration |

### Testing Workflow

```
1. Write failing test (RED)
   ↓
2. Implement minimum code to pass (GREEN)
   ↓
3. Refactor for reuse and clarity (REFACTOR)
   ↓
4. Commit with test
   ↓
5. Repeat
```

---

## Implementation Order (Dependency-Aware)

### Week 1-2: Foundation
- [ ] Define PhysicalOperator trait and core operators
- [ ] Extend query parser with edge patterns
- [ ] Write unit tests for operators

### Week 3-4: Query Execution
- [ ] Implement NodeScan, Expand, Filter operators
- [ ] Extend query planner with pattern optimization
- [ ] Integration tests for query execution

### Week 5-6: Proto + API
- [ ] Add GraphQueryRequest/Response to proto
- [ ] Implement API handlers
- [ ] End-to-end query tests

### Week 7-10: RAFT + Transactions
- [ ] Specialize RAFT for graph operations
- [ ] Implement 2PC transaction coordinator
- [ ] Chaos tests for consensus

### Week 11-14: Algorithm Library (Phase 1)
- [ ] Implement Louvain, Label Propagation
- [ ] Implement closeness, harmonic centrality
- [ ] Algorithm correctness tests

### Week 15-18: Algorithm Library (Phase 2)
- [ ] Implement Node2Vec embeddings
- [ ] Implement all-pairs shortest path
- [ ] Performance benchmarks

### Week 19-20: Hybrid Features
- [ ] Implement semantic BFS
- [ ] Implement vector-guided A*
- [ ] Hybrid integration tests

### Week 21-22: QUASAR Completion
- [ ] Fix migration logic
- [ ] Implement SST cold backend
- [ ] Tiering tests

### Week 23-24: Final Testing + Documentation
- [ ] End-to-end tests across all features
- [ ] Performance benchmarking
- [ ] Documentation updates

---

## Success Metrics

### Performance Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| Query latency (simple MATCH) | < 5ms | p99 |
| Cross-shard query latency | < 50ms | p99 for 3-hop BFS |
| Transaction throughput | 5K tx/sec | Single-shard |
| Leader election time | < 5s | Worst case |
| Algorithm performance | Within 2x of Neo4j | Benchmark suite |

### Code Quality Targets

| Metric | Target |
|--------|--------|
| Test coverage | > 85% overall, > 90% for critical paths |
| Trait-based design | > 80% of new code uses traits |
| Code reuse | < 10% duplication (SonarQube) |
| Documentation | 100% public APIs documented |

---

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| RAFT complexity bugs | Extensive Jepsen-style testing, formal verification for critical sections |
| Performance regressions | Continuous benchmarking, performance tests in CI |
| Integration issues | Incremental integration, feature flags for gradual rollout |
| Test maintenance burden | Shared test utilities, property-based testing for algorithms |

---

## Conclusion

This plan achieves best-in-class parity through:

✅ **Maximum Code Reuse** - Leverages existing CSR, RAFT, SST, distance compute
✅ **Trait-Based Extensibility** - PhysicalOperator, GraphAlgorithm, TransactionCoordinator
✅ **SOLID Principles** - Single responsibility, open-closed, dependency inversion
✅ **Protocol-First Design** - Proto definitions drive implementation
✅ **TDD Approach** - 85%+ test coverage, tests written first

**Total Estimated Effort**: 20-24 weeks with 2-3 senior engineers working in parallel on independent phases.
