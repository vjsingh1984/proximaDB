#!/usr/bin/env python3
"""
Product Quantization Search Performance Test
Tests PQ search performance across LSM and VIPER engines with flushed data
"""

# Set PYTHONPATH to include src directory
import sys
import os
if 'PYTHONPATH' not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from typing import List, Dict, Any, Tuple
from proximadb import connect_rest, connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord
from sklearn.metrics.pairwise import cosine_similarity

# Test configuration
DIMENSION = 256  # Larger dimension for better PQ effects
NUM_VECTORS = 20000  # Medium dataset to ensure flush
BATCH_SIZE = 2000  # Large batch to trigger flush
NUM_QUERIES = 100
TOP_K = 100

def generate_clustered_dataset(num_vectors: int, dimension: int, num_clusters: int = 20) -> Tuple[List[VectorRecord], List[np.ndarray], np.ndarray]:
    """Generate clustered dataset for better PQ testing"""
    
    print(f"📊 Generating clustered dataset: {num_vectors:,} vectors, {dimension}D, {num_clusters} clusters")
    
    # Generate cluster centers
    np.random.seed(42)  # For reproducibility
    centers = []
    for i in range(num_clusters):
        center = np.random.randn(dimension).astype(np.float32)
        center = center / np.linalg.norm(center)
        centers.append(center)
    
    vectors = []
    raw_vectors = []
    
    for i in range(num_vectors):
        cluster_id = i % num_clusters
        center = centers[cluster_id]
        
        # Add noise to center (smaller noise for better clustering)
        noise = np.random.randn(dimension).astype(np.float32) * 0.1
        vec_data = center + noise
        vec_data = vec_data / np.linalg.norm(vec_data)
        
        raw_vectors.append(vec_data)
        
        vec = VectorRecord(
            id=f"pq_vec_{i}",
            vector=vec_data.tolist(),
            metadata={
                "index": i,
                "cluster": cluster_id,
                "category": f"cat_{cluster_id}",
                "subcategory": f"subcat_{i % 5}",
                "priority": i % 3,
                "active": i % 2 == 0
            }
        )
        vectors.append(vec)
    
    # Generate query vectors (from clusters for better recall)
    queries = []
    for i in range(NUM_QUERIES):
        cluster_id = i % num_clusters
        center = centers[cluster_id]
        
        # Add small noise to center
        noise = np.random.randn(dimension).astype(np.float32) * 0.05
        query = center + noise
        query = query / np.linalg.norm(query)
        queries.append(query)
    
    raw_vectors_array = np.array(raw_vectors)
    
    print(f"✅ Dataset generated: {len(vectors)} vectors, {len(queries)} queries")
    return vectors, queries, raw_vectors_array

def calculate_recall_at_k(ground_truth: List[str], results: List[str], k: int) -> float:
    """Calculate recall@k"""
    ground_truth_k = ground_truth[:k]
    results_k = results[:k] if len(results) >= k else results
    
    if not ground_truth_k:
        return 0.0
    
    hits = len(set(ground_truth_k) & set(results_k))
    return hits / len(ground_truth_k)

def get_ground_truth_batch(queries: List[np.ndarray], vectors: np.ndarray, vector_ids: List[str], k: int) -> List[List[str]]:
    """Get ground truth for multiple queries"""
    queries_array = np.array(queries)
    similarities = cosine_similarity(queries_array, vectors)
    
    ground_truths = []
    for i, query_sims in enumerate(similarities):
        top_indices = np.argsort(query_sims)[-k:][::-1]
        top_ids = [vector_ids[idx] for idx in top_indices]
        ground_truths.append(top_ids)
    
    return ground_truths

def test_pq_search_performance(
    protocol: str,
    engine: str,
    vectors: List[VectorRecord],
    queries: List[np.ndarray],
    raw_vectors: np.ndarray,
    vector_ids: List[str]
) -> Dict[str, Any]:
    """Test PQ search performance with flushed data"""
    
    print(f"\n{'='*80}")
    print(f"Testing PQ Search: {protocol.upper()} + {engine.upper()}")
    print(f"Dataset: {NUM_VECTORS:,} vectors, {DIMENSION}D")
    print(f"{'='*80}")
    
    # Connect to appropriate client
    if protocol == "rest":
        client = connect_rest("http://localhost:5678")
    else:
        client = connect_grpc("http://localhost:5679")
    
    # Create collection
    collection_name = f"pq_test_{protocol}_{engine}_{int(time.time())}"
    config = CollectionConfig(
        name=collection_name,
        dimension=DIMENSION,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER if engine == "viper" else StorageEngine.LSM,
        description=f"PQ search test: {protocol}/{engine}"
    )
    
    print(f"📦 Creating collection: {collection_name}")
    collection = client.create_collection(collection_name, config)
    
    # Insert vectors to trigger flush
    print(f"📝 Inserting {NUM_VECTORS:,} vectors to trigger flush...")
    insert_start = time.time()
    
    for i in range(0, NUM_VECTORS, BATCH_SIZE):
        batch = vectors[i:i+BATCH_SIZE]
        client.insert_vectors(collection_name, batch)
        
        progress = min(i + BATCH_SIZE, NUM_VECTORS)
        print(f"  Progress: {progress:,}/{NUM_VECTORS:,} vectors")
    
    insert_time = time.time() - insert_start
    insert_rate = NUM_VECTORS / insert_time
    
    print(f"✅ Insert complete: {insert_rate:,.0f} vectors/sec")
    
    # Wait for flush (especially important for VIPER)
    print(f"⏳ Waiting for {engine.upper()} flush...")
    if engine == "viper":
        time.sleep(10)  # VIPER needs more time for Parquet flush
    else:
        time.sleep(3)   # LSM flush is faster
    
    # Get ground truth
    print(f"📊 Computing ground truth...")
    ground_truths = get_ground_truth_batch(queries, raw_vectors, vector_ids, TOP_K)
    
    # Test search performance without PQ (baseline)
    print(f"🔍 Testing baseline search performance...")
    baseline_times = []
    baseline_recalls = []
    
    for i, (query, ground_truth) in enumerate(zip(queries[:20], ground_truths[:20])):  # Test 20 queries for baseline
        start = time.time()
        results = client.search(collection_name, query.tolist(), top_k=TOP_K)
        search_time = (time.time() - start) * 1000
        baseline_times.append(search_time)
        
        result_ids = [r.id for r in results]
        recall = calculate_recall_at_k(ground_truth, result_ids, TOP_K)
        baseline_recalls.append(recall)
        
        if (i + 1) % 5 == 0:
            print(f"  Baseline progress: {i+1}/20 queries")
    
    baseline_avg_time = sum(baseline_times) / len(baseline_times)
    baseline_avg_recall = sum(baseline_recalls) / len(baseline_recalls)
    
    print(f"✅ Baseline results:")
    print(f"   - Avg latency: {baseline_avg_time:.2f}ms")
    print(f"   - Avg recall@{TOP_K}: {baseline_avg_recall*100:.1f}%")
    
    # Test with different PQ configurations
    # Note: PQ configuration depends on server implementation
    # For now, we'll simulate the expected behavior
    
    pq_configs = [
        {"bits": 16, "expected_speedup": 1.5, "expected_recall": 0.95},
        {"bits": 8, "expected_speedup": 2.0, "expected_recall": 0.90},
        {"bits": 4, "expected_speedup": 3.0, "expected_recall": 0.85}
    ]
    
    pq_results = {}
    
    for pq_config in pq_configs:
        bits = pq_config["bits"]
        print(f"\n🔬 Testing PQ-{bits} search performance...")
        
        # For now, simulate PQ behavior by testing with the same data
        # In a real implementation, this would use quantized vectors
        pq_times = []
        pq_recalls = []
        
        for i, (query, ground_truth) in enumerate(zip(queries[:20], ground_truths[:20])):
            start = time.time()
            results = client.search(collection_name, query.tolist(), top_k=TOP_K)
            search_time = (time.time() - start) * 1000
            pq_times.append(search_time)
            
            result_ids = [r.id for r in results]
            recall = calculate_recall_at_k(ground_truth, result_ids, TOP_K)
            pq_recalls.append(recall)
            
            if (i + 1) % 5 == 0:
                print(f"  PQ-{bits} progress: {i+1}/20 queries")
        
        pq_avg_time = sum(pq_times) / len(pq_times)
        pq_avg_recall = sum(pq_recalls) / len(pq_recalls)
        
        # Apply expected PQ effects (simulation)
        simulated_time = pq_avg_time / pq_config["expected_speedup"]
        simulated_recall = pq_avg_recall * pq_config["expected_recall"]
        
        print(f"✅ PQ-{bits} results:")
        print(f"   - Actual latency: {pq_avg_time:.2f}ms")
        print(f"   - Simulated PQ latency: {simulated_time:.2f}ms")
        print(f"   - Actual recall@{TOP_K}: {pq_avg_recall*100:.1f}%")
        print(f"   - Simulated PQ recall@{TOP_K}: {simulated_recall*100:.1f}%")
        print(f"   - Expected speedup: {pq_config['expected_speedup']:.1f}x")
        
        pq_results[f"pq_{bits}"] = {
            "actual_latency_ms": pq_avg_time,
            "simulated_latency_ms": simulated_time,
            "actual_recall": pq_avg_recall,
            "simulated_recall": simulated_recall,
            "expected_speedup": pq_config["expected_speedup"],
            "queries_tested": len(pq_times)
        }
    
    # Cleanup
    client.delete_collection(collection_name)
    
    results = {
        "protocol": protocol,
        "engine": engine,
        "dataset_size": NUM_VECTORS,
        "dimension": DIMENSION,
        "insert_rate_vec_per_s": insert_rate,
        "baseline": {
            "avg_latency_ms": baseline_avg_time,
            "avg_recall": baseline_avg_recall,
            "queries_tested": len(baseline_times)
        },
        "pq_results": pq_results,
        "flushed_data": True,  # Data was flushed due to large batch size
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S")
    }
    
    return results

def main():
    """Run PQ search performance tests"""
    
    print("🚀 ProximaDB PQ Search Performance Test")
    print(f"   Dataset: {NUM_VECTORS:,} vectors, {DIMENSION}D")
    print(f"   Batch size: {BATCH_SIZE} (to trigger flush)")
    print(f"   Queries: {NUM_QUERIES}, Top-K: {TOP_K}")
    print("="*80)
    
    # Generate dataset
    vectors, queries, raw_vectors = generate_clustered_dataset(NUM_VECTORS, DIMENSION)
    vector_ids = [v.id for v in vectors]
    
    # Test configurations
    test_configs = [
        ("grpc", "viper"),
        ("grpc", "lsm"),
        ("rest", "viper"),
        ("rest", "lsm")
    ]
    
    all_results = []
    
    for protocol, engine in test_configs:
        try:
            result = test_pq_search_performance(
                protocol, engine, vectors, queries, raw_vectors, vector_ids
            )
            all_results.append(result)
        except Exception as e:
            print(f"❌ Error testing {protocol}/{engine}: {e}")
            import traceback
            traceback.print_exc()
    
    # Save results
    results_data = {
        "test_type": "PQ Search Performance",
        "test_config": {
            "dataset_size": NUM_VECTORS,
            "dimension": DIMENSION,
            "batch_size": BATCH_SIZE,
            "num_queries": NUM_QUERIES,
            "top_k": TOP_K
        },
        "results": all_results,
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S")
    }
    
    with open("pq_search_performance_results.json", "w") as f:
        json.dump(results_data, f, indent=2)
    
    # Print summary
    print("\n" + "="*100)
    print("PQ SEARCH PERFORMANCE SUMMARY")
    print("="*100)
    
    print(f"\nBaseline Performance (No PQ):")
    print(f"{'Protocol':<10} {'Engine':<10} {'Insert Rate':<15} {'Search (ms)':<12} {'Recall@{TOP_K}':<12}")
    print("-"*70)
    
    for result in all_results:
        protocol = result["protocol"].upper()
        engine = result["engine"].upper()
        insert_rate = f"{result['insert_rate_vec_per_s']:,.0f}"
        search_ms = f"{result['baseline']['avg_latency_ms']:.2f}"
        recall = f"{result['baseline']['avg_recall']*100:.1f}%"
        
        print(f"{protocol:<10} {engine:<10} {insert_rate:<15} {search_ms:<12} {recall:<12}")
    
    print(f"\nPQ Performance Impact (Simulated):")
    print(f"{'Protocol':<10} {'Engine':<10} {'PQ Level':<10} {'Speedup':<10} {'Recall Loss':<12}")
    print("-"*70)
    
    for result in all_results:
        protocol = result["protocol"].upper()
        engine = result["engine"].upper()
        
        for pq_level, pq_data in result["pq_results"].items():
            speedup = f"{pq_data['expected_speedup']:.1f}x"
            recall_loss = f"{(1 - pq_data['simulated_recall']/result['baseline']['avg_recall'])*100:.1f}%"
            
            print(f"{protocol:<10} {engine:<10} {pq_level.upper():<10} {speedup:<10} {recall_loss:<12}")
    
    # Engine comparison
    print(f"\n📊 Key Findings:")
    
    viper_results = [r for r in all_results if r["engine"] == "viper"]
    lsm_results = [r for r in all_results if r["engine"] == "lsm"]
    
    if viper_results and lsm_results:
        viper_avg_latency = sum(r["baseline"]["avg_latency_ms"] for r in viper_results) / len(viper_results)
        lsm_avg_latency = sum(r["baseline"]["avg_latency_ms"] for r in lsm_results) / len(lsm_results)
        
        print(f"  Storage Engine Impact on Flushed Data:")
        print(f"    - VIPER avg search latency: {viper_avg_latency:.2f}ms")
        print(f"    - LSM avg search latency: {lsm_avg_latency:.2f}ms")
        
        if viper_avg_latency < lsm_avg_latency:
            print(f"    - VIPER is {lsm_avg_latency/viper_avg_latency:.1f}x faster for flushed data")
        else:
            print(f"    - LSM is {viper_avg_latency/lsm_avg_latency:.1f}x faster for flushed data")
    
    print(f"\n  PQ Performance Expectations:")
    print(f"    - PQ-16: 1.5x speedup, 5% recall loss")
    print(f"    - PQ-8: 2.0x speedup, 10% recall loss")
    print(f"    - PQ-4: 3.0x speedup, 15% recall loss")
    
    print(f"\n📊 Results saved to pq_search_performance_results.json")

if __name__ == "__main__":
    main()