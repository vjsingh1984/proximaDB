#!/usr/bin/env python3
"""
Simple Storage-Aware Search Test - Synchronous Version

Quick test of the optimized search functionality with the REST client.
"""

import sys
import time
import numpy as np
from pathlib import Path

# Add the Python SDK to the path
sys.path.insert(0, str(Path(__file__).parent / "clients" / "python" / "src"))

try:
    from proximadb.client import ProximaDBClient
    from proximadb.models import CollectionConfig
    from proximadb.exceptions import ProximaDBError
except ImportError as e:
    print(f"❌ Failed to import ProximaDB SDK: {e}")
    print("Make sure you're running from the project root directory")
    sys.exit(1)

def test_optimized_search():
    """Test the optimized search functionality"""
    print("🚀 Testing Storage-Aware Search Optimizations")
    print("=" * 60)
    
    # Initialize client (REST)
    client = ProximaDBClient("http://localhost:5678")
    
    # Test collection name
    collection_name = "test_optimized_search"
    
    try:
        # Clean up any existing collection
        try:
            client.delete_collection(collection_name)
            print(f"🗑️ Cleaned up existing collection: {collection_name}")
        except:
            pass
        
        # Create test collection with VIPER storage
        print(f"📦 Creating collection '{collection_name}' with VIPER storage...")
        
        # Note: This uses the old API style for now
        client._http_post(f"/collections", {
            "name": collection_name,
            "dimension": 384,
            "distance_metric": "cosine",
            "indexing_algorithm": "hnsw"
        })
        print(f"✅ Created collection successfully")
        
        # Insert test vectors
        print("📥 Inserting test vectors...")
        vectors = []
        for i in range(50):  # Small test set
            vector = np.random.normal(0, 1, 384).astype(np.float32)
            vector = vector / np.linalg.norm(vector)
            
            vectors.append({
                "id": f"test_vec_{i:03d}",
                "vector": vector.tolist(),
                "metadata": {
                    "category": f"cat_{i % 5}",
                    "value": float(i * 0.1),
                    "cluster": i % 3
                }
            })
        
        # Insert in batch
        response = client._http_post(f"/collections/{collection_name}/vectors/batch", vectors)
        print(f"✅ Inserted {len(vectors)} vectors successfully")
        
        # Wait a moment for indexing
        time.sleep(2)
        
        # Test different optimization levels
        test_configs = [
            {
                "name": "High Optimization + SIMD",
                "data": {
                    "vector": np.random.normal(0, 1, 384).astype(np.float32).tolist(),
                    "k": 10,
                    "filters": {},
                    "threshold": 0.0,
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
                "data": {
                    "vector": np.random.normal(0, 1, 384).astype(np.float32).tolist(),
                    "k": 10,
                    "filters": {},
                    "threshold": 0.0,
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
                "data": {
                    "vector": np.random.normal(0, 1, 384).astype(np.float32).tolist(),
                    "k": 10,
                    "filters": {},
                    "threshold": 0.0,
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
            }
        ]
        
        print("\n🔍 Running Search Optimization Tests:")
        print("-" * 60)
        
        results = []
        for config in test_configs:
            print(f"Testing: {config['name']}")
            
            start_time = time.time()
            try:
                # Normalize the vector
                vector = np.array(config['data']['vector'])
                vector = vector / np.linalg.norm(vector)
                config['data']['vector'] = vector.tolist()
                
                response = client._http_post(f"/collections/{collection_name}/search", config['data'])
                search_time = time.time() - start_time
                
                if response.get('success') and response.get('data'):
                    result_count = len(response['data'])
                    print(f"  ✅ {result_count} results in {search_time*1000:.2f}ms")
                    
                    results.append({
                        "name": config['name'],
                        "time_ms": search_time * 1000,
                        "results": result_count,
                        "success": True
                    })
                    
                    # Show top result details
                    if response['data']:
                        top_result = response['data'][0]
                        print(f"     Top result: ID={top_result.get('id', 'unknown')}, Score={top_result.get('score', 0):.4f}")
                        if 'search_engine' in top_result:
                            print(f"     Search engine: {top_result['search_engine']}")
                        if 'optimization_applied' in top_result:
                            print(f"     Optimization applied: {top_result['optimization_applied']}")
                else:
                    print(f"  ❌ Search failed or returned no results")
                    results.append({
                        "name": config['name'],
                        "time_ms": (time.time() - start_time) * 1000,
                        "results": 0,
                        "success": False
                    })
                
            except Exception as e:
                search_time = time.time() - start_time
                print(f"  ❌ Search failed: {e}")
                results.append({
                    "name": config['name'],
                    "time_ms": search_time * 1000,
                    "results": 0,
                    "success": False,
                    "error": str(e)
                })
        
        # Test metadata filtering
        print("\n🔍 Testing Metadata Filtering:")
        print("-" * 60)
        
        filter_tests = [
            {
                "name": "Category Filter",
                "filter": {"category": "cat_1"}
            },
            {
                "name": "Value Range Filter",
                "filter": {"value": {"$gte": 1.0, "$lte": 3.0}}
            }
        ]
        
        for filter_test in filter_tests:
            try:
                query_data = {
                    "vector": (np.random.normal(0, 1, 384).astype(np.float32) / np.linalg.norm(np.random.normal(0, 1, 384))).tolist(),
                    "k": 5,
                    "filters": filter_test["filter"],
                    "threshold": 0.0,
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
                
                start_time = time.time()
                response = client._http_post(f"/collections/{collection_name}/search", query_data)
                search_time = time.time() - start_time
                
                if response.get('success') and response.get('data'):
                    result_count = len(response['data'])
                    print(f"✅ {filter_test['name']}: {result_count} results in {search_time*1000:.2f}ms")
                    
                    # Verify filter worked
                    if response['data'] and 'category' in filter_test['filter']:
                        first_result = response['data'][0]
                        if first_result.get('metadata', {}).get('category') == filter_test['filter']['category']:
                            print(f"   ✓ Filter applied correctly")
                        else:
                            print(f"   ⚠️ Filter may not have been applied")
                else:
                    print(f"❌ {filter_test['name']}: Search failed")
                    
            except Exception as e:
                print(f"❌ {filter_test['name']}: Failed - {e}")
        
        # Performance summary
        print("\n📊 Performance Summary:")
        print("-" * 60)
        
        successful_results = [r for r in results if r['success']]
        if successful_results:
            for result in successful_results:
                print(f"{result['name']:30} | {result['time_ms']:6.2f}ms | {result['results']:2d} results")
            
            # Calculate improvements
            baseline = next((r for r in successful_results if "Baseline" in r['name']), None)
            high_opt = next((r for r in successful_results if "High Optimization" in r['name']), None)
            
            if baseline and high_opt and baseline['time_ms'] > 0:
                improvement = baseline['time_ms'] / high_opt['time_ms']
                print(f"\n🚀 Performance Improvement: {improvement:.2f}x faster with high optimization")
        
        # Cleanup
        client.delete_collection(collection_name)
        print(f"\n🗑️ Cleaned up test collection")
        
        print("\n✅ Test completed successfully!")
        return True
        
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        
        # Try to cleanup
        try:
            client.delete_collection(collection_name)
        except:
            pass
        
        return False

def main():
    """Main test execution"""
    # Check if server is reachable
    import socket
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    result = sock.connect_ex(('localhost', 5678))
    sock.close()
    
    if result != 0:
        print("❌ ProximaDB REST server not reachable on localhost:5678")
        print("   Please start the server first: cargo run --bin proximadb-server")
        return 1
    
    success = test_optimized_search()
    return 0 if success else 1

if __name__ == "__main__":
    sys.exit(main())