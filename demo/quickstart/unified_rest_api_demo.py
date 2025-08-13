#!/usr/bin/env python3
"""
Demo of ProximaDB's unified REST API (proto-aligned)

This example shows how to use the new unified REST endpoints that match
the gRPC/proto structure for consistency across protocols.
"""

import requests
import json
import time

BASE_URL = "http://localhost:5678"

def unified_collection_operation(operation, collection_id=None, config=None):
    """Execute a unified collection operation"""
    payload = {
        "operation": operation,
        "collection_id": collection_id,
        "config": config
    }
    
    response = requests.post(
        f"{BASE_URL}/api/v1/collection",
        json=payload,
        headers={"Content-Type": "application/json"}
    )
    
    return response.json()

def unified_vector_batch(collection_id, vectors):
    """Execute a unified vector batch operation"""
    payload = {
        "collection_id": collection_id,
        "vectors": vectors
    }
    
    response = requests.post(
        f"{BASE_URL}/api/v1/vector/batch",
        json=payload,
        headers={"Content-Type": "application/json"}
    )
    
    return response.json()

def unified_vector_search(collection_id, queries, top_k=5):
    """Execute a unified vector search"""
    payload = {
        "collection_id": collection_id,
        "queries": queries,
        "top_k": top_k,
        "include_fields": {
            "vector": False,
            "metadata": True,
            "score": True,
            "rank": True
        }
    }
    
    response = requests.post(
        f"{BASE_URL}/api/v1/vector/search",
        json=payload,
        headers={"Content-Type": "application/json"}
    )
    
    return response.json()

def main():
    print("🚀 ProximaDB Unified REST API Demo")
    print("=" * 50)
    
    # 1. Create a collection using unified endpoint
    print("\n1. Creating collection...")
    create_response = unified_collection_operation(
        operation="create",
        config={
            "name": "unified_demo",
            "dimension": 128,
            "distance_metric": "cosine",
            "storage_engine": "viper",
            "primary_indexing_algorithm": "hnsw",
            "description": "Demo collection for unified API",
            "tags": ["demo", "unified-api"],
            "owner": "demo-user"
        }
    )
    
    if create_response["success"]:
        collection_id = create_response["collection"]["id"]
        print(f"✅ Created collection: {collection_id}")
        print(f"   Name: {create_response['collection']['config']['name']}")
        print(f"   Dimension: {create_response['collection']['config']['dimension']}")
    else:
        print(f"❌ Failed to create collection: {create_response['error_message']}")
        return
    
    # 2. Insert vectors using unified batch endpoint
    print("\n2. Inserting vectors...")
    import numpy as np
    
    vectors_data = []
    for i in range(5):
        vector = np.random.rand(128).tolist()
        vectors_data.append({
            "id": f"vec_{i}",
            "vector": vector,
            "metadata": {
                "category": f"category_{i % 3}",
                "score": float(i * 10),
                "tags": [f"tag_{i}", "demo"]
            }
        })
    
    batch_response = unified_vector_batch(collection_id, vectors_data)
    
    if batch_response["success"]:
        print(f"✅ Inserted {batch_response['metrics']['successful_count']} vectors")
        print(f"   Processing time: {batch_response['metrics']['processing_time_us']} μs")
    else:
        print(f"❌ Failed to insert vectors: {batch_response['error_message']}")
    
    # Give some time for indexing
    time.sleep(1)
    
    # 3. Search vectors using unified search endpoint
    print("\n3. Searching vectors...")
    query_vector = np.random.rand(128).tolist()
    
    search_response = unified_vector_search(
        collection_id,
        queries=[{
            "vector": query_vector,
            "metadata_filter": {
                "conditions": [{
                    "field_name": "category",
                    "operation": "equals",
                    "value": "category_1"
                }],
                "operator": "and"
            }
        }],
        top_k=3
    )
    
    if search_response["success"]:
        print(f"✅ Search completed in {search_response['metrics']['processing_time_us']} μs")
        if search_response["results"]:
            print(f"   Found {len(search_response['results'])} results:")
            for i, result in enumerate(search_response["results"]):
                print(f"   {i+1}. ID: {result['id']}, Score: {result['score']:.4f}")
                if result.get("metadata"):
                    print(f"      Metadata: {json.dumps(result['metadata'], indent=8)}")
    else:
        print(f"❌ Search failed: {search_response['error_message']}")
    
    # 4. List collections
    print("\n4. Listing collections...")
    list_response = unified_collection_operation(operation="list")
    
    if list_response["success"]:
        print(f"✅ Found {list_response['affected_count']} collections:")
        for col in list_response["collections"]:
            print(f"   - {col['config']['name']} (ID: {col['id']})")
            print(f"     Vectors: {col['stats']['vector_count']}")
    
    # 5. Get specific collection
    print("\n5. Getting collection details...")
    get_response = unified_collection_operation(
        operation="get",
        collection_id=collection_id
    )
    
    if get_response["success"]:
        col = get_response["collection"]
        print(f"✅ Collection details:")
        print(f"   Name: {col['config']['name']}")
        print(f"   Storage: {col['config']['storage_engine']}")
        print(f"   Index: {col['config']['primary_indexing_algorithm']}")
        print(f"   Stats: {col['stats']['vector_count']} vectors")
    
    # 6. Update collection metadata
    print("\n6. Updating collection...")
    update_response = unified_collection_operation(
        operation="update",
        collection_id=collection_id,
        config={
            "name": "unified_demo",  # Name must match
            "dimension": 128,        # Dimension must match
            "distance_metric": "cosine",
            "storage_engine": "viper",
            "primary_indexing_algorithm": "hnsw",
            "description": "Updated demo collection",
            "tags": ["demo", "unified-api", "updated"],
            "owner": "updated-user"
        }
    )
    
    if update_response["success"]:
        print(f"✅ Updated collection successfully")
    
    # 7. Delete collection
    print("\n7. Deleting collection...")
    delete_response = unified_collection_operation(
        operation="delete",
        collection_id=collection_id
    )
    
    if delete_response["success"]:
        print(f"✅ Deleted collection successfully")
    
    print("\n✨ Demo completed!")

if __name__ == "__main__":
    main()