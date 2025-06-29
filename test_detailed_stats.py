#!/usr/bin/env python3
"""
Detailed statistics test for storage-aware search optimizations
Tracks: record counts, insertion times, individual search times
"""

import requests
import json
import numpy as np
import time
from datetime import datetime
from typing import Dict, List, Tuple
import sys

BASE_URL = "http://localhost:5678"
VECTOR_DIM = 384

class DetailedStatsTester:
    def __init__(self):
        self.stats = {
            "collections": {},
            "insertion_times": {},
            "search_results": [],
            "test_summary": {}
        }
        self.collections_created = []
    
    def cleanup(self):
        """Clean up test collections"""
        for collection in self.collections_created:
            try:
                requests.delete(f"{BASE_URL}/collections/{collection}")
            except:
                pass
    
    def create_test_vectors(self, count: int) -> List[Dict]:
        """Create test vectors with realistic distribution"""
        vectors = []
        
        # Create 5 distinct clusters
        cluster_centers = [
            np.random.randn(VECTOR_DIM) for _ in range(5)
        ]
        
        for i in range(count):
            cluster_id = i % 5
            base_vector = cluster_centers[cluster_id]
            noise = np.random.randn(VECTOR_DIM) * 0.1
            vector = base_vector + noise
            vector = vector / (np.linalg.norm(vector) + 1e-8)
            
            vectors.append({
                "id": f"vec_{i:06d}",
                "vector": vector.astype(np.float32).tolist(),
                "metadata": {
                    "index": i,
                    "cluster_id": cluster_id,
                    "category": f"cat_{i % 10}",
                    "timestamp": int(time.time() * 1000) + i,
                    "value": float(i),
                    "tags": [f"tag_{j}" for j in range(i % 3 + 1)]
                }
            })
        
        return vectors
    
    def run_detailed_tests(self):
        """Run tests with detailed statistics"""
        print("📊 ProximaDB Storage-Aware Search - Detailed Statistics Test")
        print("=" * 80)
        print(f"Test started at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
        print(f"Vector dimension: {VECTOR_DIM}")
        print("=" * 80)
        
        # Test configurations
        test_sizes = [100, 1000, 5000]
        storage_engines = ["viper", "lsm"]
        
        for size in test_sizes:
            print(f"\n\n🔷 Testing with {size} vectors")
            print("-" * 70)
            
            # Create test vectors once for consistency
            print(f"Creating {size} test vectors...")
            vectors = self.create_test_vectors(size)
            
            for engine in storage_engines:
                collection_name = f"stats_test_{engine}_{size}"
                self.collections_created.append(collection_name)
                
                print(f"\n📦 {engine.upper()} Storage Engine - {size} vectors")
                print("=" * 50)
                
                # Initialize stats for this collection
                self.stats["collections"][collection_name] = {
                    "engine": engine,
                    "vector_count": size,
                    "dimension": VECTOR_DIM,
                    "creation_time": None,
                    "insertion_time": None,
                    "search_tests": []
                }
                
                # Step 1: Create collection
                print("\n1. Creating collection...")
                start_time = time.time()
                response = requests.post(f"{BASE_URL}/collections", json={
                    "name": collection_name,
                    "dimension": VECTOR_DIM,
                    "distance_metric": "cosine",
                    "storage_engine": engine,
                    "indexing_algorithm": "hnsw"
                })
                creation_time = time.time() - start_time
                
                if response.status_code == 200:
                    self.stats["collections"][collection_name]["creation_time"] = creation_time * 1000
                    print(f"   ✓ Collection created in {creation_time*1000:.2f}ms")
                else:
                    print(f"   ✗ Failed to create collection: {response.status_code}")
                    continue
                
                # Step 2: Insert vectors
                print(f"\n2. Inserting {size} vectors...")
                batch_size = min(100, size)
                total_insertion_start = time.time()
                inserted_count = 0
                
                for i in range(0, len(vectors), batch_size):
                    batch = vectors[i:i+batch_size]
                    batch_start = time.time()
                    
                    response = requests.post(
                        f"{BASE_URL}/collections/{collection_name}/vectors/batch",
                        json=batch
                    )
                    
                    batch_time = time.time() - batch_start
                    
                    if response.status_code == 200:
                        inserted_count += len(batch)
                        print(f"   Batch {i//batch_size + 1}/{(len(vectors) + batch_size - 1)//batch_size}: "
                              f"{len(batch)} vectors in {batch_time*1000:.2f}ms "
                              f"({len(batch)/batch_time:.0f} vec/s)")
                    else:
                        print(f"   ✗ Batch {i//batch_size + 1} failed: {response.status_code}")
                
                total_insertion_time = time.time() - total_insertion_start
                self.stats["collections"][collection_name]["insertion_time"] = total_insertion_time * 1000
                self.stats["collections"][collection_name]["actual_count"] = inserted_count
                
                print(f"\n   📊 Insertion Summary:")
                print(f"      Total vectors inserted: {inserted_count}/{size}")
                print(f"      Total time: {total_insertion_time:.2f}s")
                print(f"      Average rate: {inserted_count/total_insertion_time:.0f} vectors/second")
                
                # Step 3: Wait a moment for indexing
                print("\n3. Waiting for indexing...")
                time.sleep(2)
                
                # Step 4: Verify collection status
                print("\n4. Collection status:")
                response = requests.get(f"{BASE_URL}/collections/{collection_name}")
                if response.status_code == 200:
                    collection_info = response.json().get('data', {})
                    actual_count = collection_info.get('vector_count', 0)
                    print(f"   Vector count in collection: {actual_count}")
                    print(f"   Storage engine: {collection_info.get('storage_engine', 'unknown')}")
                
                # Step 5: Run search tests
                print("\n5. Running search performance tests...")
                
                # Create different query vectors
                query_vectors = {
                    "random": np.random.randn(VECTOR_DIM).astype(np.float32),
                    "cluster_0": vectors[0]["vector"],  # From cluster 0
                    "cluster_2": vectors[2]["vector"] if size > 2 else vectors[0]["vector"]  # From cluster 2
                }
                
                # Normalize query vectors
                for name, vec in query_vectors.items():
                    vec_array = np.array(vec)
                    query_vectors[name] = (vec_array / np.linalg.norm(vec_array)).tolist()
                
                # Test configurations
                search_configs = [
                    {
                        "name": "Baseline (No Optimization)",
                        "k": 10,
                        "hints": {}
                    },
                    {
                        "name": "Basic Search",
                        "k": 50,
                        "hints": {}
                    },
                    {
                        "name": "Storage-Aware Optimized",
                        "k": 50,
                        "hints": {
                            "storage_aware": True,
                            "optimization_level": "high"
                        }
                    },
                    {
                        "name": "VIPER Quantized (PQ8)" if engine == "viper" else "LSM Tiered Search",
                        "k": 50,
                        "hints": {
                            "storage_aware": True,
                            "quantization_level": "PQ8" if engine == "viper" else "FP32",
                            "level_aware_search": True if engine == "lsm" else False
                        }
                    },
                    {
                        "name": "VIPER Quantized (PQ4)" if engine == "viper" else "LSM Bloom Filters",
                        "k": 50,
                        "hints": {
                            "storage_aware": True,
                            "quantization_level": "PQ4" if engine == "viper" else "FP32",
                            "use_bloom_filters": True if engine == "lsm" else False
                        }
                    }
                ]
                
                # Run searches
                for config in search_configs:
                    print(f"\n   🔍 {config['name']}:")
                    config_times = []
                    config_results = []
                    
                    for query_name, query_vec in query_vectors.items():
                        search_data = {
                            "vector": query_vec,
                            "k": config["k"],
                            "include_metadata": False,
                            "search_hints": config["hints"]
                        }
                        
                        # Run search 3 times for average
                        query_times = []
                        for run in range(3):
                            start_time = time.time()
                            response = requests.post(
                                f"{BASE_URL}/collections/{collection_name}/search",
                                json=search_data
                            )
                            search_time = (time.time() - start_time) * 1000
                            
                            if response.status_code == 200:
                                results = response.json().get('data', [])
                                query_times.append(search_time)
                                if run == 0:  # Store first run results
                                    config_results.append({
                                        "query": query_name,
                                        "results_count": len(results),
                                        "top_score": results[0].get('score', 0) if results else 0
                                    })
                        
                        if query_times:
                            avg_time = sum(query_times) / len(query_times)
                            config_times.append(avg_time)
                            result_info = config_results[-1] if config_results else {"results_count": 0}
                            print(f"      Query '{query_name}': {avg_time:6.2f}ms "
                                  f"({result_info['results_count']} results)")
                    
                    if config_times:
                        overall_avg = sum(config_times) / len(config_times)
                        print(f"      Average: {overall_avg:6.2f}ms")
                        
                        # Store detailed stats
                        self.stats["collections"][collection_name]["search_tests"].append({
                            "config_name": config["name"],
                            "k": config["k"],
                            "avg_time_ms": overall_avg,
                            "individual_times": config_times,
                            "results": config_results
                        })
                
                # Step 6: Test with filters
                print("\n6. Testing filtered searches...")
                filter_tests = [
                    {
                        "name": "Category Filter",
                        "filter": {"category": "cat_1"}
                    },
                    {
                        "name": "Range Filter", 
                        "filter": {"value": {"$gte": 10, "$lte": 50}}
                    },
                    {
                        "name": "Cluster Filter",
                        "filter": {"cluster_id": 1}
                    }
                ]
                
                for filter_test in filter_tests:
                    search_data = {
                        "vector": query_vectors["random"],
                        "k": 20,
                        "filters": filter_test["filter"],
                        "include_metadata": True,
                        "search_hints": {
                            "storage_aware": True,
                            "predicate_pushdown": True if engine == "viper" else False
                        }
                    }
                    
                    start_time = time.time()
                    response = requests.post(
                        f"{BASE_URL}/collections/{collection_name}/search",
                        json=search_data
                    )
                    search_time = (time.time() - start_time) * 1000
                    
                    if response.status_code == 200:
                        results = response.json().get('data', [])
                        # Verify filter correctness
                        if results and "category" in filter_test["filter"]:
                            correct = all(r.get('metadata', {}).get('category') == filter_test["filter"]["category"] 
                                        for r in results[:5])  # Check first 5
                            status = "✓" if correct else "✗"
                        else:
                            status = ""
                        
                        print(f"   {filter_test['name']:20}: {search_time:6.2f}ms "
                              f"({len(results)} results) {status}")
        
        # Print comprehensive summary
        self.print_detailed_summary()
    
    def print_detailed_summary(self):
        """Print detailed summary of all tests"""
        print("\n\n" + "=" * 80)
        print("📊 COMPREHENSIVE TEST SUMMARY")
        print("=" * 80)
        
        # Summary by collection
        for coll_name, coll_stats in self.stats["collections"].items():
            print(f"\n📦 Collection: {coll_name}")
            print(f"   Engine: {coll_stats['engine'].upper()}")
            print(f"   Target vectors: {coll_stats['vector_count']}")
            print(f"   Actual vectors: {coll_stats.get('actual_count', 'unknown')}")
            print(f"   Dimension: {coll_stats['dimension']}")
            print(f"   Creation time: {coll_stats.get('creation_time', 0):.2f}ms")
            print(f"   Insertion time: {coll_stats.get('insertion_time', 0):.2f}ms")
            
            if coll_stats.get('insertion_time', 0) > 0:
                rate = coll_stats.get('actual_count', 0) / (coll_stats.get('insertion_time', 1) / 1000)
                print(f"   Insertion rate: {rate:.0f} vectors/second")
            
            print("\n   Search Performance:")
            for test in coll_stats.get('search_tests', []):
                print(f"   • {test['config_name']}:")
                print(f"     Average time: {test['avg_time_ms']:.2f}ms")
                print(f"     K value: {test['k']}")
                if test.get('individual_times'):
                    print(f"     Query breakdown:")
                    for i, (time_ms, result) in enumerate(zip(test['individual_times'], test.get('results', []))):
                        query_name = result.get('query', f'query_{i}')
                        count = result.get('results_count', 0)
                        print(f"       - {query_name}: {time_ms:.2f}ms ({count} results)")
        
        # Performance comparison
        print("\n\n📈 Performance Comparison")
        print("-" * 60)
        
        # Compare baseline vs optimized for each engine and size
        for size in [100, 1000, 5000]:
            print(f"\nDataset size: {size} vectors")
            
            for engine in ["viper", "lsm"]:
                coll_name = f"stats_test_{engine}_{size}"
                if coll_name not in self.stats["collections"]:
                    continue
                
                coll_stats = self.stats["collections"][coll_name]
                tests = coll_stats.get("search_tests", [])
                
                baseline = next((t for t in tests if "Baseline" in t["config_name"]), None)
                optimized = next((t for t in tests if "Storage-Aware Optimized" in t["config_name"]), None)
                
                if baseline and optimized:
                    speedup = baseline["avg_time_ms"] / optimized["avg_time_ms"]
                    improvement = ((baseline["avg_time_ms"] - optimized["avg_time_ms"]) / baseline["avg_time_ms"]) * 100
                    
                    print(f"  {engine.upper()}: {speedup:.2f}x speedup ({improvement:.1f}% faster)")
                    print(f"         Baseline: {baseline['avg_time_ms']:.2f}ms → Optimized: {optimized['avg_time_ms']:.2f}ms")
        
        # Overall statistics
        print("\n\n📊 Overall Statistics")
        print("-" * 60)
        
        total_vectors_inserted = sum(
            stats.get('actual_count', 0) 
            for stats in self.stats["collections"].values()
        )
        
        total_insertion_time = sum(
            stats.get('insertion_time', 0) / 1000  # Convert to seconds
            for stats in self.stats["collections"].values()
        )
        
        total_searches = sum(
            len(stats.get('search_tests', [])) * 3 * 3  # configs * queries * runs
            for stats in self.stats["collections"].values()
        )
        
        print(f"Total vectors inserted: {total_vectors_inserted:,}")
        print(f"Total insertion time: {total_insertion_time:.2f}s")
        if total_insertion_time > 0:
            print(f"Overall insertion rate: {total_vectors_inserted/total_insertion_time:.0f} vectors/second")
        print(f"Total search operations: {total_searches}")
        
        # Save detailed results
        with open("detailed_stats_results.json", "w") as f:
            json.dump(self.stats, f, indent=2)
        
        print("\n📄 Detailed results saved to: detailed_stats_results.json")
        print("\n✅ Testing complete!")

def main():
    """Run detailed statistics test"""
    # Check server
    try:
        response = requests.get(f"{BASE_URL}/health", timeout=2)
        if response.status_code != 200:
            print("❌ ProximaDB server not healthy")
            return 1
    except:
        print("❌ ProximaDB server not reachable on localhost:5678")
        print("   Please start the server first: cargo run --bin proximadb-server")
        return 1
    
    tester = DetailedStatsTester()
    
    try:
        tester.run_detailed_tests()
        return 0
    except KeyboardInterrupt:
        print("\n\n🛑 Test interrupted by user")
        return 1
    except Exception as e:
        print(f"\n\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return 1
    finally:
        print("\n🧹 Cleaning up test collections...")
        tester.cleanup()
        print("✓ Cleanup complete")

if __name__ == "__main__":
    sys.exit(main())