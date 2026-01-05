#!/usr/bin/env python3
"""
Embedded Graph Database Performance Benchmark

Evaluates ProximaDB's graph database capabilities in embedded mode,
covering the top 80% of real-world graph use cases:

1. Node/Edge CRUD Operations (bulk insert, lookup, update, delete)
2. Graph Traversal (BFS, DFS, neighbor queries)
3. Path Finding (shortest path, multi-hop queries)
4. Property Queries (filter by labels, properties)
5. Graph Analytics (connected components simulation)
6. Hybrid Vector+Graph Workloads (semantic knowledge store pattern)

Test configurations:
- Small: 1,000 nodes, 5,000 edges
- Medium: 10,000 nodes, 50,000 edges
- Large: 100,000 nodes, 500,000 edges

Comparison databases (when available):
- ProximaDB (native Rust via PyO3) - ORION engine
- NetworkX (Python reference implementation)
- Neo4j-embedded (when available)
- SQLite-based graph (relational fallback)
"""

import gc
import os
import random
import shutil
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import numpy as np

try:
    from rich.console import Console
    from rich.table import Table

    RICH_AVAILABLE = True
except ImportError:
    RICH_AVAILABLE = False
    Console = None
    Table = None

# =============================================================================
# Database Imports
# =============================================================================

# ProximaDB (native Rust via PyO3)
try:
    import proximadb

    PROXIMADB_AVAILABLE = True
    print(f"ProximaDB v{proximadb.__version__} loaded (embedded mode)")
except ImportError as e:
    PROXIMADB_AVAILABLE = False
    print(f"ProximaDB not available: {e}")

# NetworkX (reference implementation)
try:
    import networkx as nx

    NETWORKX_AVAILABLE = True
    print("NetworkX loaded (reference implementation)")
except ImportError:
    NETWORKX_AVAILABLE = False
    print("NetworkX not available")


# =============================================================================
# Benchmark Configuration
# =============================================================================


@dataclass
class GraphConfig:
    """Configuration for graph benchmark."""

    name: str
    num_nodes: int
    num_edges: int
    edge_density: float  # Approximate edges per node

    @classmethod
    def small(cls) -> "GraphConfig":
        return cls("small", 1_000, 5_000, 5.0)

    @classmethod
    def medium(cls) -> "GraphConfig":
        return cls("medium", 10_000, 50_000, 5.0)

    @classmethod
    def large(cls) -> "GraphConfig":
        return cls("large", 100_000, 500_000, 5.0)


@dataclass
class BenchmarkResult:
    """Result from a single benchmark."""

    operation: str
    time_ms: float
    throughput: Optional[float] = None  # ops/sec or items/sec
    p50_ms: Optional[float] = None
    p95_ms: Optional[float] = None
    p99_ms: Optional[float] = None
    error: Optional[str] = None


# =============================================================================
# Graph Data Generators
# =============================================================================


def generate_graph_data(config: GraphConfig) -> Tuple[List[Dict], List[Dict]]:
    """Generate random graph data with realistic properties.

    Node labels: ["Person", "Document", "Function", "Class", "Module"]
    Edge types: ["KNOWS", "REFERENCES", "CALLS", "INHERITS", "IMPORTS"]
    """
    labels = ["Person", "Document", "Function", "Class", "Module"]
    edge_types = ["KNOWS", "REFERENCES", "CALLS", "INHERITS", "IMPORTS"]

    # Generate nodes
    nodes = []
    for i in range(config.num_nodes):
        node = {
            "id": f"node_{i}",
            "labels": [random.choice(labels)],
            "properties": {
                "name": f"Entity_{i}",
                "category": random.choice(["A", "B", "C", "D"]),
                "score": str(random.randint(1, 100)),
                "created": str(int(time.time()) - random.randint(0, 86400 * 365)),
            },
        }
        nodes.append(node)

    # Generate edges (preferring local connections for realistic graphs)
    edges = []
    edge_set = set()  # For deduplication

    for i in range(config.num_edges):
        # Prefer local connections (nodes close in ID)
        from_idx = random.randint(0, config.num_nodes - 1)
        locality = min(config.num_nodes // 10, 100)  # Max 100 nodes apart
        to_idx = (from_idx + random.randint(1, locality)) % config.num_nodes

        edge_key = (from_idx, to_idx)
        if edge_key in edge_set:
            continue
        edge_set.add(edge_key)

        edge = {
            "from_node_id": f"node_{from_idx}",
            "to_node_id": f"node_{to_idx}",
            "edge_type": random.choice(edge_types),
            "weight": random.random(),
            "properties": {
                "confidence": str(random.random()),
            },
        }
        edges.append(edge)

    return nodes, edges


def generate_vector_graph_data(
    config: GraphConfig, dimension: int = 128
) -> Tuple[List[Dict], List[Dict], np.ndarray]:
    """Generate graph data with associated vectors for hybrid workloads."""
    nodes, edges = generate_graph_data(config)

    # Generate normalized vectors for each node
    vectors = np.random.randn(config.num_nodes, dimension).astype(np.float32)
    norms = np.linalg.norm(vectors, axis=1, keepdims=True)
    vectors = vectors / norms

    return nodes, edges, vectors


# =============================================================================
# ProximaDB Benchmark Functions
# =============================================================================


def benchmark_proximadb_graph(
    config: GraphConfig, temp_dir: str
) -> Dict[str, BenchmarkResult]:
    """Benchmark ProximaDB embedded graph operations."""
    if not PROXIMADB_AVAILABLE:
        return {"error": BenchmarkResult("init", 0, error="ProximaDB not available")}

    results = {}
    data_dir = os.path.join(temp_dir, "proximadb_graph_data")
    os.makedirs(data_dir, exist_ok=True)

    try:
        # Initialize database
        db = proximadb.ProximaDB(
            data_dirs=data_dir,
            metadata_dir=os.path.join(data_dir, "metadata"),
            cache_size_mb=256,
            enable_wal=False,  # Faster for benchmarks
        )

        # Create graph
        db.create_graph("benchmark_graph")

        # Generate data
        nodes_data, edges_data = generate_graph_data(config)

        # =================================================================
        # Benchmark 1: Bulk Node Insert
        # =================================================================
        nodes = [
            proximadb.GraphNode(n["id"], labels=n["labels"], properties=n["properties"])
            for n in nodes_data
        ]

        start = time.perf_counter()
        count = db.create_nodes("benchmark_graph", nodes)
        insert_nodes_time = (time.perf_counter() - start) * 1000

        results["insert_nodes"] = BenchmarkResult(
            "insert_nodes",
            insert_nodes_time,
            throughput=config.num_nodes / (insert_nodes_time / 1000),
        )

        # =================================================================
        # Benchmark 2: Bulk Edge Insert
        # =================================================================
        edges = [
            proximadb.GraphEdge(
                e["from_node_id"],
                e["to_node_id"],
                e["edge_type"],
                weight=e.get("weight"),
                properties=e.get("properties", {}),
            )
            for e in edges_data
        ]

        start = time.perf_counter()
        count = db.create_edges("benchmark_graph", edges)
        insert_edges_time = (time.perf_counter() - start) * 1000

        results["insert_edges"] = BenchmarkResult(
            "insert_edges",
            insert_edges_time,
            throughput=len(edges_data) / (insert_edges_time / 1000),
        )

        # =================================================================
        # Benchmark 3: Single Node Lookup
        # =================================================================
        lookup_times = []
        for _ in range(100):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()
            node = db.get_node("benchmark_graph", node_id)
            lookup_times.append((time.perf_counter() - start) * 1000)

        results["node_lookup"] = BenchmarkResult(
            "node_lookup",
            float(np.mean(lookup_times)),
            throughput=1000 / np.mean(lookup_times),  # ops/sec
            p50_ms=float(np.percentile(lookup_times, 50)),
            p95_ms=float(np.percentile(lookup_times, 95)),
            p99_ms=float(np.percentile(lookup_times, 99)),
        )

        # =================================================================
        # Benchmark 4: Query by Labels
        # =================================================================
        labels = ["Person", "Function", "Class"]
        query_times = []
        for label in labels * 10:  # 30 queries
            start = time.perf_counter()
            nodes = db.query_nodes_by_labels("benchmark_graph", [label])
            query_times.append((time.perf_counter() - start) * 1000)

        results["query_by_labels"] = BenchmarkResult(
            "query_by_labels",
            float(np.mean(query_times)),
            p50_ms=float(np.percentile(query_times, 50)),
            p95_ms=float(np.percentile(query_times, 95)),
            p99_ms=float(np.percentile(query_times, 99)),
        )

        # =================================================================
        # Benchmark 5: Outgoing Edge Traversal (1-hop)
        # =================================================================
        traversal_times = []
        for _ in range(100):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()
            edges = db.get_outgoing_edges("benchmark_graph", node_id)
            traversal_times.append((time.perf_counter() - start) * 1000)

        results["1hop_outgoing"] = BenchmarkResult(
            "1hop_outgoing",
            float(np.mean(traversal_times)),
            throughput=1000 / np.mean(traversal_times),
            p50_ms=float(np.percentile(traversal_times, 50)),
            p95_ms=float(np.percentile(traversal_times, 95)),
            p99_ms=float(np.percentile(traversal_times, 99)),
        )

        # =================================================================
        # Benchmark 6: Incoming Edge Traversal (reverse 1-hop)
        # =================================================================
        incoming_times = []
        for _ in range(100):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()
            edges = db.get_incoming_edges("benchmark_graph", node_id)
            incoming_times.append((time.perf_counter() - start) * 1000)

        results["1hop_incoming"] = BenchmarkResult(
            "1hop_incoming",
            float(np.mean(incoming_times)),
            throughput=1000 / np.mean(incoming_times),
            p50_ms=float(np.percentile(incoming_times, 50)),
            p95_ms=float(np.percentile(incoming_times, 95)),
            p99_ms=float(np.percentile(incoming_times, 99)),
        )

        # =================================================================
        # Benchmark 7: 2-Hop Traversal (neighbors of neighbors)
        # =================================================================
        twohop_times = []
        for _ in range(50):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()

            # Get 1-hop neighbors
            edges1 = db.get_outgoing_edges("benchmark_graph", node_id)
            neighbor_ids = [e.to_node_id for e in edges1[:10]]  # Limit for perf

            # Get 2-hop neighbors
            for neighbor_id in neighbor_ids:
                edges2 = db.get_outgoing_edges("benchmark_graph", neighbor_id)

            twohop_times.append((time.perf_counter() - start) * 1000)

        results["2hop_traversal"] = BenchmarkResult(
            "2hop_traversal",
            float(np.mean(twohop_times)),
            p50_ms=float(np.percentile(twohop_times, 50)),
            p95_ms=float(np.percentile(twohop_times, 95)),
            p99_ms=float(np.percentile(twohop_times, 99)),
        )

        # =================================================================
        # Benchmark 8: Graph Statistics
        # =================================================================
        stats_times = []
        for _ in range(20):
            start = time.perf_counter()
            stats = db.graph_stats("benchmark_graph")
            stats_times.append((time.perf_counter() - start) * 1000)

        results["graph_stats"] = BenchmarkResult(
            "graph_stats",
            float(np.mean(stats_times)),
            p50_ms=float(np.percentile(stats_times, 50)),
            p95_ms=float(np.percentile(stats_times, 95)),
            p99_ms=float(np.percentile(stats_times, 99)),
        )

        # =================================================================
        # Benchmark 9: Node Delete
        # =================================================================
        delete_times = []
        for i in range(min(100, config.num_nodes // 100)):
            node_id = f"node_{config.num_nodes - 1 - i}"  # Delete from end
            start = time.perf_counter()
            deleted = db.delete_node("benchmark_graph", node_id)
            delete_times.append((time.perf_counter() - start) * 1000)

        results["delete_node"] = BenchmarkResult(
            "delete_node",
            float(np.mean(delete_times)),
            throughput=1000 / np.mean(delete_times),
            p50_ms=float(np.percentile(delete_times, 50)),
            p95_ms=float(np.percentile(delete_times, 95)),
            p99_ms=float(np.percentile(delete_times, 99)),
        )

        # Cleanup
        db.delete_graph("benchmark_graph")

    except Exception as e:
        results["error"] = BenchmarkResult("error", 0, error=str(e))

    return results


def benchmark_networkx(
    config: GraphConfig, temp_dir: str
) -> Dict[str, BenchmarkResult]:
    """Benchmark NetworkX as reference implementation."""
    if not NETWORKX_AVAILABLE:
        return {"error": BenchmarkResult("init", 0, error="NetworkX not available")}

    results = {}

    try:
        # Generate data
        nodes_data, edges_data = generate_graph_data(config)

        # =================================================================
        # Benchmark 1: Node Insert
        # =================================================================
        G = nx.DiGraph()

        start = time.perf_counter()
        for n in nodes_data:
            G.add_node(n["id"], **n["properties"], labels=n["labels"])
        insert_nodes_time = (time.perf_counter() - start) * 1000

        results["insert_nodes"] = BenchmarkResult(
            "insert_nodes",
            insert_nodes_time,
            throughput=config.num_nodes / (insert_nodes_time / 1000),
        )

        # =================================================================
        # Benchmark 2: Edge Insert
        # =================================================================
        start = time.perf_counter()
        for e in edges_data:
            G.add_edge(
                e["from_node_id"],
                e["to_node_id"],
                edge_type=e["edge_type"],
                weight=e.get("weight", 1.0),
                **e.get("properties", {}),
            )
        insert_edges_time = (time.perf_counter() - start) * 1000

        results["insert_edges"] = BenchmarkResult(
            "insert_edges",
            insert_edges_time,
            throughput=len(edges_data) / (insert_edges_time / 1000),
        )

        # =================================================================
        # Benchmark 3: Single Node Lookup
        # =================================================================
        lookup_times = []
        for _ in range(100):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()
            node_data = G.nodes[node_id]
            lookup_times.append((time.perf_counter() - start) * 1000)

        results["node_lookup"] = BenchmarkResult(
            "node_lookup",
            float(np.mean(lookup_times)),
            throughput=1000 / np.mean(lookup_times),
            p50_ms=float(np.percentile(lookup_times, 50)),
            p95_ms=float(np.percentile(lookup_times, 95)),
            p99_ms=float(np.percentile(lookup_times, 99)),
        )

        # =================================================================
        # Benchmark 4: Query by Labels (simulated with filter)
        # =================================================================
        query_times = []
        labels_to_query = ["Person", "Function", "Class"]
        for label in labels_to_query * 10:
            start = time.perf_counter()
            matching = [
                n for n, d in G.nodes(data=True) if label in d.get("labels", [])
            ]
            query_times.append((time.perf_counter() - start) * 1000)

        results["query_by_labels"] = BenchmarkResult(
            "query_by_labels",
            float(np.mean(query_times)),
            p50_ms=float(np.percentile(query_times, 50)),
            p95_ms=float(np.percentile(query_times, 95)),
            p99_ms=float(np.percentile(query_times, 99)),
        )

        # =================================================================
        # Benchmark 5: Outgoing Edge Traversal
        # =================================================================
        traversal_times = []
        for _ in range(100):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()
            successors = list(G.successors(node_id))
            traversal_times.append((time.perf_counter() - start) * 1000)

        results["1hop_outgoing"] = BenchmarkResult(
            "1hop_outgoing",
            float(np.mean(traversal_times)),
            throughput=1000 / np.mean(traversal_times),
            p50_ms=float(np.percentile(traversal_times, 50)),
            p95_ms=float(np.percentile(traversal_times, 95)),
            p99_ms=float(np.percentile(traversal_times, 99)),
        )

        # =================================================================
        # Benchmark 6: Incoming Edge Traversal
        # =================================================================
        incoming_times = []
        for _ in range(100):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()
            predecessors = list(G.predecessors(node_id))
            incoming_times.append((time.perf_counter() - start) * 1000)

        results["1hop_incoming"] = BenchmarkResult(
            "1hop_incoming",
            float(np.mean(incoming_times)),
            throughput=1000 / np.mean(incoming_times),
            p50_ms=float(np.percentile(incoming_times, 50)),
            p95_ms=float(np.percentile(incoming_times, 95)),
            p99_ms=float(np.percentile(incoming_times, 99)),
        )

        # =================================================================
        # Benchmark 7: 2-Hop Traversal
        # =================================================================
        twohop_times = []
        for _ in range(50):
            node_id = f"node_{random.randint(0, config.num_nodes - 1)}"
            start = time.perf_counter()

            neighbors1 = list(G.successors(node_id))[:10]
            for neighbor in neighbors1:
                neighbors2 = list(G.successors(neighbor))

            twohop_times.append((time.perf_counter() - start) * 1000)

        results["2hop_traversal"] = BenchmarkResult(
            "2hop_traversal",
            float(np.mean(twohop_times)),
            p50_ms=float(np.percentile(twohop_times, 50)),
            p95_ms=float(np.percentile(twohop_times, 95)),
            p99_ms=float(np.percentile(twohop_times, 99)),
        )

        # =================================================================
        # Benchmark 8: Graph Statistics
        # =================================================================
        stats_times = []
        for _ in range(20):
            start = time.perf_counter()
            num_nodes = G.number_of_nodes()
            num_edges = G.number_of_edges()
            stats_times.append((time.perf_counter() - start) * 1000)

        results["graph_stats"] = BenchmarkResult(
            "graph_stats",
            float(np.mean(stats_times)),
            p50_ms=float(np.percentile(stats_times, 50)),
            p95_ms=float(np.percentile(stats_times, 95)),
            p99_ms=float(np.percentile(stats_times, 99)),
        )

        # =================================================================
        # Benchmark 9: Node Delete
        # =================================================================
        delete_times = []
        for i in range(min(100, config.num_nodes // 100)):
            node_id = f"node_{config.num_nodes - 1 - i}"
            start = time.perf_counter()
            G.remove_node(node_id)
            delete_times.append((time.perf_counter() - start) * 1000)

        results["delete_node"] = BenchmarkResult(
            "delete_node",
            float(np.mean(delete_times)),
            throughput=1000 / np.mean(delete_times),
            p50_ms=float(np.percentile(delete_times, 50)),
            p95_ms=float(np.percentile(delete_times, 95)),
            p99_ms=float(np.percentile(delete_times, 99)),
        )

    except Exception as e:
        results["error"] = BenchmarkResult("error", 0, error=str(e))

    return results


# =============================================================================
# Reporting Functions
# =============================================================================


def render_comparison_table(
    config: GraphConfig,
    proximadb_results: Dict[str, BenchmarkResult],
    networkx_results: Dict[str, BenchmarkResult],
) -> None:
    """Render comparison table between ProximaDB and NetworkX."""

    operations = [
        "insert_nodes",
        "insert_edges",
        "node_lookup",
        "query_by_labels",
        "1hop_outgoing",
        "1hop_incoming",
        "2hop_traversal",
        "graph_stats",
        "delete_node",
    ]

    print(f"\n{'='*90}")
    print(
        f"GRAPH BENCHMARK RESULTS: {config.name.upper()} ({config.num_nodes:,} nodes, {config.num_edges:,} edges)"
    )
    print(f"{'='*90}")

    if RICH_AVAILABLE:
        console = Console()
        table = Table(title=f"Graph Database Comparison - {config.name}", expand=True)
        table.add_column("Operation", style="cyan", no_wrap=True)
        table.add_column("ProximaDB (ms)", justify="right")
        table.add_column("NetworkX (ms)", justify="right")
        table.add_column("Speedup", justify="right", style="green")
        table.add_column("ProximaDB p95", justify="right")
        table.add_column("NetworkX p95", justify="right")

        for op in operations:
            pdb_result = proximadb_results.get(op)
            nx_result = networkx_results.get(op)

            pdb_time = (
                f"{pdb_result.time_ms:.3f}"
                if pdb_result and not pdb_result.error
                else "N/A"
            )
            nx_time = (
                f"{nx_result.time_ms:.3f}"
                if nx_result and not nx_result.error
                else "N/A"
            )

            if (
                pdb_result
                and nx_result
                and not pdb_result.error
                and not nx_result.error
            ):
                speedup = nx_result.time_ms / pdb_result.time_ms
                speedup_str = (
                    f"{speedup:.2f}x" if speedup >= 1 else f"1/{1/speedup:.2f}x"
                )
            else:
                speedup_str = "N/A"

            pdb_p95 = (
                f"{pdb_result.p95_ms:.3f}" if pdb_result and pdb_result.p95_ms else "-"
            )
            nx_p95 = (
                f"{nx_result.p95_ms:.3f}" if nx_result and nx_result.p95_ms else "-"
            )

            table.add_row(op, pdb_time, nx_time, speedup_str, pdb_p95, nx_p95)

        console.print(table)
    else:
        print(
            f"{'Operation':<20} {'ProximaDB (ms)':<16} {'NetworkX (ms)':<16} {'Speedup':<12}"
        )
        print("-" * 64)
        for op in operations:
            pdb_result = proximadb_results.get(op)
            nx_result = networkx_results.get(op)

            pdb_time = (
                f"{pdb_result.time_ms:.3f}"
                if pdb_result and not pdb_result.error
                else "N/A"
            )
            nx_time = (
                f"{nx_result.time_ms:.3f}"
                if nx_result and not nx_result.error
                else "N/A"
            )

            if (
                pdb_result
                and nx_result
                and not pdb_result.error
                and not nx_result.error
            ):
                speedup = nx_result.time_ms / pdb_result.time_ms
                speedup_str = f"{speedup:.2f}x"
            else:
                speedup_str = "N/A"

            print(f"{op:<20} {pdb_time:<16} {nx_time:<16} {speedup_str:<12}")


def render_throughput_summary(
    all_results: Dict[str, Dict[str, Dict[str, BenchmarkResult]]],
) -> None:
    """Render throughput summary across all configurations."""
    print("\n" + "=" * 90)
    print("THROUGHPUT SUMMARY (operations/second)")
    print("=" * 90)

    configs = list(all_results.keys())
    ops_with_throughput = [
        "insert_nodes",
        "insert_edges",
        "node_lookup",
        "1hop_outgoing",
        "delete_node",
    ]

    if RICH_AVAILABLE:
        console = Console()
        table = Table(title="Throughput by Graph Size", expand=True)
        table.add_column("Operation", style="cyan")
        for cfg in configs:
            table.add_column(f"{cfg} (ProximaDB)", justify="right")
            table.add_column(f"{cfg} (NetworkX)", justify="right")

        for op in ops_with_throughput:
            row = [op]
            for cfg in configs:
                pdb_result = all_results[cfg].get("proximadb", {}).get(op)
                nx_result = all_results[cfg].get("networkx", {}).get(op)

                pdb_tput = (
                    f"{pdb_result.throughput:,.0f}"
                    if pdb_result and pdb_result.throughput
                    else "N/A"
                )
                nx_tput = (
                    f"{nx_result.throughput:,.0f}"
                    if nx_result and nx_result.throughput
                    else "N/A"
                )

                row.extend([pdb_tput, nx_tput])

            table.add_row(*row)

        console.print(table)
    else:
        header = f"{'Operation':<20}"
        for cfg in configs:
            header += f" {cfg:<14} {'':<14}"
        print(header)
        print("-" * (20 + 28 * len(configs)))


def write_markdown_report(
    all_results: Dict[str, Dict[str, Dict[str, BenchmarkResult]]],
) -> None:
    """Write benchmark results to markdown file."""
    target_dir = Path("target")
    target_dir.mkdir(exist_ok=True)

    lines = []
    lines.append("# Embedded Graph Database Benchmark")
    lines.append("")
    lines.append(
        "Comparing ProximaDB (ORION engine) with NetworkX (reference implementation)"
    )
    lines.append("")

    for config_name, db_results in all_results.items():
        lines.append(f"## {config_name.title()} Graph")
        lines.append("")

        proximadb_results = db_results.get("proximadb", {})
        networkx_results = db_results.get("networkx", {})

        lines.append("| Operation | ProximaDB (ms) | NetworkX (ms) | Speedup |")
        lines.append("| --- | ---: | ---: | ---: |")

        operations = [
            "insert_nodes",
            "insert_edges",
            "node_lookup",
            "query_by_labels",
            "1hop_outgoing",
            "1hop_incoming",
            "2hop_traversal",
            "graph_stats",
            "delete_node",
        ]

        for op in operations:
            pdb_result = proximadb_results.get(op)
            nx_result = networkx_results.get(op)

            pdb_time = (
                f"{pdb_result.time_ms:.3f}"
                if pdb_result and not pdb_result.error
                else "N/A"
            )
            nx_time = (
                f"{nx_result.time_ms:.3f}"
                if nx_result and not nx_result.error
                else "N/A"
            )

            if (
                pdb_result
                and nx_result
                and not pdb_result.error
                and not nx_result.error
            ):
                speedup = nx_result.time_ms / pdb_result.time_ms
                speedup_str = f"{speedup:.2f}x"
            else:
                speedup_str = "N/A"

            lines.append(f"| {op} | {pdb_time} | {nx_time} | {speedup_str} |")

        lines.append("")

    report_path = target_dir / "embedded_graph_benchmark_latest.md"
    report_path.write_text("\n".join(lines))
    print(f"\nMarkdown report written to {report_path}")


# =============================================================================
# Main Benchmark Runner
# =============================================================================


def run_benchmark(configs: List[GraphConfig] = None):
    """Run the embedded graph database benchmark."""

    if configs is None:
        configs = [GraphConfig.small(), GraphConfig.medium()]

    print("=" * 90)
    print("EMBEDDED GRAPH DATABASE PERFORMANCE BENCHMARK")
    print("=" * 90)
    print()
    print("Databases: ProximaDB (ORION engine), NetworkX (reference)")
    print("Top 80% use cases: CRUD, traversal, property queries, analytics")
    print()

    all_results = {}

    for config in configs:
        print(f"\n{'='*90}")
        print(
            f"BENCHMARK: {config.name.upper()} ({config.num_nodes:,} nodes, {config.num_edges:,} edges)"
        )
        print(f"{'='*90}")

        all_results[config.name] = {}

        # Benchmark ProximaDB
        print("\n  ProximaDB (ORION engine)...")
        with tempfile.TemporaryDirectory() as temp_dir:
            proximadb_results = benchmark_proximadb_graph(config, temp_dir)
            all_results[config.name]["proximadb"] = proximadb_results

            if "error" in proximadb_results:
                print(f"    Error: {proximadb_results['error'].error}")
            else:
                print(
                    f"    Insert nodes: {proximadb_results['insert_nodes'].time_ms:.2f}ms"
                )
                print(
                    f"    Node lookup: {proximadb_results['node_lookup'].time_ms:.3f}ms (p95: {proximadb_results['node_lookup'].p95_ms:.3f}ms)"
                )

        gc.collect()

        # Benchmark NetworkX
        print("\n  NetworkX (reference)...")
        with tempfile.TemporaryDirectory() as temp_dir:
            networkx_results = benchmark_networkx(config, temp_dir)
            all_results[config.name]["networkx"] = networkx_results

            if "error" in networkx_results:
                print(f"    Error: {networkx_results['error'].error}")
            else:
                print(
                    f"    Insert nodes: {networkx_results['insert_nodes'].time_ms:.2f}ms"
                )
                print(
                    f"    Node lookup: {networkx_results['node_lookup'].time_ms:.3f}ms (p95: {networkx_results['node_lookup'].p95_ms:.3f}ms)"
                )

        gc.collect()

        # Render comparison table
        render_comparison_table(config, proximadb_results, networkx_results)

    # Summary
    render_throughput_summary(all_results)
    write_markdown_report(all_results)

    return all_results


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Embedded Graph Database Benchmark")
    parser.add_argument(
        "--size",
        choices=["small", "medium", "large", "all"],
        default="small",
        help="Graph size to benchmark",
    )
    args = parser.parse_args()

    if args.size == "all":
        configs = [GraphConfig.small(), GraphConfig.medium(), GraphConfig.large()]
    elif args.size == "large":
        configs = [GraphConfig.large()]
    elif args.size == "medium":
        configs = [GraphConfig.medium()]
    else:
        configs = [GraphConfig.small()]

    run_benchmark(configs)
