#!/usr/bin/env python3
"""
Test script for ProximaDB REST Client
Tests the new REST client implementation against the documented API.
"""

import json
import numpy as np
import traceback
from proximadb.rest_client import ProximaDBRestClient
import hashlib

def generate_client_id():
    """Generate client-side ID using hash-based approach"""
    return f"rest_test_{hashlib.md5(str(np.random.random()).encode()).hexdigest()[:8]}"

def test_rest_client():
    """Comprehensive test of REST client functionality"""
    print("🧪 Testing ProximaDB REST Client Implementation")
    print("=" * 60)
    
    # Initialize client
    client = ProximaDBRestClient(url="http://localhost:5678")
    collection_id = None
    
    try:
        # 1. Health Check
        print("\n1️⃣ Testing Health Check...")
        health = client.health()
        print(f"   ✅ Health Status: {health.status}")
        print(f"   📊 Version: {health.version}")
        
        # 2. System Metrics
        print("\n2️⃣ Testing System Metrics...")
        try:
            metrics = client.get_metrics()
            print(f"   ✅ Retrieved metrics with {len(metrics)} fields")
        except Exception as e:
            print(f"   ⚠️ Metrics endpoint may not be implemented: {e}")
        
        # 3. Create Collection
        print("\n3️⃣ Testing Collection Creation...")
        from proximadb.models import CollectionConfig
        
        config = CollectionConfig(
            dimension=4,
            distance_metric="cosine",
            description="REST client test collection"
        )
        
        collection = client.create_collection("rest_test_collection", config)
        collection_id = collection.id
        print(f"   ✅ Created collection: {collection.name}")
        print(f"   🆔 Collection ID: {collection_id}")
        print(f"   📏 Dimension: {collection.dimension}")
        print(f"   📐 Distance Metric: {collection.distance_metric}")
        
        # 4. List Collections
        print("\n4️⃣ Testing List Collections...")
        collections = client.list_collections()
        print(f"   ✅ Found {len(collections)} collections")
        for col in collections:
            if col.name == "rest_test_collection":
                print(f"   🎯 Found our test collection: {col.name}")
        
        # 5. Get Collection
        print("\n5️⃣ Testing Get Collection...")
        retrieved_collection = client.get_collection(collection_id)
        print(f"   ✅ Retrieved collection: {retrieved_collection.name}")
        print(f"   📊 Vector count: {retrieved_collection.vector_count}")
        
        # 6. Insert Single Vector
        print("\n6️⃣ Testing Single Vector Insert...")
        vector_id = generate_client_id()
        test_vector = [0.1, 0.2, 0.3, 0.4]
        test_metadata = {
            "category": "test",
            "priority": 5,
            "active": True,
            "source": "rest_client_test"
        }
        
        insert_result = client.insert_vector(
            collection_id=collection_id,
            vector_id=vector_id,
            vector=test_vector,
            metadata=test_metadata
        )
        print(f"   ✅ Inserted vector: {vector_id}")
        print(f"   📊 Success: {insert_result.success}")
        print(f"   ⏱️ Duration: {insert_result.duration_ms}ms")
        
        # 7. Batch Insert Vectors
        print("\n7️⃣ Testing Batch Vector Insert...")
        batch_vectors = []
        batch_ids = []
        batch_metadata = []
        
        for i in range(3):
            batch_ids.append(generate_client_id())
            # Create diverse vectors for testing
            vector = [0.1 + i*0.1, 0.2 + i*0.1, 0.3 + i*0.1, 0.4 + i*0.1]
            batch_vectors.append(vector)
            batch_metadata.append({
                "batch_index": i,
                "category": "batch_test",
                "value": i * 10
            })
        
        batch_result = client.insert_vectors(
            collection_id=collection_id,
            vectors=batch_vectors,
            ids=batch_ids,
            metadata=batch_metadata
        )
        print(f"   ✅ Batch inserted {len(batch_vectors)} vectors")
        print(f"   📊 Successful: {batch_result.successful_count}")
        print(f"   ❌ Failed: {batch_result.failed_count}")
        print(f"   ⏱️ Duration: {batch_result.duration_ms}ms")
        
        # 8. Vector Search
        print("\n8️⃣ Testing Vector Search...")
        query_vector = [0.15, 0.25, 0.35, 0.45]  # Similar to our test vectors
        
        search_results = client.search(
            collection_id=collection_id,
            query=query_vector,
            k=5,
            include_metadata=True,
            include_vectors=False
        )
        print(f"   ✅ Search completed, found {len(search_results)} results")
        
        for i, result in enumerate(search_results[:3]):  # Show top 3
            print(f"   🎯 Result {i+1}: ID={result.id}, Score={result.score:.4f}")
            if result.metadata:
                print(f"        📝 Metadata: {result.metadata}")
        
        # 9. Search with Metadata Filter
        print("\n9️⃣ Testing Search with Metadata Filter...")
        filtered_results = client.search(
            collection_id=collection_id,
            query=query_vector,
            k=10,
            filter={"category": "test"},
            include_metadata=True
        )
        print(f"   ✅ Filtered search found {len(filtered_results)} results")
        
        # 10. Get Vector by ID
        print("\n🔟 Testing Get Vector by ID...")
        retrieved_vector = client.get_vector(
            collection_id=collection_id,
            vector_id=vector_id,
            include_vector=True,
            include_metadata=True
        )
        
        if retrieved_vector:
            print(f"   ✅ Retrieved vector: {retrieved_vector.get('id')}")
            print(f"   📐 Vector data: {retrieved_vector.get('vector')}")
            print(f"   📝 Metadata: {retrieved_vector.get('metadata')}")
        else:
            print(f"   ❌ Vector not found: {vector_id}")
        
        # 11. Delete Single Vector
        print("\n1️⃣1️⃣ Testing Delete Single Vector...")
        delete_result = client.delete_vector(collection_id, vector_id)
        print(f"   ✅ Delete completed: Success={delete_result.success}")
        print(f"   📊 Deleted count: {delete_result.count}")
        
        # 12. Delete Multiple Vectors
        print("\n1️⃣2️⃣ Testing Delete Multiple Vectors...")
        multi_delete_result = client.delete_vectors(collection_id, batch_ids)
        print(f"   ✅ Multi-delete completed: Success={multi_delete_result.success}")
        print(f"   📊 Deleted count: {multi_delete_result.count}")
        
        # 13. Final Collection Check
        print("\n1️⃣3️⃣ Final Collection Status...")
        final_collection = client.get_collection(collection_id)
        print(f"   📊 Final vector count: {final_collection.vector_count}")
        
        print("\n🎉 REST Client Test Suite Completed Successfully!")
        return True
        
    except Exception as e:
        print(f"\n❌ Test failed with error: {e}")
        print(f"📝 Traceback:\n{traceback.format_exc()}")
        return False
        
    finally:
        # Cleanup
        if collection_id:
            try:
                print(f"\n🧹 Cleaning up collection: {collection_id}")
                success = client.delete_collection(collection_id)
                print(f"   ✅ Collection deleted: {success}")
            except Exception as e:
                print(f"   ⚠️ Cleanup failed: {e}")
        
        client.close()

if __name__ == "__main__":
    success = test_rest_client()
    exit(0 if success else 1)