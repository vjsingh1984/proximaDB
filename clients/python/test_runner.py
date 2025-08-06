#!/usr/bin/env python3
"""
Simple test runner to verify SDK functionality
"""
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "src"))

from proximadb import ProximaDBClient
from proximadb.models import VectorRecord, CollectionConfig, DistanceMetric, StorageEngine
import numpy as np


def test_basic_operations():
    """Test basic CRUD operations"""
    print("Testing basic operations...")
    
    # Create clients
    rest_client = ProximaDBClient(url="http://localhost:5678")
    grpc_client = ProximaDBClient(url="grpc://localhost:5679")
    
    test_collection = f"test_basic_{int(time.time())}"
    
    try:
        # Test 1: Create collection
        print("1. Creating collection...")
        config = CollectionConfig(
            name=test_collection,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER
        )
        
        # REST
        rest_result = rest_client.create_collection(test_collection, config)
        print(f"   REST: {rest_result}")
        
        # Clean up and recreate for gRPC
        rest_client.delete_collection(test_collection)
        time.sleep(0.5)
        
        grpc_result = grpc_client.create_collection(test_collection, config)
        print(f"   gRPC: {grpc_result}")
        
        # Test 2: Insert vectors
        print("\n2. Inserting vectors...")
        vectors = []
        for i in range(10):
            vec = VectorRecord(
                id=f"vec_{i}",
                vector=np.random.randn(128).tolist(),
                metadata={"index": i, "test": True}
            )
            vectors.append(vec)
        
        insert_result = grpc_client.insert_vectors(test_collection, records=vectors)
        print(f"   Insert result: {insert_result}")
        
        # Wait for indexing
        time.sleep(1)
        
        # Test 3: Search
        print("\n3. Searching vectors...")
        query = np.random.randn(128).tolist()
        search_results = grpc_client.search(
            collection_id=test_collection,
            query_vector=query,
            top_k=5
        )
        print(f"   Found {len(search_results.results)} results")
        for i, result in enumerate(search_results.results[:3]):
            print(f"   #{i+1}: {result.id} (score: {result.score:.4f})")
        
        # Test 4: Get vector
        print("\n4. Getting specific vector...")
        vector = grpc_client.get_vector(test_collection, "vec_0")
        print(f"   Retrieved: {vector.id}, metadata: {vector.metadata}")
        
        # Test 5: List collections
        print("\n5. Listing collections...")
        collections = grpc_client.list_collections()
        print(f"   Found {len(collections)} collections")
        
        print("\n✅ All basic operations completed successfully!")
        
    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        # Cleanup
        try:
            rest_client.delete_collection(test_collection)
        except:
            pass
        try:
            grpc_client.delete_collection(test_collection)
        except:
            pass


def test_unified_features():
    """Test unified architecture features"""
    print("\n\nTesting unified features...")
    
    # Test with batching and caching enabled
    from proximadb.batching_unified import BatchConfig, BatchStrategy
    
    client = ProximaDBClient(
        url="http://localhost:5678",
        grpc_url="localhost:5679",
        enable_batching=True,
        batch_config=BatchConfig(
            strategy=BatchStrategy.HYBRID,
            max_batch_size=100
        ),
        enable_caching=True,
        cache_config={
            "strategy": "LRU",
            "max_size": 1000
        }
    )
    
    test_collection = f"test_unified_{int(time.time())}"
    
    try:
        # Create collection
        client.create_collection(
            test_collection,
            dimension=64,
            engine="sst"  # Test SST engine
        )
        
        # Insert many vectors (will be batched)
        vectors = []
        for i in range(50):
            vec = VectorRecord(
                id=f"unified_{i}",
                vector=np.random.randn(64).tolist(),
                metadata={"batch": i // 10}
            )
            vectors.append(vec)
        
        result = client.insert_vectors(test_collection, records=vectors)
        print(f"   Batch insert: {result}")
        
        time.sleep(1)
        
        # Search multiple times (will be cached)
        query = np.random.randn(64).tolist()
        
        start = time.time()
        results1 = client.search(test_collection, query, top_k=10)
        time1 = time.time() - start
        
        start = time.time()
        results2 = client.search(test_collection, query, top_k=10)  # Should be cached
        time2 = time.time() - start
        
        print(f"   First search: {time1*1000:.2f}ms")
        print(f"   Cached search: {time2*1000:.2f}ms (speedup: {time1/time2:.1f}x)")
        
        # Get stats if available
        try:
            routing_stats = client.get_routing_stats()
            print(f"   Routing stats: {routing_stats}")
        except:
            pass
        
        print("\n✅ Unified features working correctly!")
        
    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        try:
            client.delete_collection(test_collection)
        except:
            pass


def main():
    """Run all tests"""
    print("ProximaDB Python SDK Test Runner")
    print("=" * 50)
    
    # Check server
    try:
        client = ProximaDBClient(url="http://localhost:5678")
        # Try to list collections as a health check
        collections = client.list_collections()
        print(f"Server is running (found {len(collections)} collections)")
    except Exception as e:
        print(f"❌ Server not available: {e}")
        print("Please start the server with:")
        print("  ./target/release/proximadb-server --config demo/local-demo-config.toml")
        return 1
    
    # Run tests
    test_basic_operations()
    test_unified_features()
    
    print("\n" + "=" * 50)
    print("Test run completed!")
    return 0


if __name__ == "__main__":
    sys.exit(main())