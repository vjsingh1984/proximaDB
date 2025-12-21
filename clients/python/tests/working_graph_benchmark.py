#!/usr/bin/env python3
"""
Working Graph Database Benchmark
Compares ProximaDB, NetworkX, and igraph using actual embedded APIs
"""

import gc
import os
import sys
import time
import tempfile
import shutil
import random
import json
from typing import List, Dict, Tuple, Optional
from dataclasses import dataclass, asdict
from collections import defaultdict
import traceback

import numpy as np

# Database imports
try:
    import proximadb
    PROXIMADB_AVAILABLE = True
    print(f"✓ ProximaDB v{proximadb.__version__}")
except ImportError:
    PROXIMADB_AVAILABLE = False
    print("✗ ProximaDB not available")

try:
    import networkx as nx
    NETWORKX_AVAILABLE = True
    print(f"✓ NetworkX v{nx.__version__}")
except ImportError:
    NETWORKX_AVAILABLE = False
    print("✗ NetworkX not available")

try:
    import igraph as ig
    IGRAPH_AVAILABLE = True
    print(f"✓ igraph v{ig.__version__}")
except ImportError:
    IGRAPH_AVAILABLE = False
    print("✗ igraph not available")


@dataclass
class GraphConfig:
    name: str
    num_nodes: int
    num_edges: int

    @classmethod
    def small(cls):
        return cls("small", 1_000, 5_000)

    @classmethod
    def medium(cls):
        return cls("medium", 10_000, 50_000)

    @classmethod
    def large(cls):
        return cls("large", 50_000, 250_000)


@dataclass
class BenchmarkResult:
    database: str
    operation: str
    config: str
    time_ms: float
    throughput_ops_sec: Optional[float] = None
    success: bool = True
    error: Optional[str] = None


def generate_graph_data(config: GraphConfig, seed=42):
    """Generate deterministic graph data."""
    random.seed(seed)
    np.random.seed(seed)

    labels = ["Person", "Document", "Function", "Class", "Module"]
    edge_types = ["KNOWS", "REFERENCES", "CALLS", "INHERITS", "IMPORTS"]

    nodes = []
    for i in range(config.num_nodes):
        embedding = np.random.randn(128).astype(np.float32).tolist()
        node = {
            "id": f"node_{i}",
            "labels": [random.choice(labels)],
            "properties": {
                "name": f"Entity_{i}",
                "category": random.choice(["A", "B", "C", "D"]),
                "score": random.randint(1, 100),
            },
            "embedding": embedding
        }
        nodes.append(node)

    edges = []
    edge_set = set()
    for i in range(config.num_edges):
        from_idx = random.randint(0, config.num_nodes - 1)
        locality = min(config.num_nodes // 10, 100)
        to_idx = (from_idx + random.randint(1, locality)) % config.num_nodes

        edge_key = (from_idx, to_idx)
        if edge_key in edge_set or from_idx == to_idx:
            continue
        edge_set.add(edge_key)

        edge = {
            "from_node_id": f"node_{from_idx}",
            "to_node_id": f"node_{to_idx}",
            "edge_type": random.choice(edge_types),
            "weight": random.random(),
        }
        edges.append(edge)

    return nodes, edges


# =============================================================================
# ProximaDB Benchmark
# =============================================================================

class ProximaDBBenchmark:
    def __init__(self, config: GraphConfig):
        self.config = config
        self.db = None
        self.graph_id = "benchmark_graph"

    def setup(self):
        self.temp_dir = tempfile.mkdtemp(prefix="proximadb_bench_")
        self.db = proximadb.ProximaDB(data_dirs=self.temp_dir)
        self.db.create_graph(self.graph_id)

    def teardown(self):
        if self.db:
            try:
                self.db.delete_graph(self.graph_id)
            except:
                pass
            del self.db
        if hasattr(self, 'temp_dir') and os.path.exists(self.temp_dir):
            shutil.rmtree(self.temp_dir)

    def benchmark_bulk_insert(self, nodes: List[Dict], edges: List[Dict]) -> BenchmarkResult:
        start = time.perf_counter()

        # Create GraphNode objects using constructor with labels and properties
        graph_nodes = []
        for node in nodes:
            # Convert properties to string dict
            props = {k: str(v) for k, v in node["properties"].items()}
            gn = proximadb.GraphNode(
                node["id"],
                labels=node["labels"],
                properties=props
            )
            graph_nodes.append(gn)

        # Batch insert nodes
        self.db.create_nodes(self.graph_id, graph_nodes)

        # Create GraphEdge objects
        graph_edges = []
        for edge in edges:
            ge = proximadb.GraphEdge(
                edge["from_node_id"],
                edge["to_node_id"],
                edge["edge_type"],
                weight=edge.get("weight", 1.0)
            )
            graph_edges.append(ge)

        # Batch insert edges
        self.db.create_edges(self.graph_id, graph_edges)

        elapsed_ms = (time.perf_counter() - start) * 1000
        total_ops = len(nodes) + len(edges)
        throughput = total_ops / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="ProximaDB",
            operation="bulk_insert",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_node_lookup(self, node_ids: List[str]) -> BenchmarkResult:
        start = time.perf_counter()

        for node_id in node_ids:
            node = self.db.get_node(self.graph_id, node_id)

        elapsed_ms = (time.perf_counter() - start) * 1000
        throughput = len(node_ids) / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="ProximaDB",
            operation="node_lookup",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_neighbor_query(self, node_ids: List[str]) -> BenchmarkResult:
        start = time.perf_counter()

        for node_id in node_ids:
            edges = self.db.get_outgoing_edges(self.graph_id, node_id)

        elapsed_ms = (time.perf_counter() - start) * 1000
        throughput = len(node_ids) / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="ProximaDB",
            operation="neighbor_query",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_graph_stats(self) -> BenchmarkResult:
        start = time.perf_counter()

        stats = self.db.graph_stats(self.graph_id)

        elapsed_ms = (time.perf_counter() - start) * 1000

        return BenchmarkResult(
            database="ProximaDB",
            operation="graph_stats",
            config=self.config.name,
            time_ms=elapsed_ms
        )


# =============================================================================
# NetworkX Benchmark
# =============================================================================

class NetworkXBenchmark:
    def __init__(self, config: GraphConfig):
        self.config = config
        self.graph = None

    def setup(self):
        self.graph = nx.DiGraph()

    def teardown(self):
        self.graph = None

    def benchmark_bulk_insert(self, nodes: List[Dict], edges: List[Dict]) -> BenchmarkResult:
        start = time.perf_counter()

        for node in nodes:
            self.graph.add_node(node["id"], **node["properties"])

        for edge in edges:
            self.graph.add_edge(
                edge["from_node_id"],
                edge["to_node_id"],
                edge_type=edge["edge_type"],
                weight=edge.get("weight", 1.0)
            )

        elapsed_ms = (time.perf_counter() - start) * 1000
        total_ops = len(nodes) + len(edges)
        throughput = total_ops / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="NetworkX",
            operation="bulk_insert",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_node_lookup(self, node_ids: List[str]) -> BenchmarkResult:
        start = time.perf_counter()

        for node_id in node_ids:
            node_data = self.graph.nodes[node_id] if node_id in self.graph else None

        elapsed_ms = (time.perf_counter() - start) * 1000
        throughput = len(node_ids) / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="NetworkX",
            operation="node_lookup",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_neighbor_query(self, node_ids: List[str]) -> BenchmarkResult:
        start = time.perf_counter()

        for node_id in node_ids:
            neighbors = list(self.graph.successors(node_id))

        elapsed_ms = (time.perf_counter() - start) * 1000
        throughput = len(node_ids) / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="NetworkX",
            operation="neighbor_query",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_graph_stats(self) -> BenchmarkResult:
        start = time.perf_counter()

        num_nodes = self.graph.number_of_nodes()
        num_edges = self.graph.number_of_edges()

        elapsed_ms = (time.perf_counter() - start) * 1000

        return BenchmarkResult(
            database="NetworkX",
            operation="graph_stats",
            config=self.config.name,
            time_ms=elapsed_ms
        )


# =============================================================================
# igraph Benchmark
# =============================================================================

class IGraphBenchmark:
    def __init__(self, config: GraphConfig):
        self.config = config
        self.graph = None
        self.node_map = {}

    def setup(self):
        self.graph = ig.Graph(directed=True)
        self.node_map = {}

    def teardown(self):
        self.graph = None
        self.node_map = {}

    def benchmark_bulk_insert(self, nodes: List[Dict], edges: List[Dict]) -> BenchmarkResult:
        start = time.perf_counter()

        self.graph.add_vertices(len(nodes))
        for i, node in enumerate(nodes):
            self.node_map[node["id"]] = i
            self.graph.vs[i]["name"] = node["id"]
            for k, v in node["properties"].items():
                self.graph.vs[i][k] = v

        edge_list = []
        for edge in edges:
            from_idx = self.node_map[edge["from_node_id"]]
            to_idx = self.node_map[edge["to_node_id"]]
            edge_list.append((from_idx, to_idx))

        self.graph.add_edges(edge_list)

        elapsed_ms = (time.perf_counter() - start) * 1000
        total_ops = len(nodes) + len(edges)
        throughput = total_ops / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="igraph",
            operation="bulk_insert",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_node_lookup(self, node_ids: List[str]) -> BenchmarkResult:
        start = time.perf_counter()

        for node_id in node_ids:
            idx = self.node_map.get(node_id)
            if idx is not None:
                vertex = self.graph.vs[idx]

        elapsed_ms = (time.perf_counter() - start) * 1000
        throughput = len(node_ids) / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="igraph",
            operation="node_lookup",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_neighbor_query(self, node_ids: List[str]) -> BenchmarkResult:
        start = time.perf_counter()

        for node_id in node_ids:
            idx = self.node_map.get(node_id)
            if idx is not None:
                neighbors = self.graph.neighbors(idx, mode="out")

        elapsed_ms = (time.perf_counter() - start) * 1000
        throughput = len(node_ids) / (elapsed_ms / 1000)

        return BenchmarkResult(
            database="igraph",
            operation="neighbor_query",
            config=self.config.name,
            time_ms=elapsed_ms,
            throughput_ops_sec=throughput
        )

    def benchmark_graph_stats(self) -> BenchmarkResult:
        start = time.perf_counter()

        num_nodes = self.graph.vcount()
        num_edges = self.graph.ecount()

        elapsed_ms = (time.perf_counter() - start) * 1000

        return BenchmarkResult(
            database="igraph",
            operation="graph_stats",
            config=self.config.name,
            time_ms=elapsed_ms
        )


# =============================================================================
# Runner
# =============================================================================

def run_benchmark_suite(config: GraphConfig) -> List[BenchmarkResult]:
    results = []

    print(f"\n{'='*80}")
    print(f"Benchmark: {config.name} ({config.num_nodes:,} nodes, {config.num_edges:,} edges)")
    print(f"{'='*80}\n")

    nodes, edges = generate_graph_data(config)
    sample_nodes = [f"node_{random.randint(0, config.num_nodes-1)}" for _ in range(100)]

    benchmarks = []
    if PROXIMADB_AVAILABLE:
        benchmarks.append(("ProximaDB", ProximaDBBenchmark))
    if NETWORKX_AVAILABLE:
        benchmarks.append(("NetworkX", NetworkXBenchmark))
    if IGRAPH_AVAILABLE:
        benchmarks.append(("igraph", IGraphBenchmark))

    for db_name, BenchmarkClass in benchmarks:
        print(f"\n{db_name}:")
        print(f"{'-'*40}")

        bench = BenchmarkClass(config)

        try:
            bench.setup()

            print(f"  Bulk insert... ", end='', flush=True)
            r = bench.benchmark_bulk_insert(nodes, edges)
            results.append(r)
            print(f"{r.time_ms:>8.1f}ms ({r.throughput_ops_sec:>8,.0f} ops/sec)")

            print(f"  Node lookup... ", end='', flush=True)
            r = bench.benchmark_node_lookup(sample_nodes)
            results.append(r)
            print(f"{r.time_ms:>8.1f}ms ({r.throughput_ops_sec:>8,.0f} ops/sec)")

            print(f"  Neighbor query...", end='', flush=True)
            r = bench.benchmark_neighbor_query(sample_nodes[:50])
            results.append(r)
            print(f"{r.time_ms:>8.1f}ms ({r.throughput_ops_sec:>8,.0f} ops/sec)")

            print(f"  Graph stats... ", end='', flush=True)
            r = bench.benchmark_graph_stats()
            results.append(r)
            print(f"{r.time_ms:>8.1f}ms")

        except Exception as e:
            print(f"\n  ✗ Error: {str(e)}")
            traceback.print_exc()
        finally:
            bench.teardown()
            gc.collect()

    return results


def print_summary(results: List[BenchmarkResult]):
    by_op = defaultdict(list)
    for r in results:
        by_op[(r.config, r.operation)].append(r)

    print(f"\n{'='*80}")
    print("SUMMARY")
    print(f"{'='*80}\n")

    for (config, op), op_results in sorted(by_op.items()):
        print(f"\n{op.upper()} ({config})")
        print(f"{'-'*80}")
        print(f"{'Database':<15} {'Time (ms)':>12} {'Throughput':>15} {'Speedup':>12}")
        print(f"{'-'*80}")

        baseline = max(r.time_ms for r in op_results)

        for r in sorted(op_results, key=lambda x: x.time_ms):
            speedup = baseline / r.time_ms if r.time_ms > 0 else 0
            thr = f"{r.throughput_ops_sec:,.0f}" if r.throughput_ops_sec else "N/A"
            print(f"{r.database:<15} {r.time_ms:>12.2f} {thr:>15} {speedup:>11.2f}x")


def main():
    all_results = []

    configs = [GraphConfig.small(), GraphConfig.medium()]

    if len(sys.argv) > 1 and sys.argv[1] == "--large":
        configs.append(GraphConfig.large())

    for config in configs:
        results = run_benchmark_suite(config)
        all_results.extend(results)

    print_summary(all_results)

    with open("graph_benchmark_results.json", 'w') as f:
        json.dump([asdict(r) for r in all_results], f, indent=2)
    print(f"\n✓ Results saved to: graph_benchmark_results.json\n")


if __name__ == "__main__":
    main()
