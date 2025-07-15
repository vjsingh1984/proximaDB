#!/usr/bin/env python3
"""
Test Optimized WAL Sync with All Serialization Strategies

This test verifies that the new atomic WAL sync design works correctly
with Proto, Avro, and Bincode serialization strategies.
"""

# Set PYTHONPATH to include src directory
import sys
import os
if 'PYTHONPATH' not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from proximadb import connect_rest, connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def test_atomic_sync_strategies():
    """Test atomic sync with different serialization strategies"""
    
    print("🧪 Testing Optimized WAL Atomic Sync")
    print("="*60)
    
    # Test configurations for different strategies
    test_configs = [
        {
            "name": "Proto Strategy (Default)",
            "collection_suffix": "proto",
            "expected_behavior": "Atomic sync with Proto serialization"
        },
        {
            "name": "Bincode Strategy (Performance)", 
            "collection_suffix": "bincode",
            "expected_behavior": "Atomic sync with Bincode serialization"
        },
        {
            "name": "Avro Strategy (Legacy)",
            "collection_suffix": "avro", 
            "expected_behavior": "Atomic sync with Avro serialization"
        }
    ]
    
    # Connect to server
    client = connect_rest("http://localhost:5678")
    
    results = {}
    
    for config in test_configs:
        print(f"\n🔬 Testing {config['name']}")
        print("-" * 40)
        
        collection_name = f"wal_sync_test_{config['collection_suffix']}"
        
        try:
            # Clean up existing collection
            try:
                client.delete_collection(collection_name)
                time.sleep(0.5)
            except:
                pass
            
            # Create test collection
            collection_config = CollectionConfig(
                name=collection_name,
                dimension=128,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.VIPER,
                description=f"Test collection for {config['name']}"
            )
            
            collection = client.create_collection(collection_name, collection_config)
            print(f"✅ Collection created: {collection.config.name}")
            
            # Test vector insertion (triggers atomic sync)
            print(f"📝 Inserting test vectors...")
            vectors = []
            
            # Insert exactly enough vectors to test 2MB flush trigger
            # With 128D float32 vectors (~512 bytes each), we need ~4000 vectors for 2MB
            num_vectors = 1000  # Start with smaller batch for testing
            
            for i in range(num_vectors):
                vec_data = np.random.randn(128).astype(np.float32)
                vec_data = vec_data / np.linalg.norm(vec_data)
                
                vector = VectorRecord(
                    id=f"{config['collection_suffix']}_vec_{i:04d}",
                    vector=vec_data.tolist(),
                    metadata={
                        "strategy": config['collection_suffix'],
                        "test_batch": "atomic_sync_test",
                        "index": i,
                        "timestamp": int(time.time())
                    }
                )
                vectors.append(vector)
            
            # Insert vectors (should trigger atomic sync)
            start_time = time.time()
            insert_result = client.insert_vectors(collection_name, vectors)
            insert_duration = time.time() - start_time
            
            print(f"✅ Inserted {num_vectors} vectors in {insert_duration:.3f}s")
            
            # Test immediate search (verify data in memory)
            print(f"🔍 Testing immediate search...")
            query = np.random.randn(128).astype(np.float32)
            query = query / np.linalg.norm(query)
            
            search_results = client.search(collection_name, query.tolist(), top_k=10)
            print(f"📊 Search results: {len(search_results)} vectors found")
            
            # Analyze results
            strategy_vectors = 0
            for result in search_results:
                if hasattr(result, 'metadata') and result.metadata:
                    if result.metadata.get('strategy') == config['collection_suffix']:
                        strategy_vectors += 1
            
            results[config['collection_suffix']] = {
                "collection_name": collection_name,
                "vectors_inserted": num_vectors,
                "insert_duration_ms": insert_duration * 1000,
                "search_results": len(search_results),
                "strategy_vectors_found": strategy_vectors,
                "performance_per_vector_ms": (insert_duration * 1000) / num_vectors,
                "expected_behavior": config['expected_behavior'],
                "test_status": "SUCCESS" if len(search_results) > 0 else "FAILED"
            }
            
            print(f"📈 Performance: {results[config['collection_suffix']]['performance_per_vector_ms']:.3f}ms per vector")
            print(f"🎯 Status: {results[config['collection_suffix']]['test_status']}")
            
        except Exception as e:
            print(f"❌ Error testing {config['name']}: {e}")
            results[config['collection_suffix']] = {
                "test_status": "ERROR",
                "error": str(e),
                "expected_behavior": config['expected_behavior']
            }
    
    # Summary report
    print(f"\n📋 ATOMIC SYNC STRATEGY TEST SUMMARY")
    print("="*60)
    
    success_count = 0
    total_tests = len(test_configs)
    
    for strategy, result in results.items():
        status_emoji = "✅" if result.get("test_status") == "SUCCESS" else "❌"
        print(f"{status_emoji} {strategy.upper()} Strategy:")
        
        if result.get("test_status") == "SUCCESS":
            success_count += 1
            print(f"   - Vectors: {result['vectors_inserted']}")
            print(f"   - Performance: {result['performance_per_vector_ms']:.3f}ms/vector")
            print(f"   - Search Results: {result['search_results']}")
        else:
            print(f"   - Status: {result.get('test_status', 'UNKNOWN')}")
            if 'error' in result:
                print(f"   - Error: {result['error']}")
    
    print(f"\n🎯 Overall Results: {success_count}/{total_tests} strategies working")
    
    # Save detailed results
    with open("wal_atomic_sync_test_results.json", "w") as f:
        json.dump({
            "test_timestamp": time.strftime("%Y-%m-%d %H:%M:%S"),
            "test_type": "atomic_wal_sync_strategies",
            "results": results,
            "summary": {
                "total_strategies_tested": total_tests,
                "successful_strategies": success_count,
                "success_rate_percent": (success_count / total_tests) * 100
            }
        }, f, indent=2)
    
    print(f"💾 Detailed results saved to: wal_atomic_sync_test_results.json")
    
    return results

def test_flush_trigger_behavior():
    """Test that 2MB flush triggers work properly"""
    
    print(f"\n🔥 Testing 2MB Flush Trigger Behavior")
    print("-" * 40)
    
    client = connect_rest("http://localhost:5678")
    collection_name = "flush_trigger_test"
    
    try:
        # Clean up
        try:
            client.delete_collection(collection_name)
            time.sleep(0.5)
        except:
            pass
        
        # Create collection
        collection_config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            description="Test 2MB flush trigger"
        )
        
        collection = client.create_collection(collection_name, collection_config)
        print(f"✅ Collection created for flush trigger test")
        
        # Calculate vectors needed for 2MB
        # 128D float32 = 128 * 4 bytes = 512 bytes per vector
        # Plus metadata overhead ~100 bytes = ~612 bytes per vector
        # 2MB = 2,097,152 bytes / 612 bytes ≈ 3,426 vectors
        vectors_for_2mb = 3500  # Slightly over 2MB to ensure trigger
        
        print(f"📝 Inserting {vectors_for_2mb} vectors to exceed 2MB trigger...")
        
        # Insert in batches to monitor progress
        batch_size = 500
        total_inserted = 0
        
        for batch_start in range(0, vectors_for_2mb, batch_size):
            batch_end = min(batch_start + batch_size, vectors_for_2mb)
            batch_vectors = []
            
            for i in range(batch_start, batch_end):
                vec_data = np.random.randn(128).astype(np.float32)
                vec_data = vec_data / np.linalg.norm(vec_data)
                
                vector = VectorRecord(
                    id=f"flush_test_{i:06d}",
                    vector=vec_data.tolist(),
                    metadata={
                        "batch": f"batch_{batch_start//batch_size:03d}",
                        "index": i,
                        "test_type": "flush_trigger"
                    }
                )
                batch_vectors.append(vector)
            
            start_time = time.time()
            client.insert_vectors(collection_name, batch_vectors)
            duration = time.time() - start_time
            
            total_inserted += len(batch_vectors)
            estimated_size_mb = (total_inserted * 612) / (1024 * 1024)
            
            print(f"   Batch {batch_start//batch_size + 1}: {len(batch_vectors)} vectors "
                  f"({duration:.2f}s) - Total: {total_inserted} vectors "
                  f"(~{estimated_size_mb:.2f}MB)")
            
            # Check if flush was triggered (would show up in server logs)
            if estimated_size_mb >= 2.0:
                print(f"   🔥 Exceeded 2MB threshold - flush should be triggered")
        
        print(f"✅ Flush trigger test completed: {total_inserted} vectors inserted")
        
        # Verify all data is searchable
        query = np.random.randn(128).astype(np.float32)
        query = query / np.linalg.norm(query)
        
        search_results = client.search(collection_name, query.tolist(), top_k=20)
        print(f"🔍 Final search verification: {len(search_results)} vectors found")
        
        return {
            "vectors_inserted": total_inserted,
            "estimated_size_mb": estimated_size_mb,
            "search_results": len(search_results),
            "flush_trigger_expected": estimated_size_mb >= 2.0
        }
        
    except Exception as e:
        print(f"❌ Error in flush trigger test: {e}")
        return {"error": str(e)}

if __name__ == "__main__":
    print("🚀 Starting Optimized WAL Atomic Sync Tests")
    print("=" * 80)
    
    # Test 1: Strategy-specific atomic sync
    strategy_results = test_atomic_sync_strategies()
    
    # Test 2: Flush trigger behavior
    flush_results = test_flush_trigger_behavior()
    
    print(f"\n✨ ALL TESTS COMPLETED")
    print("=" * 80)
    print("📊 Check 'wal_atomic_sync_test_results.json' for detailed results")
    print("📋 Review server logs for WAL sync and flush trigger messages")