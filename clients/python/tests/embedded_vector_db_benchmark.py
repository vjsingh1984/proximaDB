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
from typing import List, Dict, Any

import numpy as np

# =============================================================================
# Database Imports
# =============================================================================

# ProximaDB (native Rust via PyO3)
try:
    import proximadb
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
        start = time.perf_counter()
        results = db.search("benchmark", query, 10)
        search_time = time.perf_counter() - start

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": search_time * 1000,
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
        start = time.perf_counter()
        results = collection.query(query_embeddings=query, n_results=10)
        search_time = time.perf_counter() - start

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": search_time * 1000,
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
        start = time.perf_counter()
        results = table.search(query).limit(10).to_list()
        search_time = time.perf_counter() - start

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": search_time * 1000,
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
        start = time.perf_counter()
        distances, indices = faiss_index.search(query, 10)
        search_time = time.perf_counter() - start

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": search_time * 1000,
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
        start = time.perf_counter()
        results = client.query_points(
            collection_name="benchmark",
            query=query,
            limit=10,
        )
        search_time = time.perf_counter() - start

        return {
            "insert_time_ms": insert_time * 1000,
            "search_time_ms": search_time * 1000,
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

    all_results = {}

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

        # Summary table
        print(f"\n  {'─'*78}")
        print(f"  Results: {batch_size:,} vectors")
        print(f"  {'─'*78}")
        print(f"  {'Database':<18} {'Insert (ms)':<15} {'Throughput':<18} {'Search (ms)':<12}")
        print(f"  {'─'*78}")

        sorted_results = sorted(
            [(k, v) for k, v in results.items() if "error" not in v],
            key=lambda x: x[1]['docs_per_sec'],
            reverse=True
        )

        for db_name, result in sorted_results:
            print(f"  {db_name:<18} {result['insert_time_ms']:>10.2f}ms   {result['docs_per_sec']:>14,.0f}/s   {result['search_time_ms']:>8.3f}ms")

        for db_name, result in results.items():
            if "error" in result:
                print(f"  {db_name:<18} Error: {result['error'][:40]}")

    # Final summary
    print("\n" + "=" * 80)
    print("SUMMARY: Insert Throughput (vectors/second)")
    print("=" * 80)
    print()

    print(f"{'Database':<18}", end="")
    for batch_size in batch_sizes:
        print(f" {batch_size:>12,}", end="")
    print()
    print("-" * (18 + len(batch_sizes) * 14))

    db_names = [f"ProximaDB-{eng.upper()}" for eng in proximadb_engines] + ["ChromaDB", "LanceDB", "FAISS", "Qdrant"]

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

    print()
    print("=" * 80)
    print("OBSERVATIONS")
    print("=" * 80)
    print("""
Performance characteristics by database:

  ProximaDB Storage Engines:
  - SST: Write-optimized with LSM-tree structure, good for real-time ingestion
  - HELIX: Hilbert curve locality optimization, efficient for range queries
  - VIPER: Columnar Parquet format, optimized for analytical workloads
  - NOVA: Progressive columnar storage, balanced for mixed workloads
  - SWIFT: Optimized for ultra-low latency with smaller datasets
  - RAPTOR: Adaptive row-group sizing for dynamic access patterns

  FAISS
  - Highest throughput (in-memory optimized C++)
  - No built-in persistence or metadata support
  - Best for: Temporary indexes, maximum speed requirements

  LanceDB
  - Strong batch performance with disk persistence
  - Columnar Arrow format enables efficient I/O
  - Best for: Disk-based storage with good throughput

  Qdrant
  - Rich metadata filtering capabilities
  - Higher insert latency due to indexing
  - Best for: Complex queries with payload filtering

  ChromaDB
  - Simple Python-native API
  - Batch size limitations at scale
  - Best for: Prototyping and smaller datasets
""")


if __name__ == "__main__":
    run_benchmark([1024, 5000, 10000])
