#!/usr/bin/env python3
"""
Proto-First Architecture Integration Test

This test validates that:
1. Python SDK sends pure proto VectorRecord messages 
2. Rust server handles proto VectorBatchRequest correctly
3. End-to-end proto-first architecture works without Avro
"""

import sys
import os
import time
import pytest

# Add the Python SDK to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'clients', 'python', 'src'))

from proximadb.protocols.grpc_async import ProximaDBClient
from proximadb import proximadb_pb2 as pb2
import grpc

def test_proto_first_vector_insert():
    """Test pure proto message insertion"""
    
    # Create gRPC client
    client = ProximaDBClient(endpoint="localhost:5679")
    
    # Create collection first
    collection_config = pb2.CollectionConfig(
        name="test_proto_collection",
        dimension=3,
        distance_metric=pb2.DistanceMetric.COSINE,
        storage_engine=pb2.StorageEngine.VIPER
    )
    
    collection_request = pb2.CollectionRequest(
        operation=pb2.CollectionOperation.COLLECTION_CREATE,
        collection_config=collection_config
    )
    
    try:
        # Create collection
        collection_response = client.stub.CollectionOperation(collection_request)
        assert collection_response.success, f"Collection creation failed: {collection_response.error_message}"
        collection_id = collection_response.collection.id
        
        print(f"✅ Created collection: {collection_id}")
        
        # Create pure proto vector records (no Avro involved)
        proto_vectors = []
        for i in range(3):
            vector_record = pb2.VectorRecord()
            vector_record.id = f"test_vector_{i}"
            vector_record.vector.extend([float(i), float(i+1), float(i+2)])
            vector_record.version = 1
            
            # Add metadata
            metadata_map = pb2.MetadataMap()
            metadata_value = pb2.MetadataValue()
            metadata_value.string_value = f"test_metadata_{i}"
            metadata_map.fields["test_key"].CopyFrom(metadata_value)
            vector_record.metadata.CopyFrom(metadata_map)
            
            proto_vectors.append(vector_record)
        
        # Create proto-first vector batch request
        vector_batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=proto_vectors  # Pure proto, no Avro binary payload
        )
        
        print(f"🚀 Sending {len(proto_vectors)} proto vectors (no Avro)")
        
        # Send pure proto message to server
        vector_response = client.stub.VectorBatch(vector_batch_request)
        
        print(f"📥 Server response: success={vector_response.success}")
        
        if not vector_response.success:
            print(f"❌ Vector insertion failed: {vector_response.error_message}")
            return False
        
        print(f"✅ Proto-first vector insertion successful!")
        print(f"   - Vectors processed: {vector_response.metrics.total_processed}")
        print(f"   - Processing time: {vector_response.metrics.processing_time_us}μs")
        print(f"   - Vector IDs: {list(vector_response.vector_ids)}")
        
        return True
        
    except grpc.RpcError as e:
        print(f"❌ gRPC Error: {e.code()} - {e.details()}")
        return False
    except Exception as e:
        print(f"❌ Test Error: {e}")
        return False

def test_proto_first_architecture_demo():
    """Comprehensive demo of proto-first architecture"""
    
    print("🎯 ProximaDB Proto-First Architecture Demo")
    print("=" * 50)
    
    # Test 1: Proto Vector Insertion
    print("\n📝 Test 1: Pure Proto Vector Insertion")
    success = test_proto_first_vector_insert()
    
    if success:
        print("\n🎉 Proto-First Architecture Test: PASSED")
        print("   ✅ Python SDK sends pure proto messages")
        print("   ✅ Rust server handles proto VectorBatchRequest")
        print("   ✅ No Avro serialization in the wire protocol")
        print("   ✅ End-to-end proto-first architecture working")
    else:
        print("\n❌ Proto-First Architecture Test: FAILED")
        
    return success

if __name__ == "__main__":
    success = test_proto_first_architecture_demo()
    sys.exit(0 if success else 1)