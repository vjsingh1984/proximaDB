#!/usr/bin/env python3
"""Test collection update functionality for both REST and gRPC"""

import sys
import time
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb import ProximaDBClient, CollectionConfig

def test_rest_collection_update():
    """Test collection update via REST API"""
    print("\n=== Testing REST Collection Update ===")
    
    client = ProximaDBClient("localhost:5678")  # REST port
    collection_name = f"test_update_rest_{int(time.time())}"
    
    # Create collection
    print(f"Creating collection: {collection_name}")
    config = CollectionConfig(dimension=128, distance_metric="cosine")
    collection = client.create_collection(collection_name, config=config)
    print(f"Created collection: {collection.name}")
    
    # Update collection metadata
    print("\nUpdating collection metadata...")
    updates = {
        "description": "Test collection for update operations",
        "tags": ["test", "update", "rest"],
        "owner": "test_user",
        "config": {"max_vectors": 10000, "index_type": "hnsw"}
    }
    
    try:
        updated_collection = client.update_collection(collection_name, updates)
        print("✅ Collection updated successfully!")
        
        # Verify the updates
        collection = client.get_collection(collection_name)
        print(f"\nVerifying updates:")
        print(f"  Description: {getattr(collection, 'description', 'N/A')}")
        print(f"  Tags: {getattr(collection, 'tags', 'N/A')}")
        print(f"  Owner: {getattr(collection, 'owner', 'N/A')}")
        
    except Exception as e:
        print(f"❌ Update failed: {e}")
    
    # Try to update immutable fields (should fail)
    print("\nTrying to update immutable field (dimension)...")
    try:
        client.update_collection(collection_name, {"dimension": 256})
        print("❌ Should have failed!")
    except Exception as e:
        print(f"✅ Correctly rejected: {e}")
    
    # Cleanup
    print(f"\nCleaning up collection: {collection_name}")
    client.delete_collection(collection_name)
    print("✅ Cleanup complete")


def test_grpc_collection_update():
    """Test collection update via gRPC API"""
    print("\n=== Testing gRPC Collection Update ===")
    
    client = ProximaDBClient("localhost:5679")  # gRPC port
    collection_name = f"test_update_grpc_{int(time.time())}"
    
    # Create collection
    print(f"Creating collection: {collection_name}")
    config = CollectionConfig(dimension=128, distance_metric="euclidean")
    collection = client.create_collection(collection_name, config=config)
    print(f"Created collection: {collection.name}")
    
    # Update collection metadata
    print("\nUpdating collection metadata...")
    updates = {
        "description": "Test collection for gRPC update",
        "tags": ["test", "update", "grpc"],
        "owner": "grpc_user",
        "config": {"ef_construction": 200, "m": 16}
    }
    
    try:
        updated_collection = client.update_collection(collection_name, updates)
        print("✅ Collection updated successfully!")
        
        # Verify the updates
        collection = client.get_collection(collection_name)
        print(f"\nVerifying updates:")
        print(f"  Description: {getattr(collection, 'description', 'N/A')}")
        print(f"  Tags: {getattr(collection, 'tags', 'N/A')}")
        print(f"  Owner: {getattr(collection, 'owner', 'N/A')}")
        
    except Exception as e:
        print(f"❌ Update failed: {e}")
    
    # Try to update immutable fields (should fail)
    print("\nTrying to update immutable field (distance_metric)...")
    try:
        client.update_collection(collection_name, {"distance_metric": "cosine"})
        print("❌ Should have failed!")
    except Exception as e:
        print(f"✅ Correctly rejected: {e}")
    
    # Cleanup
    print(f"\nCleaning up collection: {collection_name}")
    client.delete_collection(collection_name)
    print("✅ Cleanup complete")


def main():
    """Run all tests"""
    print("Testing ProximaDB Collection Update Functionality")
    print("=" * 50)
    
    try:
        test_rest_collection_update()
    except Exception as e:
        print(f"\n❌ REST test failed with error: {e}")
    
    try:
        test_grpc_collection_update()
    except Exception as e:
        print(f"\n❌ gRPC test failed with error: {e}")
    
    print("\n" + "=" * 50)
    print("Tests completed!")


if __name__ == "__main__":
    main()