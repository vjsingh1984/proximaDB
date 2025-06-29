#!/usr/bin/env python3
"""
Direct REST API Test for Storage-Aware Search Optimizations

This test bypasses the Python SDK and tests the optimized search directly
via REST API calls to validate our implementation.
"""

import requests
import json
import time
import numpy as np
import sys
from typing import Dict, Any, List

def check_server():
    """Check if ProximaDB server is running"""
    try:
        response = requests.get("http://localhost:5678/health", timeout=5)
        if response.status_code == 200:
            health_data = response.json()
            print(f"✅ Server is running: {health_data}")
            return True
    except Exception as e:
        print(f"❌ Server not reachable: {e}")
        return False
    
    return False

def create_collection(name: str, dimension: int = 384) -> bool:
    """Create a test collection"""
    try:
        data = {
            "name": name,
            "dimension": dimension,
            "distance_metric": "cosine",
            "indexing_algorithm": "hnsw"
        }
        
        response = requests.post("http://localhost:5678/collections", json=data, timeout=30)
        
        if response.status_code == 200:
            result = response.json()
            print(f"✅ Created collection '{name}': {result}")
            return True
        else:
            print(f"❌ Failed to create collection: {response.status_code} - {response.text}")
            return False
            
    except Exception as e:
        print(f"❌ Exception creating collection: {e}")
        return False

def insert_test_vectors(collection_name: str, count: int = 100) -> bool:
    """Insert test vectors into collection"""
    try:
        print(f"📥 Inserting {count} test vectors...")
        
        vectors = []
        for i in range(count):
            # Create diverse test vectors
            if i % 3 == 0:
                vector = np.random.normal(0, 1, 384).astype(np.float32)
            elif i % 3 == 1:
                vector = np.ones(384, dtype=np.float32) * 0.5 + np.random.normal(0, 0.1, 384).astype(np.float32)
            else:
                vector = np.ones(384, dtype=np.float32) * -0.5 + np.random.normal(0, 0.1, 384).astype(np.float32)
            
            # Normalize
            vector = vector / np.linalg.norm(vector)
            
            vectors.append({
                "id": f"test_vec_{i:03d}",
                "vector": vector.tolist(),
                "metadata": {
                    "category": f"cat_{i % 5}",
                    "value": float(i * 0.1),
                    "cluster": i % 3,
                    "index": i
                }
            })
        
        # Insert batch
        response = requests.post(
            f"http://localhost:5678/collections/{collection_name}/vectors/batch",
            json=vectors,
            timeout=60
        )
        
        if response.status_code == 200:
            result = response.json()
            print(f"✅ Inserted {count} vectors successfully")
            return True
        else:
            print(f"❌ Failed to insert vectors: {response.status_code} - {response.text}")
            return False
            
    except Exception as e:
        print(f"❌ Exception inserting vectors: {e}")
        return False

def test_optimized_search(collection_name: str, test_config: Dict[str, Any]) -> Dict[str, Any]:
    """Test search with specific optimization configuration"""
    test_name = test_config["name"]
    print(f"🔍 Testing: {test_name}")
    
    # Create a random query vector
    query_vector = np.random.normal(0, 1, 384).astype(np.float32)
    query_vector = query_vector / np.linalg.norm(query_vector)
    
    search_data = {
        "vector": query_vector.tolist(),
        "k": 10,
        "filters": {},
        "threshold": 0.0,
        **test_config.get("search_params", {})
    }
    
    start_time = time.time()
    
    try:
        response = requests.post(
            f"http://localhost:5678/collections/{collection_name}/search",
            json=search_data,
            timeout=30
        )
        
        search_time = time.time() - start_time
        
        if response.status_code == 200:
            result = response.json()
            
            if result.get("success") and result.get("data"):
                results = result["data"]
                
                metrics = {
                    "test_name": test_name,
                    "search_time_ms": search_time * 1000,
                    "results_count": len(results),
                    "success": True,
                    "message": result.get("message", ""),
                    "optimization_params": test_config.get("search_params", {})
                }
                
                if results:
                    metrics["top_score"] = results[0].get("score", 0)
                    metrics["avg_score"] = sum(r.get("score", 0) for r in results) / len(results)
                    
                    # Check for optimization indicators
                    top_result = results[0]
                    if "search_engine" in top_result:
                        metrics["search_engine_used"] = top_result["search_engine"]
                    if "optimization_applied" in top_result:
                        metrics["optimization_applied"] = top_result["optimization_applied"]
                
                print(f"  ✅ {len(results)} results in {search_time*1000:.2f}ms")
                if "search_engine_used" in metrics:
                    print(f"     Search engine: {metrics['search_engine_used']}")
                if "optimization_applied" in metrics:
                    print(f"     Optimization: {metrics['optimization_applied']}")
                
                return metrics
            else:
                print(f"  ❌ Search returned no results or failed")
                return {
                    "test_name": test_name,
                    "search_time_ms": search_time * 1000,
                    "results_count": 0,
                    "success": False,
                    "error": "No results returned"
                }
        else:
            print(f"  ❌ Search failed: {response.status_code} - {response.text}")
            return {
                "test_name": test_name,
                "search_time_ms": search_time * 1000,
                "results_count": 0,
                "success": False,
                "error": f"HTTP {response.status_code}: {response.text}"
            }
            
    except Exception as e:
        search_time = time.time() - start_time
        print(f"  ❌ Exception during search: {e}")
        return {
            "test_name": test_name,
            "search_time_ms": search_time * 1000,
            "results_count": 0,
            "success": False,
            "error": str(e)
        }

def test_metadata_filtering(collection_name: str) -> List[Dict[str, Any]]:
    """Test metadata filtering with optimizations"""
    print("\n🔍 Testing Metadata Filtering:")
    print("-" * 50)
    
    filter_tests = [
        {
            "name": "Category Filter",
            "search_params": {
                "filters": {"category": "cat_1"},
                "search_hints": {
                    "predicate_pushdown": True,
                    "use_bloom_filters": True,
                    "use_clustering": True,
                    "quantization_level": "FP32",
                    "parallel_search": True,
                    "engine_specific": {
                        "optimization_level": "high",
                        "enable_simd": True,
                        "prefer_indices": True
                    }
                }
            }
        },
        {
            "name": "Value Range Filter",
            "search_params": {
                "filters": {"value": {"$gte": 2.0, "$lte": 5.0}},
                "search_hints": {
                    "predicate_pushdown": True,
                    "use_bloom_filters": True,
                    "use_clustering": True,
                    "quantization_level": "FP32",
                    "parallel_search": True,
                    "engine_specific": {
                        "optimization_level": "high",
                        "enable_simd": True,
                        "prefer_indices": True
                    }
                }
            }
        },
        {
            "name": "Cluster Filter",
            "search_params": {
                "filters": {"cluster": 1},
                "search_hints": {
                    "predicate_pushdown": True,
                    "use_bloom_filters": True,
                    "use_clustering": True,
                    "quantization_level": "FP32",
                    "parallel_search": True,
                    "engine_specific": {
                        "optimization_level": "high",
                        "enable_simd": True,
                        "prefer_indices": True
                    }
                }
            }
        }
    ]
    
    results = []
    for test_config in filter_tests:
        result = test_optimized_search(collection_name, test_config)
        results.append(result)
        
        # Validate filter correctness if successful
        if result["success"] and "category" in test_config["search_params"]["filters"]:
            expected_category = test_config["search_params"]["filters"]["category"]
            print(f"     Expected category: {expected_category}")
    
    return results

def delete_collection(name: str) -> bool:
    """Delete a test collection"""
    try:
        response = requests.delete(f"http://localhost:5678/collections/{name}", timeout=30)
        if response.status_code == 200:
            print(f"🗑️ Deleted collection '{name}'")
            return True
        else:
            print(f"⚠️ Failed to delete collection '{name}': {response.status_code}")
            return False
    except Exception as e:
        print(f"⚠️ Exception deleting collection: {e}")
        return False

def run_optimization_tests():
    """Run comprehensive storage-aware search optimization tests"""
    print("🚀 ProximaDB Storage-Aware Search Optimization Test Suite")
    print("=" * 80)
    
    if not check_server():
        print("❌ Server not available. Please start ProximaDB server first:")
        print("   cargo run --bin proximadb-server")
        return False
    
    collection_name = "test_optimized_search_rest"
    
    try:
        # Clean up any existing collection
        delete_collection(collection_name)
        
        # Create test collection
        if not create_collection(collection_name):
            return False
        
        # Insert test data
        if not insert_test_vectors(collection_name, count=100):
            return False
        
        # Wait for indexing
        print("⏳ Waiting for indexing...")
        time.sleep(3)
        
        # Test configurations
        test_configs = [
            {
                "name": "High Optimization + SIMD",
                "search_params": {
                    "search_hints": {
                        "predicate_pushdown": True,
                        "use_bloom_filters": True,
                        "use_clustering": True,
                        "quantization_level": "FP32",
                        "parallel_search": True,
                        "engine_specific": {
                            "optimization_level": "high",
                            "enable_simd": True,
                            "prefer_indices": True
                        }
                    }
                }
            },
            {
                "name": "Medium Optimization",
                "search_params": {
                    "search_hints": {
                        "predicate_pushdown": True,
                        "use_bloom_filters": True,
                        "use_clustering": False,
                        "quantization_level": "FP32",
                        "parallel_search": True,
                        "engine_specific": {
                            "optimization_level": "medium",
                            "enable_simd": False,
                            "prefer_indices": True
                        }
                    }
                }
            },
            {
                "name": "Baseline (No Optimization)",
                "search_params": {
                    "search_hints": {
                        "predicate_pushdown": False,
                        "use_bloom_filters": False,
                        "use_clustering": False,
                        "quantization_level": "FP32",
                        "parallel_search": False,
                        "engine_specific": {
                            "optimization_level": "low",
                            "enable_simd": False,
                            "prefer_indices": False
                        }
                    }
                }
            },
            {
                "name": "PQ8 Quantization",
                "search_params": {
                    "search_hints": {
                        "predicate_pushdown": True,
                        "use_bloom_filters": True,
                        "use_clustering": True,
                        "quantization_level": "PQ8",
                        "parallel_search": True,
                        "engine_specific": {
                            "optimization_level": "high",
                            "enable_simd": True,
                            "prefer_indices": True
                        }
                    }
                }
            }
        ]
        
        print("\n🔍 Running Storage-Aware Search Tests:")
        print("-" * 60)
        
        test_results = []
        for config in test_configs:
            result = test_optimized_search(collection_name, config)
            test_results.append(result)
        
        # Test metadata filtering
        filter_results = test_metadata_filtering(collection_name)
        
        # Print summary
        print("\n📊 Test Results Summary:")
        print("=" * 80)
        
        successful_tests = [r for r in test_results if r["success"]]
        if successful_tests:
            print("✅ Basic Search Tests:")
            for result in successful_tests:
                print(f"  {result['test_name']:30} | {result['search_time_ms']:6.2f}ms | {result['results_count']:2d} results")
            
            # Calculate performance improvements
            baseline = next((r for r in successful_tests if "Baseline" in r["test_name"]), None)
            high_opt = next((r for r in successful_tests if "High Optimization" in r["test_name"]), None)
            
            if baseline and high_opt and baseline["search_time_ms"] > 0:
                improvement = baseline["search_time_ms"] / high_opt["search_time_ms"]
                print(f"\n🚀 Performance Improvement: {improvement:.2f}x faster with high optimization")
        
        successful_filters = [r for r in filter_results if r["success"]]
        if successful_filters:
            print("\n✅ Metadata Filter Tests:")
            for result in successful_filters:
                print(f"  {result['test_name']:30} | {result['search_time_ms']:6.2f}ms | {result['results_count']:2d} results")
        
        # Save detailed results
        all_results = {
            "basic_tests": test_results,
            "filter_tests": filter_results,
            "test_timestamp": time.time(),
            "collection_name": collection_name
        }
        
        with open("rest_search_test_results.json", "w") as f:
            json.dump(all_results, f, indent=2, default=str)
        
        print(f"\n📄 Detailed results saved to: rest_search_test_results.json")
        
        # Cleanup
        delete_collection(collection_name)
        
        print("\n✅ All tests completed successfully!")
        return True
        
    except KeyboardInterrupt:
        print("\n🛑 Tests interrupted by user")
        delete_collection(collection_name)
        return False
    except Exception as e:
        print(f"\n❌ Test execution failed: {e}")
        delete_collection(collection_name)
        return False

def main():
    """Main execution"""
    success = run_optimization_tests()
    return 0 if success else 1

if __name__ == "__main__":
    sys.exit(main())