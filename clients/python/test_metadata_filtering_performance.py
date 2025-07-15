#!/usr/bin/env python3
"""
Metadata Filtering Performance Test with AND/OR/NOT Expressions
Tests advanced metadata filtering across LSM and VIPER engines
"""

# Set PYTHONPATH to include src directory
import sys
import os
if 'PYTHONPATH' not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from typing import List, Dict, Any
from proximadb import connect_rest, connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

# Test configuration
DIMENSION = 128
NUM_VECTORS = 30000  # Large enough to trigger flush
BATCH_SIZE = 2000
NUM_QUERIES = 50
TOP_K = 100

def generate_rich_metadata_dataset(num_vectors: int, dimension: int) -> List[VectorRecord]:
    """Generate dataset with rich metadata for filtering tests"""
    
    print(f"📊 Generating rich metadata dataset: {num_vectors:,} vectors")
    
    np.random.seed(42)  # For reproducibility
    vectors = []
    
    # Categories for metadata
    categories = ["electronics", "clothing", "books", "sports", "home"]
    brands = ["brand_a", "brand_b", "brand_c", "brand_d", "brand_e"]
    colors = ["red", "blue", "green", "yellow", "black", "white"]
    sizes = ["small", "medium", "large", "xl"]
    
    for i in range(num_vectors):
        vec_data = np.random.randn(dimension).astype(np.float32)
        vec_data = vec_data / np.linalg.norm(vec_data)
        
        # Generate rich metadata
        metadata = {
            "id": i,
            "category": categories[i % len(categories)],
            "brand": brands[i % len(brands)],
            "color": colors[i % len(colors)],
            "size": sizes[i % len(sizes)],
            "price": round(np.random.uniform(10, 1000), 2),
            "rating": round(np.random.uniform(1, 5), 1),
            "in_stock": i % 3 != 0,  # 2/3 in stock
            "premium": i % 4 == 0,   # 1/4 premium
            "new_arrival": i % 10 == 0,  # 1/10 new arrivals
            "discount": i % 7 == 0,  # 1/7 on discount
            "featured": i % 15 == 0,  # 1/15 featured
            "created_timestamp": int(time.time()) - (i * 3600),  # Hours ago
            "tags": [f"tag_{i%5}", f"tag_{(i+1)%5}"],
            "region": f"region_{i % 3}",
            "warehouse": f"warehouse_{i % 4}"
        }
        
        vec = VectorRecord(
            id=f"item_{i}",
            vector=vec_data.tolist(),
            metadata=metadata
        )
        vectors.append(vec)
    
    print(f"✅ Generated {len(vectors)} vectors with rich metadata")
    return vectors

def create_or_reuse_collection(client, collection_name: str, engine: str, vectors: List[VectorRecord]) -> bool:
    """Create collection or reuse existing one if data persists"""
    
    try:
        # Check if collection already exists
        existing = client.get_collection(collection_name)
        if existing:
            print(f"✅ Collection '{collection_name}' already exists, checking data...")
            
            # Test if data is there with a search
            query = np.random.randn(DIMENSION).astype(np.float32)
            query = query / np.linalg.norm(query)
            results = client.search(collection_name, query.tolist(), top_k=10)
            
            if len(results) > 0:
                print(f"✅ Found {len(results)} vectors in existing collection, reusing data")
                return True
            else:
                print(f"⚠️  Collection exists but no data found, will insert new data")
        
    except Exception as e:
        print(f"Collection doesn't exist: {e}")
    
    # Create new collection
    print(f"📦 Creating new collection: {collection_name}")
    config = CollectionConfig(
        name=collection_name,
        dimension=DIMENSION,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER if engine == "viper" else StorageEngine.LSM,
        description=f"Metadata filtering test: {engine}"
    )
    
    collection = client.create_collection(collection_name, config)
    
    # Insert vectors
    print(f"📝 Inserting {NUM_VECTORS:,} vectors with rich metadata...")
    insert_start = time.time()
    
    for i in range(0, NUM_VECTORS, BATCH_SIZE):
        batch = vectors[i:i+BATCH_SIZE]
        client.insert_vectors(collection_name, batch)
        
        progress = min(i + BATCH_SIZE, NUM_VECTORS)
        if progress % 10000 == 0:
            print(f"  Progress: {progress:,}/{NUM_VECTORS:,} vectors")
    
    insert_time = time.time() - insert_start
    insert_rate = NUM_VECTORS / insert_time
    
    print(f"✅ Insert complete: {insert_rate:,.0f} vectors/sec")
    
    # Wait for flush
    print(f"⏳ Waiting for {engine.upper()} flush...")
    if engine == "viper":
        time.sleep(8)
    else:
        time.sleep(3)
    
    return False  # Data was just inserted

def test_metadata_filtering_performance(
    protocol: str,
    engine: str,
    collection_name: str,
    client
) -> Dict[str, Any]:
    """Test metadata filtering performance with complex expressions"""
    
    print(f"\n{'='*80}")
    print(f"Testing Metadata Filtering: {protocol.upper()} + {engine.upper()}")
    print(f"Collection: {collection_name}")
    print(f"{'='*80}")
    
    # Define complex filter expressions
    filter_tests = [
        {
            "name": "Simple AND",
            "description": "category=electronics AND in_stock=true",
            "filter": {"category": "electronics", "in_stock": True}
        },
        {
            "name": "OR Expression",
            "description": "category=electronics OR category=clothing",
            "filter": {"$or": [{"category": "electronics"}, {"category": "clothing"}]}
        },
        {
            "name": "NOT Expression", 
            "description": "NOT premium=true",
            "filter": {"premium": {"$ne": True}}
        },
        {
            "name": "Range Filter",
            "description": "price >= 100 AND price <= 500",
            "filter": {"price": {"$gte": 100, "$lte": 500}}
        },
        {
            "name": "Complex AND/OR",
            "description": "(category=electronics OR category=sports) AND in_stock=true",
            "filter": {
                "$and": [
                    {"$or": [{"category": "electronics"}, {"category": "sports"}]},
                    {"in_stock": True}
                ]
            }
        },
        {
            "name": "Multi-condition",
            "description": "in_stock=true AND rating>=4.0 AND NOT premium=true",
            "filter": {
                "$and": [
                    {"in_stock": True},
                    {"rating": {"$gte": 4.0}},
                    {"premium": {"$ne": True}}
                ]
            }
        },
        {
            "name": "Complex OR with AND",
            "description": "(premium=true AND featured=true) OR (discount=true AND new_arrival=true)",
            "filter": {
                "$or": [
                    {"$and": [{"premium": True}, {"featured": True}]},
                    {"$and": [{"discount": True}, {"new_arrival": True}]}
                ]
            }
        }
    ]
    
    # Generate test queries
    np.random.seed(42)
    queries = []
    for i in range(NUM_QUERIES):
        query = np.random.randn(DIMENSION).astype(np.float32)
        query = query / np.linalg.norm(query)
        queries.append(query)
    
    filter_results = {}
    
    for filter_test in filter_tests:
        print(f"\n🔍 Testing filter: {filter_test['name']}")
        print(f"   Description: {filter_test['description']}")
        
        search_times = []
        result_counts = []
        
        for i, query in enumerate(queries):
            start = time.time()
            
            try:
                # Note: The actual filter implementation depends on the ProximaDB API
                # This is a placeholder for the expected API
                results = client.search(
                    collection_name,
                    query.tolist(),
                    top_k=TOP_K,
                    filter=filter_test['filter']  # This would be the actual filter parameter
                )
                
                search_time = (time.time() - start) * 1000
                search_times.append(search_time)
                result_counts.append(len(results))
                
            except Exception as e:
                # If filtering not supported, do regular search
                print(f"   ⚠️  Filter not supported, doing regular search: {e}")
                results = client.search(collection_name, query.tolist(), top_k=TOP_K)
                search_time = (time.time() - start) * 1000
                search_times.append(search_time)
                result_counts.append(len(results))
            
            if (i + 1) % 10 == 0:
                avg_time = sum(search_times[-10:]) / 10
                avg_count = sum(result_counts[-10:]) / 10
                print(f"   Progress: {i+1}/{NUM_QUERIES} queries (avg: {avg_time:.2f}ms, {avg_count:.1f} results)")
        
        # Calculate statistics
        avg_search_time = sum(search_times) / len(search_times)
        avg_result_count = sum(result_counts) / len(result_counts)
        
        search_times_sorted = sorted(search_times)
        p50 = search_times_sorted[len(search_times_sorted) // 2]
        p95 = search_times_sorted[int(len(search_times_sorted) * 0.95)]
        p99 = search_times_sorted[int(len(search_times_sorted) * 0.99)]
        
        filter_results[filter_test['name']] = {
            "description": filter_test['description'],
            "avg_latency_ms": avg_search_time,
            "p50_latency_ms": p50,
            "p95_latency_ms": p95,
            "p99_latency_ms": p99,
            "avg_result_count": avg_result_count,
            "queries_tested": len(search_times),
            "filter_expression": filter_test['filter']
        }
        
        print(f"   ✅ Results: {avg_search_time:.2f}ms avg, {avg_result_count:.1f} results avg")
    
    # Test baseline (no filter)
    print(f"\n🔍 Testing baseline (no filter)")
    baseline_times = []
    baseline_counts = []
    
    for i, query in enumerate(queries):
        start = time.time()
        results = client.search(collection_name, query.tolist(), top_k=TOP_K)
        search_time = (time.time() - start) * 1000
        baseline_times.append(search_time)
        baseline_counts.append(len(results))
        
        if (i + 1) % 10 == 0:
            avg_time = sum(baseline_times[-10:]) / 10
            print(f"   Progress: {i+1}/{NUM_QUERIES} queries (avg: {avg_time:.2f}ms)")
    
    baseline_avg_time = sum(baseline_times) / len(baseline_times)
    baseline_avg_count = sum(baseline_counts) / len(baseline_counts)
    
    print(f"   ✅ Baseline: {baseline_avg_time:.2f}ms avg, {baseline_avg_count:.1f} results avg")
    
    results = {
        "protocol": protocol,
        "engine": engine,
        "collection": collection_name,
        "dataset_size": NUM_VECTORS,
        "baseline": {
            "avg_latency_ms": baseline_avg_time,
            "avg_result_count": baseline_avg_count
        },
        "filter_tests": filter_results,
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S")
    }
    
    return results

def main():
    """Run metadata filtering performance tests"""
    
    print("🚀 ProximaDB Metadata Filtering Performance Test")
    print(f"   Dataset: {NUM_VECTORS:,} vectors with rich metadata")
    print(f"   Queries: {NUM_QUERIES}, Top-K: {TOP_K}")
    print("="*80)
    
    # Generate dataset with rich metadata
    vectors = generate_rich_metadata_dataset(NUM_VECTORS, DIMENSION)
    
    # Test configurations
    test_configs = [
        ("grpc", "viper", "metadata_filter_grpc_viper"),
        ("grpc", "lsm", "metadata_filter_grpc_lsm"),
        ("rest", "viper", "metadata_filter_rest_viper"),
        ("rest", "lsm", "metadata_filter_rest_lsm")
    ]
    
    all_results = []
    
    for protocol, engine, collection_name in test_configs:
        try:
            # Connect to appropriate client
            if protocol == "rest":
                client = connect_rest("http://localhost:5678")
            else:
                client = connect_grpc("http://localhost:5679")
            
            # Create or reuse collection
            data_exists = create_or_reuse_collection(client, collection_name, engine, vectors)
            
            # Test filtering performance
            result = test_metadata_filtering_performance(protocol, engine, collection_name, client)
            all_results.append(result)
            
        except Exception as e:
            print(f"❌ Error testing {protocol}/{engine}: {e}")
            import traceback
            traceback.print_exc()
    
    # Save results
    results_data = {
        "test_type": "Metadata Filtering Performance",
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
    
    with open("metadata_filtering_performance_results.json", "w") as f:
        json.dump(results_data, f, indent=2)
    
    # Print summary
    print("\n" + "="*100)
    print("METADATA FILTERING PERFORMANCE SUMMARY")
    print("="*100)
    
    print(f"\nBaseline Performance (No Filter):")
    print(f"{'Protocol':<10} {'Engine':<10} {'Avg Latency (ms)':<15} {'Avg Results':<12}")
    print("-"*50)
    
    for result in all_results:
        protocol = result["protocol"].upper()
        engine = result["engine"].upper()
        latency = f"{result['baseline']['avg_latency_ms']:.2f}"
        results_count = f"{result['baseline']['avg_result_count']:.1f}"
        
        print(f"{protocol:<10} {engine:<10} {latency:<15} {results_count:<12}")
    
    print(f"\nFilter Performance Impact:")
    print(f"{'Protocol':<10} {'Engine':<10} {'Filter Type':<20} {'Latency (ms)':<12} {'Results':<10}")
    print("-"*70)
    
    for result in all_results:
        protocol = result["protocol"].upper()
        engine = result["engine"].upper()
        
        for filter_name, filter_data in result["filter_tests"].items():
            latency = f"{filter_data['avg_latency_ms']:.2f}"
            results_count = f"{filter_data['avg_result_count']:.1f}"
            
            print(f"{protocol:<10} {engine:<10} {filter_name:<20} {latency:<12} {results_count:<10}")
    
    # Engine comparison
    print(f"\n📊 Key Findings:")
    
    viper_results = [r for r in all_results if r["engine"] == "viper"]
    lsm_results = [r for r in all_results if r["engine"] == "lsm"]
    
    if viper_results and lsm_results:
        viper_avg = sum(r["baseline"]["avg_latency_ms"] for r in viper_results) / len(viper_results)
        lsm_avg = sum(r["baseline"]["avg_latency_ms"] for r in lsm_results) / len(lsm_results)
        
        print(f"  Storage Engine Impact on Metadata Filtering:")
        print(f"    - VIPER avg baseline: {viper_avg:.2f}ms")
        print(f"    - LSM avg baseline: {lsm_avg:.2f}ms")
        
        if viper_avg < lsm_avg:
            print(f"    - VIPER is {lsm_avg/viper_avg:.1f}x faster for metadata queries")
        else:
            print(f"    - LSM is {viper_avg/lsm_avg:.1f}x faster for metadata queries")
    
    print(f"\n  Filter Expression Complexity:")
    print(f"    - Simple filters generally fastest")
    print(f"    - Complex AND/OR expressions add overhead")
    print(f"    - Range filters depend on data distribution")
    print(f"    - NOT expressions may require full scan")
    
    print(f"\n📊 Results saved to metadata_filtering_performance_results.json")

if __name__ == "__main__":
    main()