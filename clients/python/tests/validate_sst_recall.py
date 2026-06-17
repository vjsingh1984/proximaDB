#!/usr/bin/env python3
"""
SST Recall Validation Test

Tests the SST engine recall at 50K vectors to verify the batch-aware IVF training fix.
Expected: Recall should be 95%+ (was 47% before fix)

This test validates the fix implemented in:
- src/index/axis/management/manager.rs (train_ivf_for_batch)
- Calculates optimal cluster count: n_clusters = clamp(sqrt(N) * 2, 16, 256)

Usage:
    PYTHONPATH=clients/python/src python3 clients/python/tests/validate_sst_recall.py
"""

import os
import shutil
import sys
import tempfile
import time

import numpy as np

# Import the native Rust-backed ProximaDB
import proximadb


def generate_clustered_vectors(
    count: int, dimension: int, n_clusters: int = 10, seed: int = 42
):
    """Generate clustered vectors for realistic testing."""
    np.random.seed(seed)

    # Create cluster centers
    centers = np.random.randn(n_clusters, dimension).astype(np.float32)
    centers = centers / np.linalg.norm(centers, axis=1, keepdims=True)

    # Generate vectors around centers
    vectors = []
    for i in range(count):
        center_idx = i % n_clusters
        noise = np.random.randn(dimension).astype(np.float32) * 0.3
        vec = centers[center_idx] + noise
        vec = vec / np.linalg.norm(vec)
        vectors.append(vec)

    return np.array(vectors, dtype=np.float32)


def compute_ground_truth(vectors: np.ndarray, query: np.ndarray, k: int) -> list:
    """Compute exact ground truth using brute force."""
    # Cosine similarity (vectors are normalized)
    similarities = np.dot(vectors, query)
    top_k_indices = np.argsort(similarities)[-k:][::-1]
    return list(top_k_indices)


def compute_recall(predicted: list, ground_truth: list) -> float:
    """Compute recall@k."""
    predicted_set = set(predicted)
    ground_truth_set = set(ground_truth)
    return len(predicted_set & ground_truth_set) / len(ground_truth_set)


def main():
    print("=" * 60)
    print("  SST RECALL VALIDATION TEST - 50K VECTORS")
    print("=" * 60)
    print(f"  ProximaDB v{proximadb.__version__}")

    # Configuration
    VECTOR_COUNT = 50000
    DIMENSION = 128  # Smaller dimension for faster testing
    TOP_K = 10
    NUM_QUERIES = 20
    BATCH_SIZE = 10000

    print("\nConfiguration:")
    print(f"  Vectors: {VECTOR_COUNT}")
    print(f"  Dimension: {DIMENSION}")
    print(f"  Top-K: {TOP_K}")
    print(f"  Queries: {NUM_QUERIES}")

    # Generate vectors
    print("\nGenerating vectors...")
    start = time.time()
    vectors = generate_clustered_vectors(VECTOR_COUNT, DIMENSION)
    print(f"  Generated {VECTOR_COUNT} vectors in {time.time() - start:.2f}s")

    # Generate query vectors
    query_vectors = generate_clustered_vectors(NUM_QUERIES, DIMENSION, seed=123)

    # Compute ground truth
    print("\nComputing ground truth...")
    start = time.time()
    ground_truths = []
    for q in query_vectors:
        gt = compute_ground_truth(vectors, q, TOP_K)
        ground_truths.append(gt)
    print(f"  Computed ground truth in {time.time() - start:.2f}s")

    # Create temp directory
    data_dir = tempfile.mkdtemp(prefix="sst_recall_test_")
    print(f"\nData directory: {data_dir}")

    try:
        os.makedirs(data_dir, exist_ok=True)

        # Use context manager for proper cleanup
        with proximadb.ProximaDB(
            data_dirs=data_dir,
            metadata_dir=os.path.join(data_dir, "metadata"),
            cache_size_mb=256,
            enable_wal=True,
        ) as db:

            collection_name = "test_sst"
            print("\nCreating SST collection...")
            db.create_collection(collection_name, DIMENSION, "sst")
            print("  Collection created")

            # Insert vectors in batches
            print(f"\nInserting {VECTOR_COUNT} vectors...")
            start = time.time()

            ids = [f"vec_{i}" for i in range(VECTOR_COUNT)]

            # Check if insert_numpy is available
            if hasattr(db, "insert_numpy"):
                db.insert_numpy(collection_name, ids, vectors)
            else:
                vectors_list = [v.tolist() for v in vectors]
                db.insert(collection_name, ids, vectors_list, None)

            insert_time = time.time() - start
            print(
                f"  Insert completed: {insert_time:.2f}s ({VECTOR_COUNT/insert_time:.0f} vec/s)"
            )

            # Flush to disk
            print("\nFlushing to disk...")
            start = time.time()
            db.flush()
            flush_time = time.time() - start
            print(f"  Flush completed: {flush_time:.2f}s")

            # Wait for async index building
            print("\nWaiting for async index building (5s)...")
            time.sleep(5)

            # Run search queries
            print(f"\nRunning {NUM_QUERIES} search queries...")
            start = time.time()

            recalls = []
            latencies = []

            for i, query in enumerate(query_vectors):
                query_start = time.time()

                if hasattr(db, "search_numpy"):
                    results = db.search_numpy(collection_name, query, TOP_K)
                else:
                    results = db.search(collection_name, query.tolist(), TOP_K)

                latencies.append((time.time() - query_start) * 1000)

                # Extract result IDs and convert to indices
                result_indices = []
                for r in results:
                    if hasattr(r, "id"):
                        idx = int(r.id.split("_")[1])
                        result_indices.append(idx)
                    elif isinstance(r, dict) and "id" in r:
                        idx = int(r["id"].split("_")[1])
                        result_indices.append(idx)
                    elif isinstance(r, tuple):
                        # (id, score, metadata) format
                        idx = int(r[0].split("_")[1])
                        result_indices.append(idx)

                recall = compute_recall(result_indices, ground_truths[i])
                recalls.append(recall)

            search_time = time.time() - start

            # Calculate metrics
            avg_recall = np.mean(recalls) * 100
            avg_latency = np.mean(latencies)
            p99_latency = np.percentile(latencies, 99)

            print("\n" + "=" * 60)
            print("  RESULTS")
            print("=" * 60)
            print(f"\n  Recall@{TOP_K}: {avg_recall:.1f}%")
            print(f"  Avg Latency: {avg_latency:.2f}ms")
            print(f"  P99 Latency: {p99_latency:.2f}ms")

            # Determine rating
            if avg_recall >= 95:
                rating = "EXCELLENT"
                status = "PASS"
            elif avg_recall >= 90:
                rating = "GOOD"
                status = "PASS"
            elif avg_recall >= 80:
                rating = "ACCEPTABLE"
                status = "MARGINAL"
            else:
                rating = "POOR"
                status = "FAIL"

            print(f"  Rating: {rating}")
            print(f"\n  Test Status: {status}")

            if avg_recall < 90:
                print(f"\n  WARNING: Recall ({avg_recall:.1f}%) is below expected 95%")
                print("  The batch-aware IVF training fix may need verification")
                sys.exit(1)
            else:
                print(f"\n  SUCCESS: Recall is healthy at {avg_recall:.1f}%")

            print("\n" + "=" * 60)

    finally:
        # Cleanup
        shutil.rmtree(data_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
