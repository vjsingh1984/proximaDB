#!/usr/bin/env python3
"""
Embedded Vector Database Performance Benchmark

Evaluates batch insert and search performance across popular embedded vector
databases for use in CLI applications requiring in-process vector storage.

Databases tested:
- ProximaDB (native Rust via PyO3) with all storage engines:
  - SST: Write-optimized, real-time ingestion
  - HELIX: Locality-optimized with Hilbert curve indexing
  - VIPER: Columnar Parquet for analytics workloads
  - NOVA: Progressive columnar for mixed workloads
  - SWIFT: Ultra-low latency for small datasets
  - RAPTOR: Adaptive row-group for dynamic workloads
- ChromaDB (Python-native with SQLite/HNSW)
- LanceDB (columnar Arrow format)
- FAISS (Facebook AI Similarity Search)
- Qdrant (Rust-based with filtering)

Test configurations: 1024, 5000, and 10000 vectors (dimension=384)
"""

import gc
import os
import sys
import time
import tempfile
import shutil
from pathlib import Path
from typing import List, Dict, Any, Tuple

import numpy as np
try:
    from rich.console import Console
    from rich.table import Table
    RICH_AVAILABLE = True
except ImportError:
    RICH_AVAILABLE = False

# =============================================================================
# Database Imports
# =============================================================================

# ProximaDB (native Rust via PyO3)
try:
    import proximadb_sdk
    PROXIMADB_AVAILABLE = True
    print(f"ProximaDB v{proximadb.__version__} loaded")
except ImportError as e:
    PROXIMADB_AVAILABLE = False
    print(f"ProximaDB not available: {e}")

# ChromaDB
import chromadb

# LanceDB
import lancedb

# FAISS
try:
    import faiss
    FAISS_AVAILABLE = True
except ImportError:
    FAISS_AVAILABLE = False

# Qdrant
try:
    from qdrant_client import QdrantClient
    from qdrant_client.models import Distance, VectorParams, PointStruct
    QDRANT_AVAILABLE = True
except ImportError:
    QDRANT_AVAILABLE = False


# =============================================================================
# Benchmark Functions
# =============================================================================

def generate_random_vectors(num_vectors: int, dimension: int = 384) -> np.ndarray:
    """Generate random normalized vectors."""
    vectors = np.random.randn(num_vectors, dimension).astype(np.float32)
    norms = np.linalg.norm(vectors, axis=1, keepdims=True)
    return vectors / norms


def benchmark_proximadb(vectors: np.ndarray, temp_dir: str, engine: str = "sst") -> Dict[str, Any]:
    """Benchmark ProximaDB embedded mode with specified storage engine."""
    if not PROXIMADB_AVAILABLE:
        return {"error": "ProximaDB not available"}

    data_dir = os.path.join(temp_dir, f"proximadb_{engine}_data")
    os.makedirs(data_dir, exist_ok=True)

    try:
        disk_config = proximadb.DiskConfig(data_dir, weight=1)
        db = proximadb.ProximaDB([disk_config])
        db.create_collection("benchmark", vectors.shape[1], engine)

        ids = [f"vec_{i}" for i in range(len(vectors))]
        vectors_list = [v.tolist() for v in vectors]

        start = time.perf_counter()
        db.insert("benchmark", ids, vectors_list, None)
        insert_time = time.perf_counter() - start

        query = vectors[0].tolist()
        search_times = []
        results = []
        for _ in range(10):
            start = time.perf_counter()
            results = db.search("benchmark", query, 10)
            search_times.append((time.perf_counter() - start) * 1000)

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": float(np.mean(search_times)),
            "p50_ms": float(np.percentile(search_times, 50)),
            "p95_ms": float(np.percentile(search_times, 95)),
            "p99_ms": float(np.percentile(search_times, 99)),
            "docs_per_sec": len(vectors) / insert_time,
            "num_results": len(results) if results else 0,
        }
    except Exception as e:
        return {"error": str(e)}


def benchmark_chromadb(vectors: np.ndarray, temp_dir: str) -> Dict[str, Any]:
    """Benchmark ChromaDB embedded mode."""
    data_dir = os.path.join(temp_dir, "chroma_data")

    try:
        client = chromadb.PersistentClient(path=data_dir)
        collection = client.create_collection("benchmark", metadata={"hnsw:space": "cosine"})

        ids = [f"vec_{i}" for i in range(len(vectors))]

        start = time.perf_counter()
        collection.add(ids=ids, embeddings=vectors.tolist())
        insert_time = time.perf_counter() - start

        query = vectors[0:1].tolist()
        search_times = []
        results = {}
        for _ in range(10):
            start = time.perf_counter()
            results = collection.query(query_embeddings=query, n_results=10)
            search_times.append((time.perf_counter() - start) * 1000)

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": float(np.mean(search_times)),
            "p50_ms": float(np.percentile(search_times, 50)),
            "p95_ms": float(np.percentile(search_times, 95)),
            "p99_ms": float(np.percentile(search_times, 99)),
            "docs_per_sec": len(vectors) / insert_time,
            "num_results": len(results['ids'][0]) if results['ids'] else 0,
        }
    except Exception as e:
        return {"error": str(e)}


def benchmark_lancedb(vectors: np.ndarray, temp_dir: str) -> Dict[str, Any]:
    """Benchmark LanceDB embedded mode."""
    data_dir = os.path.join(temp_dir, "lance_data")

    try:
        db = lancedb.connect(data_dir)
        data = [{"id": f"vec_{i}", "vector": vectors[i].tolist()} for i in range(len(vectors))]

        start = time.perf_counter()
        table = db.create_table("benchmark", data)
        insert_time = time.perf_counter() - start

        query = vectors[0].tolist()
        search_times = []
        results = []
        for _ in range(10):
            start = time.perf_counter()
            results = table.search(query).limit(10).to_list()
            search_times.append((time.perf_counter() - start) * 1000)

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": float(np.mean(search_times)),
            "p50_ms": float(np.percentile(search_times, 50)),
            "p95_ms": float(np.percentile(search_times, 95)),
            "p99_ms": float(np.percentile(search_times, 99)),
            "docs_per_sec": len(vectors) / insert_time,
            "num_results": len(results),
        }
    except Exception as e:
        return {"error": str(e)}


def benchmark_faiss(vectors: np.ndarray, temp_dir: str) -> Dict[str, Any]:
    """Benchmark FAISS (in-memory)."""
    if not FAISS_AVAILABLE:
        return {"error": "FAISS not available"}

    try:
        dimension = vectors.shape[1]
        faiss_index = faiss.IndexFlatIP(dimension)

        start = time.perf_counter()
        faiss_index.add(vectors)
        insert_time = time.perf_counter() - start

        query = vectors[0:1]
        search_times = []
        indices = []
        for _ in range(10):
            start = time.perf_counter()
            distances, indices = faiss_index.search(query, 10)
            search_times.append((time.perf_counter() - start) * 1000)

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": float(np.mean(search_times)),
            "p50_ms": float(np.percentile(search_times, 50)),
            "p95_ms": float(np.percentile(search_times, 95)),
            "p99_ms": float(np.percentile(search_times, 99)),
            "docs_per_sec": len(vectors) / insert_time,
            "num_results": len(indices[0]),
        }
    except Exception as e:
        return {"error": str(e)}


def benchmark_qdrant(vectors: np.ndarray, temp_dir: str) -> Dict[str, Any]:
    """Benchmark Qdrant embedded mode."""
    if not QDRANT_AVAILABLE:
        return {"error": "Qdrant not available"}

    data_dir = os.path.join(temp_dir, "qdrant_data")

    try:
        client = QdrantClient(path=data_dir)
        dimension = vectors.shape[1]
        client.create_collection(
            collection_name="benchmark",
            vectors_config=VectorParams(size=dimension, distance=Distance.COSINE)
        )

        points = [
            PointStruct(id=i, vector=vectors[i].tolist())
            for i in range(len(vectors))
        ]

        start = time.perf_counter()
        client.upsert(collection_name="benchmark", points=points)
        insert_time = time.perf_counter() - start

        query = vectors[0].tolist()
        search_times = []
        results = None
        for _ in range(10):
            start = time.perf_counter()
            results = client.query_points(
                collection_name="benchmark",
                query=query,
                limit=10,
            )
            search_times.append((time.perf_counter() - start) * 1000)

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": float(np.mean(search_times)),
            "p50_ms": float(np.percentile(search_times, 50)),
            "p95_ms": float(np.percentile(search_times, 95)),
            "p99_ms": float(np.percentile(search_times, 99)),
            "docs_per_sec": len(vectors) / insert_time,
            "num_results": len(results.points) if results.points else 0,
        }
    except Exception as e:
        return {"error": str(e)}


def run_benchmark(batch_sizes: List[int] = [1024, 5000, 10000]):
    """Run the embedded vector database benchmark."""

    print("=" * 80)
    print("EMBEDDED VECTOR DATABASE PERFORMANCE BENCHMARK")
    print("=" * 80)
    print()
    print("Databases: ProximaDB (6 engines), ChromaDB, LanceDB, FAISS, Qdrant")
    print("All databases run in-process (embedded mode)")
    print()

    dimension = 384  # Standard embedding dimension

    # ProximaDB storage engines to test
    proximadb_engines = ["sst", "helix", "viper", "nova", "swift", "raptor"]

    benchmarks = [
        # ProximaDB with all storage engines
        *[(f"ProximaDB-{eng.upper()}", lambda v, t, e=eng: benchmark_proximadb(v, t, e))
          for eng in proximadb_engines],
        # Other embedded databases
        ("ChromaDB", benchmark_chromadb),
        ("LanceDB", benchmark_lancedb),
        ("FAISS", benchmark_faiss),
        ("Qdrant", benchmark_qdrant),
    ]

    all_results: Dict[int, Dict[str, Dict[str, Any]]] = {}

    for batch_size in batch_sizes:
        print(f"\n{'='*80}")
        print(f"BATCH SIZE: {batch_size:,} vectors (dimension={dimension})")
        print(f"{'='*80}")

        print(f"\nGenerating {batch_size:,} random vectors...")
        vectors = generate_random_vectors(batch_size, dimension)
        print(f"  Shape: {vectors.shape}, Memory: {vectors.nbytes / 1024 / 1024:.1f} MB")

        results = {}

        for db_name, benchmark_fn in benchmarks:
            print(f"\n  {db_name}...")

            with tempfile.TemporaryDirectory() as temp_dir:
                result = benchmark_fn(vectors, temp_dir)
                results[db_name] = result

                if "error" in result:
                    print(f"    Error: {result['error'][:50]}")
                else:
                    print(f"    Insert: {result['insert_time_ms']:.2f}ms ({result['docs_per_sec']:,.0f} vectors/sec)")
                    print(f"    Search: {result['search_time_ms']:.3f}ms ({result['num_results']} results)")

            gc.collect()

        all_results[batch_size] = results

        render_batch_table(batch_size, results)

    # Final summary
    print("\n" + "=" * 80)
    print("SUMMARY: Insert Throughput (vectors/second)")
    print("=" * 80)
    print()

    render_summary_table(all_results, proximadb_engines)
    write_markdown_report(all_results, proximadb_engines)


def render_batch_table(batch_size: int, results: Dict[str, Dict[str, Any]]) -> None:
    """Render per-batch results using Rich if available, otherwise ASCII."""
    header = f"{'='*80}\nRESULTS: {batch_size:,} vectors\n{'='*80}"
    print("\n" + header)

    def fmt_row(name: str, r: Dict[str, Any]) -> str:
        return (f"{name:<18} {r['insert_time_ms']:>10.2f}ms   "
                f"{r['docs_per_sec']:>14,.0f}/s   {r['search_time_ms']:>8.3f}ms   "
                f"p95 {r.get('p95_ms', r['search_time_ms']):>8.3f}ms")

    sorted_results = sorted(
        [(k, v) for k, v in results.items() if "error" not in v],
        key=lambda x: x[1]['docs_per_sec'],
        reverse=True
    )

    if RICH_AVAILABLE:
        console = Console()
        table = Table(title=f"Embedded Vector DBs – {batch_size:,} vectors", expand=True)
        table.add_column("Database", style="cyan", no_wrap=True)
        table.add_column("Insert (ms)", justify="right")
        table.add_column("Throughput (/s)", justify="right")
        table.add_column("Search (ms)", justify="right")
        table.add_column("p95 Search (ms)", justify="right")

        best_tput = max((v["docs_per_sec"] for _, v in sorted_results), default=0)
        best_p95 = min((v.get("p95_ms", v["search_time_ms"]) for _, v in sorted_results), default=0)

        for name, r in sorted_results:
            style = "bold green" if r["docs_per_sec"] == best_tput else None
            p95_style = "bold magenta" if r.get("p95_ms", r["search_time_ms"]) == best_p95 else None
            table.add_row(
                name,
                f"{r['insert_time_ms']:.2f}",
                f"{r['docs_per_sec']:,.0f}",
                f"{r['search_time_ms']:.3f}",
                f"{r.get('p95_ms', r['search_time_ms']):.3f}",
                style=style,
                end_section=False,
            )

        for name, r in results.items():
            if "error" in r:
                table.add_row(name, r["error"], "-", "-", "-", style="red")

        console.print(table)
    else:
        print(f"  {'Database':<18} {'Insert (ms)':<15} {'Throughput':<18} {'Search (ms)':<12} {'p95 (ms)':<10}")
        print(f"  {'─'*78}")
        for name, r in sorted_results:
            print(f"  {fmt_row(name, r)}")
        for name, r in results.items():
            if "error" in r:
                print(f"  {name:<18} Error: {r['error'][:50]}")


def render_summary_table(all_results: Dict[int, Dict[str, Dict[str, Any]]], proximadb_engines: List[str]) -> None:
    """Render cross-batch summary for throughput."""
    print("\n" + "=" * 80)
    print("SUMMARY: Insert Throughput (vectors/second)")
    print("=" * 80)
    batch_sizes = list(all_results.keys())
    db_names = [f"ProximaDB-{eng.upper()}" for eng in proximadb_engines] + ["ChromaDB", "LanceDB", "FAISS", "Qdrant"]

    if RICH_AVAILABLE:
        console = Console()
        table = Table(title="Throughput by Batch Size", expand=True)
        table.add_column("Database", style="cyan")
        for b in batch_sizes:
            table.add_column(f"{b:,} vec", justify="right")

        for db_name in db_names:
            cells = []
            for b in batch_sizes:
                result = all_results[b].get(db_name, {})
                cells.append("N/A" if "error" in result else f"{result['docs_per_sec']:,.0f}")
            table.add_row(db_name, *cells)
        console.print(table)
    else:
        print(f"{'Database':<18}", end="")
        for batch_size in batch_sizes:
            print(f" {batch_size:>12,}", end="")
        print()
        print("-" * (18 + len(batch_sizes) * 14))
        for db_name in db_names:
            print(f"{db_name:<18}", end="")
            for batch_size in batch_sizes:
                result = all_results[batch_size].get(db_name, {})
                if "error" in result:
                    print(f" {'N/A':>12}", end="")
                else:
                    throughput = result['docs_per_sec']
                    print(f" {throughput:>12,.0f}", end="")
            print()


def write_markdown_report(all_results: Dict[int, Dict[str, Dict[str, Any]]], proximadb_engines: List[str]) -> None:
    """Persist a Markdown-friendly snapshot for sharing."""
    target_dir = Path("target")
    target_dir.mkdir(exist_ok=True)
    lines: List[str] = []

    lines.append("# Embedded Vector DB Benchmark")
    lines.append("")
    lines.append("Databases: ProximaDB (SST/HELIX/VIPER/NOVA/SWIFT/RAPTOR), ChromaDB, LanceDB, FAISS, Qdrant.")
    lines.append("")

    for batch_size, results in all_results.items():
        lines.append(f"## {batch_size:,} vectors")
        lines.append("")
        lines.append("| Database | Insert (ms) | Throughput (/s) | Search (ms) | p95 (ms) |")
        lines.append("| --- | ---: | ---: | ---: | ---: |")
        sorted_results = sorted(
            [(k, v) for k, v in results.items() if "error" not in v],
            key=lambda x: x[1]["docs_per_sec"],
            reverse=True
        )
        for name, r in sorted_results:
            lines.append(
                f"| {name} | {r['insert_time_ms']:.2f} | {r['docs_per_sec']:,.0f} | "
                f"{r['search_time_ms']:.3f} | {r.get('p95_ms', r['search_time_ms']):.3f} |"
            )
        for name, r in results.items():
            if "error" in r:
                lines.append(f"| {name} | error | error | error | error |")
        lines.append("")

    summary_header = "| Database | " + " | ".join(f"{b:,} vec" for b in all_results.keys()) + " |"
    summary_sep = "| --- " + " | ---: " * len(all_results) + "|"
    lines.append("## Throughput Summary")
    lines.append("")
    lines.append(summary_header)
    lines.append(summary_sep)
    db_names = [f"ProximaDB-{eng.upper()}" for eng in proximadb_engines] + ["ChromaDB", "LanceDB", "FAISS", "Qdrant"]
    for db_name in db_names:
        row = [db_name]
        for batch_size in all_results.keys():
            result = all_results[batch_size].get(db_name, {})
            if "error" in result:
                row.append("N/A")
            else:
                row.append(f"{result['docs_per_sec']:,.0f}")
        lines.append("| " + " | ".join(row) + " |")

    report_path = target_dir / "embedded_benchmark_latest.md"
    report_path.write_text("\n".join(lines))
    print(f"\nMarkdown report written to {report_path}")


if __name__ == "__main__":
    run_benchmark([1024, 5000, 10000])
