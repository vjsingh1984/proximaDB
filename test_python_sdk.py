#!/usr/bin/env python3
"""
Test script for ProximaDB Python REST SDK
"""

import sys
import os
import numpy as np

# Add the Python SDK to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients', 'python', 'src'))

from proximadb.rest_client import ProximaDBRestClient
from proximadb.models import CollectionConfig

def test_python_sdk():
    print("🐍 ProximaDB Python SDK Test")
    print("============================")
    
    # Initialize client
    client = ProximaDBRestClient(url="http://localhost:5678")
    
    try:
        # Test 1: Health check
        print("1. Testing health check...")
        health = client.health()
        print(f"   Status: {health.status}")
        print(f"   Version: {health.version}")
        
        # Test 2: Create collection
        print("\n2. Creating collection...")
        config = CollectionConfig(
            dimension=384,
            distance_metric="cosine",
            indexing_algorithm="hnsw"
        )
        collection = client.create_collection("python_test_collection", config)
        print(f"   Created collection: {collection.name}")
        
        # Test 3: List collections
        print("\n3. Listing collections...")
        collections = client.list_collections()
        print(f"   Found collections: {collections}")
        
        # Test 4: Get collection details
        print("\n4. Getting collection details...")
        collection_details = client.get_collection("python_test_collection")
        print(f"   Collection ID: {collection_details.id}")
        print(f"   Dimension: {collection_details.dimension}")
        print(f"   Distance metric: {collection_details.metric}")
        
        # Test 5: Insert vector
        print("\n5. Inserting vector...")
        test_vector = np.random.random(384).astype(np.float32)
        result = client.insert_vector(
            "python_test_collection",
            "test_vector_1",
            test_vector,
            metadata={"source": "test", "type": "example"}
        )
        print(f"   Insert count: {result.count}")
        
        # Test 6: Search vectors
        print("\n6. Searching vectors...")
        query_vector = np.random.random(384).astype(np.float32)
        search_results = client.search(
            "python_test_collection",
            query_vector,
            k=5,
            include_metadata=True
        )
        print(f"   Found {len(search_results)} results")
        if search_results:
            print(f"   Best match ID: {search_results[0].id}")
            print(f"   Best match score: {search_results[0].score}")
        
        # Test 7: Batch insert
        print("\n7. Batch inserting vectors...")
        batch_vectors = np.random.random((3, 384)).astype(np.float32)
        batch_ids = ["batch_1", "batch_2", "batch_3"]
        batch_metadata = [
            {"type": "batch", "index": i} for i in range(3)
        ]
        batch_result = client.insert_vectors(
            "python_test_collection",
            batch_vectors,
            batch_ids,
            metadata=batch_metadata
        )
        print(f"   Batch insert - Total: {batch_result.total_count}, Success: {batch_result.successful_count}")
        
        # Test 8: Delete collection
        print("\n8. Deleting collection...")
        delete_success = client.delete_collection("python_test_collection")
        print(f"   Deletion result: {delete_success}")
        
        print("\n✅ Python SDK test completed successfully!")
        
    except Exception as e:
        print(f"\n❌ Error during testing: {e}")
        import traceback
        traceback.print_exc()
        return False
    
    finally:
        client.close()
    
    return True

if __name__ == "__main__":
    success = test_python_sdk()
    sys.exit(0 if success else 1)