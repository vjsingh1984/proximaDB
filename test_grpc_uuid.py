#!/usr/bin/env python3
"""Test gRPC UUID operations directly."""

import grpc
import sys
import os

# Add the generated protobuf files to the path
sys.path.insert(0, '/workspace/clients/python/src')

try:
    from proximadb import proximadb_pb2, proximadb_pb2_grpc
except ImportError:
    print("❌ Could not import gRPC modules. Protobuf files may not be generated.")
    exit(1)

def test_grpc_uuid_operations():
    """Test UUID operations via gRPC directly."""
    print("Testing gRPC UUID Operations")
    print("=" * 35)
    
    # Connect to gRPC server
    channel = grpc.insecure_channel('localhost:5679')
    stub = proximadb_pb2_grpc.ProximaDBStub(channel)
    
    # Test 1: Create collection
    print("1️⃣ Creating collection via gRPC...")
    import time
    collection_name = f"grpc-uuid-test-{int(time.time())}"
    
    create_request = proximadb_pb2.CollectionRequest(
        operation=proximadb_pb2.CollectionOperation.COLLECTION_CREATE,
        collection_id=collection_name,
        config=proximadb_pb2.CollectionConfig(
            name=collection_name,
            dimension=384,
            distance_metric=proximadb_pb2.DistanceMetric.COSINE,
            indexing_algorithm=proximadb_pb2.IndexingAlgorithm.HNSW
        )
    )
    
    try:
        create_response = stub.ManageCollection(create_request)
        if create_response.success and create_response.collection:
            collection_uuid = create_response.collection.id
            print(f"✅ Created collection: {collection_name}")
            print(f"✅ Collection UUID: {collection_uuid}")
        else:
            print(f"❌ Failed to create collection: {create_response.error_message}")
            return False
    except Exception as e:
        print(f"❌ Error creating collection: {e}")
        return False
    
    # Test 2: Get collection by UUID
    print(f"\n2️⃣ Getting collection by UUID via gRPC...")
    get_request = proximadb_pb2.CollectionRequest(
        operation=proximadb_pb2.CollectionOperation.COLLECTION_GET,
        collection_id=collection_uuid  # Using UUID instead of name
    )
    
    try:
        get_response = stub.ManageCollection(get_request)
        if get_response.success and get_response.collection:
            retrieved_collection = get_response.collection
            print(f"✅ Retrieved collection by UUID")
            print(f"   Name: {retrieved_collection.config.name}")
            print(f"   UUID: {retrieved_collection.id}")
            
            # Verify it's the same collection
            if retrieved_collection.config.name == collection_name and retrieved_collection.id == collection_uuid:
                print(f"✅ Collection data matches perfectly")
            else:
                print(f"❌ Collection data mismatch")
                return False
        else:
            print(f"❌ Failed to get collection by UUID: {get_response.error_message}")
            return False
    except Exception as e:
        print(f"❌ Error getting collection by UUID: {e}")
        return False
    
    # Test 3: Delete collection by UUID
    print(f"\n3️⃣ Deleting collection by UUID via gRPC...")
    delete_request = proximadb_pb2.CollectionRequest(
        operation=proximadb_pb2.CollectionOperation.COLLECTION_DELETE,
        collection_id=collection_uuid  # Using UUID instead of name
    )
    
    try:
        delete_response = stub.ManageCollection(delete_request)
        if delete_response.success:
            print(f"✅ Deleted collection by UUID")
        else:
            print(f"❌ Failed to delete collection by UUID: {delete_response.error_message}")
            return False
    except Exception as e:
        print(f"❌ Error deleting collection by UUID: {e}")
        return False
    
    # Test 4: Verify deletion
    print(f"\n4️⃣ Verifying deletion...")
    verify_request = proximadb_pb2.CollectionRequest(
        operation=proximadb_pb2.CollectionOperation.COLLECTION_GET,
        collection_id=collection_uuid
    )
    
    try:
        verify_response = stub.ManageCollection(verify_request)
        if not verify_response.success:
            print(f"✅ Confirmed collection was deleted (expected failure)")
        else:
            print(f"❌ Collection should have been deleted but still exists")
            return False
    except Exception as e:
        print(f"✅ Confirmed collection was deleted (gRPC error: {type(e).__name__})")
    
    print(f"\n🎉 All gRPC UUID operations PASSED!")
    return True

if __name__ == "__main__":
    success = test_grpc_uuid_operations()
    exit(0 if success else 1)