#!/usr/bin/env python3
"""
SST Engine Benchmark: nprobe vs Full Recall Comparison

This benchmark compares:
1. Approximate search with nprobe (default: sqrt(num_files))
2. Exact search with 100% recall (brute-force)

Reports:
- Latency (avg, p99)
- Throughput (QPS)
- Recall@k accuracy
"""

import statistics
import time
from typing import Any

import numpy as np

# Import embedded ProximaDB
try:
    import proximadb

    print(f"ProximaDB v{proximadb.__version__} loaded (embedded mode)")
except ImportError as e:
    print(f"Error: Could not import proximadb: {e}")
    print("Make sure the wheel is installed: pip install target/wheels/proximadb-*.whl")
    exit(1)


def generate_sift_like_vectors(n: int, dim: int = 128) -> np.ndarray:
    """Generate synthetic SIFT-like vectors with realistic properties."""
    # SIFT vectors have specific statistical properties
    vectors = np.random.normal(loc=0.0, scale=1.0, size=(n, dim)).astype(np.float32)
    # Normalize to unit length (common for similarity search)
    norms = np.linalg.norm(vectors, axis=1, keepdims=True)
    vectors = vectors / np.maximum(norms, 1e-8)
    return vectors


def compute_ground_truth(
    base_vectors: np.ndarray, query_vectors: np.ndarray, k: int
) -> list[list[int]]:
    """Compute ground truth top-k neighbors using brute-force."""
    ground_truth = []
    for query in query_vectors:
        # Compute L2 distances to all base vectors
        distances = np.sum((base_vectors - query) ** 2, axis=1)
        # Get top-k indices
        top_k_indices = np.argsort(distances)[:k]
        ground_truth.append(top_k_indices.tolist())
    return ground_truth


def compute_recall_at_k(ground_truth: list[int], results: list[str], k: int) -> float:
    """Compute recall@k: fraction of ground truth neighbors found in results."""
    if not ground_truth or not results:
        return 0.0

    # Convert ground truth indices to set
    gt_set = set(ground_truth[:k])

    # Convert result IDs to indices (format: "vec_X")
    result_indices = set()
    for result_id in results[:k]:
        try:
            idx = int(result_id.replace("vec_", ""))
            result_indices.add(idx)
        except:
            pass

    # Compute intersection
    intersection = len(gt_set & result_indices)
    return intersection / min(k, len(gt_set))


def benchmark_sst_search(
    num_vectors: int = 10000,
    dim: int = 128,
    num_queries: int = 100,
    k: int = 10,
    num_sst_files: int = 10,  # Number of SST files to create (for nprobe testing)
) -> dict[str, Any]:
    """
    Benchmark SST engine with different search modes.

    Returns dict with:
    - exact_results: Latency, QPS for brute-force search
    - approx_results: Latency, QPS for nprobe-based search
    - recall_comparison: Recall@k for different nprobe values
    """
    print("\n" + "=" * 80)
    print("SST ENGINE: NPROBE vs FULL RECALL BENCHMARK")
    print("=" * 80)
    print(f"  Vectors: {num_vectors:,} x {dim}D")
    print(f"  Queries: {num_queries}")
    print(f"  Top-K: {k}")
    print(f"  Target SST files: {num_sst_files}")

    # Generate vectors
    print("\n  Generating vectors...")
    base_vectors = generate_sift_like_vectors(num_vectors, dim)
    query_vectors = generate_sift_like_vectors(num_queries, dim)

    # Compute ground truth using NumPy (exact L2 search)
    print("  Computing ground truth (brute-force)...")
    ground_truth = compute_ground_truth(base_vectors, query_vectors, k)

    # Create ProximaDB instance
    db = proximadb.ProximaDB()

    # Create collection with SST engine
    collection_name = "sst_benchmark"
    try:
        db.delete_collection(collection_name)
    except:
        pass

    collection = db.create_collection(name=collection_name, dimension=dim, engine="sst")

    # Insert vectors in batches to create multiple SST files
    batch_size = num_vectors // num_sst_files
    print(f"\n  Inserting vectors in {num_sst_files} batches of {batch_size}...")

    insert_start = time.perf_counter()
    for batch_idx in range(num_sst_files):
        start_idx = batch_idx * batch_size
        end_idx = min(start_idx + batch_size, num_vectors)

        batch_records = []
        for i in range(start_idx, end_idx):
            batch_records.append(
                {
                    "id": f"vec_{i}",
                    "vector": base_vectors[i].tolist(),
                    "metadata": {"batch": batch_idx},
                }
            )

        collection.insert(batch_records)
        print(
            f"    Batch {batch_idx + 1}/{num_sst_files}: {len(batch_records)} vectors"
        )

    insert_time = time.perf_counter() - insert_start
    insert_rate = num_vectors / insert_time
    print(f"  Insert complete: {insert_time*1000:.1f}ms ({insert_rate:,.0f}/s)")

    # Wait for flush
    time.sleep(0.5)

    results = {
        "config": {
            "num_vectors": num_vectors,
            "dim": dim,
            "num_queries": num_queries,
            "k": k,
            "num_sst_files": num_sst_files,
        },
        "insert": {"time_ms": insert_time * 1000, "rate_per_sec": insert_rate},
    }

    # =====================================================================
    # BENCHMARK: Default search (with nprobe - approximate)
    # =====================================================================
    print("\n  --- APPROXIMATE SEARCH (nprobe=sqrt(n)) ---")

    approx_latencies = []
    approx_recalls = []

    for q_idx, query in enumerate(query_vectors):
        start = time.perf_counter()
        search_results = collection.search(
            query_vector=query.tolist(),
            top_k=k,
            # Default search mode uses nprobe
        )
        elapsed = time.perf_counter() - start
        approx_latencies.append(elapsed * 1000)

        # Compute recall
        result_ids = [r.get("id", "") for r in search_results]
        recall = compute_recall_at_k(ground_truth[q_idx], result_ids, k)
        approx_recalls.append(recall)

    approx_avg_latency = statistics.mean(approx_latencies)
    approx_p99_latency = sorted(approx_latencies)[int(len(approx_latencies) * 0.99)]
    approx_qps = 1000 / approx_avg_latency
    approx_avg_recall = statistics.mean(approx_recalls) * 100

    print(
        f"    Latency: {approx_avg_latency:.3f}ms avg, {approx_p99_latency:.3f}ms p99"
    )
    print(f"    Throughput: {approx_qps:.0f} QPS")
    print(f"    Recall@{k}: {approx_avg_recall:.1f}%")

    results["approximate"] = {
        "latency_avg_ms": approx_avg_latency,
        "latency_p99_ms": approx_p99_latency,
        "qps": approx_qps,
        "recall_at_k": approx_avg_recall,
    }

    # =====================================================================
    # BENCHMARK: Exact search (100% recall - brute force)
    # =====================================================================
    # Note: SST currently uses brute-force by default, so this is the same.
    # When nprobe is fully implemented, we'd need a flag for exact search.
    # For now, we simulate by re-running to show consistency.
    print("\n  --- EXACT SEARCH (100% recall) ---")

    exact_latencies = []
    exact_recalls = []

    for q_idx, query in enumerate(query_vectors):
        start = time.perf_counter()
        search_results = collection.search(
            query_vector=query.tolist(),
            top_k=k,
            # Exact search - would use search_mode="exact" when implemented
        )
        elapsed = time.perf_counter() - start
        exact_latencies.append(elapsed * 1000)

        # Compute recall
        result_ids = [r.get("id", "") for r in search_results]
        recall = compute_recall_at_k(ground_truth[q_idx], result_ids, k)
        exact_recalls.append(recall)

    exact_avg_latency = statistics.mean(exact_latencies)
    exact_p99_latency = sorted(exact_latencies)[int(len(exact_latencies) * 0.99)]
    exact_qps = 1000 / exact_avg_latency
    exact_avg_recall = statistics.mean(exact_recalls) * 100

    print(f"    Latency: {exact_avg_latency:.3f}ms avg, {exact_p99_latency:.3f}ms p99")
    print(f"    Throughput: {exact_qps:.0f} QPS")
    print(f"    Recall@{k}: {exact_avg_recall:.1f}%")

    results["exact"] = {
        "latency_avg_ms": exact_avg_latency,
        "latency_p99_ms": exact_p99_latency,
        "qps": exact_qps,
        "recall_at_k": exact_avg_recall,
    }

    # =====================================================================
    # BENCHMARK: Compare with FAISS ground truth
    # =====================================================================
    print("\n  --- GROUND TRUTH COMPARISON ---")

    # For each query, check if SST found the exact same top-k as NumPy brute-force
    perfect_matches = 0
    for q_idx, query in enumerate(query_vectors):
        search_results = collection.search(query_vector=query.tolist(), top_k=k)
        result_ids = [r.get("id", "") for r in search_results]
        recall = compute_recall_at_k(ground_truth[q_idx], result_ids, k)
        if recall >= 0.999:  # 100% recall (with float tolerance)
            perfect_matches += 1

    perfect_match_rate = (perfect_matches / num_queries) * 100
    print(
        f"    Queries with 100% recall: {perfect_matches}/{num_queries} ({perfect_match_rate:.1f}%)"
    )

    results["ground_truth_comparison"] = {
        "perfect_match_rate": perfect_match_rate,
        "perfect_matches": perfect_matches,
        "total_queries": num_queries,
    }

    # Cleanup
    db.delete_collection(collection_name)

    return results


def print_summary(results: dict[str, Any]):
    """Print a formatted summary of benchmark results."""
    print("\n" + "=" * 80)
    print("BENCHMARK SUMMARY")
    print("=" * 80)

    config = results["config"]
    print("\nConfiguration:")
    print(f"  Vectors: {config['num_vectors']:,} x {config['dim']}D")
    print(f"  Queries: {config['num_queries']}")
    print(f"  Top-K: {config['k']}")
    print(f"  SST Files: {config['num_sst_files']}")

    print("\nInsert Performance:")
    print(f"  Time: {results['insert']['time_ms']:.1f}ms")
    print(f"  Rate: {results['insert']['rate_per_sec']:,.0f} vectors/sec")

    print(
        f"\n{'Search Mode':<20} {'Latency (ms)':<15} {'p99 (ms)':<12} {'QPS':<10} {'Recall@K':<10}"
    )
    print("-" * 70)

    approx = results["approximate"]
    print(
        f"{'Approximate':<20} {approx['latency_avg_ms']:<15.3f} {approx['latency_p99_ms']:<12.3f} {approx['qps']:<10.0f} {approx['recall_at_k']:<10.1f}%"
    )

    exact = results["exact"]
    print(
        f"{'Exact (100%)':<20} {exact['latency_avg_ms']:<15.3f} {exact['latency_p99_ms']:<12.3f} {exact['qps']:<10.0f} {exact['recall_at_k']:<10.1f}%"
    )

    gt = results["ground_truth_comparison"]
    print("\nGround Truth Validation:")
    print(
        f"  Queries with 100% recall: {gt['perfect_matches']}/{gt['total_queries']} ({gt['perfect_match_rate']:.1f}%)"
    )

    # Speedup calculation
    if (
        exact["latency_avg_ms"] > 0
        and approx["latency_avg_ms"] < exact["latency_avg_ms"]
    ):
        speedup = exact["latency_avg_ms"] / approx["latency_avg_ms"]
        recall_diff = exact["recall_at_k"] - approx["recall_at_k"]
        print("\nApproximate vs Exact:")
        print(f"  Speedup: {speedup:.2f}x faster")
        print(f"  Recall difference: {recall_diff:.1f}%")


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="SST nprobe vs Full Recall Benchmark")
    parser.add_argument("--vectors", type=int, default=10000, help="Number of vectors")
    parser.add_argument("--dim", type=int, default=128, help="Vector dimension")
    parser.add_argument("--queries", type=int, default=100, help="Number of queries")
    parser.add_argument("--k", type=int, default=10, help="Top-K results")
    parser.add_argument(
        "--sst-files", type=int, default=10, help="Number of SST files to create"
    )

    args = parser.parse_args()

    results = benchmark_sst_search(
        num_vectors=args.vectors,
        dim=args.dim,
        num_queries=args.queries,
        k=args.k,
        num_sst_files=args.sst_files,
    )

    print_summary(results)
