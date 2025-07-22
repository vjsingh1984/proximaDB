#!/usr/bin/env python3
"""
Debug Avro Serialization
"""

import time
import numpy as np
import avro.schema
import avro.io
from io import BytesIO
from proximadb import ProximaDBClient, Protocol


def test_avro_serialization():
    """Test Avro serialization independently"""
    
    print("🔍 Testing Avro Serialization")
    print("=" * 60)
    
    # The server schema
    schema_str = '''
    {
      "type": "record",
      "name": "WalVectorBatch",
      "namespace": "ai.proximadb.wal",
      "fields": [
        {"name": "vectors", "type": {
          "type": "array", 
          "items": {
            "type": "record",
            "name": "VectorRecord", 
            "fields": [
              {"name": "id", "type": ["null", "string"], "default": null},
              {"name": "collection_id", "type": "string"},
              {"name": "vector", "type": {"type": "array", "items": "float"}},
              {"name": "metadata", "type": ["null", {"type": "map", "values": "string"}], "default": null},
              {"name": "timestamp", "type": "int"},
              {"name": "expires_at", "type": ["null", "int"], "default": null},
              {"name": "version", "type": "int"}
            ]
          }
        }}
      ]
    }
    '''
    
    # Parse schema
    schema = avro.schema.parse(schema_str)
    print("✅ Schema parsed successfully")
    
    # Create test data
    test_vector = {
        "id": "test_vec_1",
        "collection_id": "test_collection",
        "vector": [0.1, 0.2, 0.3, 0.4],
        "metadata": {"key": "value"},
        "timestamp": int(time.time()),  # Unix timestamp in seconds
        "expires_at": None,
        "version": 1
    }
    
    batch = {"vectors": [test_vector]}
    
    print("\n📊 Test data:")
    print(f"   ID: {test_vector['id']}")
    print(f"   Collection: {test_vector['collection_id']}")
    print(f"   Vector length: {len(test_vector['vector'])}")
    print(f"   Timestamp: {test_vector['timestamp']} (type: {type(test_vector['timestamp'])})")
    
    # Serialize
    try:
        bytes_writer = BytesIO()
        encoder = avro.io.BinaryEncoder(bytes_writer)
        writer = avro.io.DatumWriter(schema)
        writer.write(batch, encoder)
        
        avro_bytes = bytes_writer.getvalue()
        print(f"\n✅ Serialization successful: {len(avro_bytes)} bytes")
        
        # Try to deserialize
        bytes_reader = BytesIO(avro_bytes)
        decoder = avro.io.BinaryDecoder(bytes_reader)
        reader = avro.io.DatumReader(schema)
        result = reader.read(decoder)
        
        print(f"✅ Deserialization successful")
        print(f"   Vectors count: {len(result['vectors'])}")
        
    except Exception as e:
        print(f"\n❌ Serialization failed: {e}")
        import traceback
        traceback.print_exc()
    
    # Now test with gRPC client
    print("\n\n🔍 Testing with gRPC Client")
    print("=" * 60)
    
    client = ProximaDBClient(url="localhost", protocol=Protocol.GRPC)
    
    # Create collection
    collection_name = f"avro_test_{int(time.time())}"
    try:
        collection = client.create_collection(
            name=collection_name,
            dimension=4,
            distance_metric="cosine",
            storage_engine="viper"
        )
        print(f"✅ Collection created: {collection_name}")
    except Exception as e:
        print(f"❌ Collection creation failed: {e}")
        return
    
    # Try inserting a single vector
    print("\n📤 Testing vector insertion...")
    
    try:
        vectors = [{
            "id": "test_vec_1",
            "vector": [0.1, 0.2, 0.3, 0.4],
            "metadata": {"test": "value"}
        }]
        
        # Modern proto-first approach: create proto vector batch
        try:
            proto_vectors = client._create_proto_vector_batch(vectors, collection_name)
            print(f"✅ Proto vectors created: {len(proto_vectors)} records")
            
            # Inspect the proto structure
            if proto_vectors:
                first_vec = proto_vectors[0]
                print(f"   ID: {first_vec.id}")
                print(f"   Collection: {first_vec.collection_id}")
                print(f"   Vector length: {len(first_vec.vector)}")
                print(f"   Timestamp: {first_vec.timestamp}")
                print(f"   Proto type: {type(first_vec)}")
                
        except Exception as e:
            print(f"❌ Exception during proto creation: {e}")
        
        # Now try the actual insert
        response = client.insert_vectors(collection_name, vectors)
        
        if response.success:
            print(f"\n✅ Vector insertion successful!")
        else:
            print(f"\n❌ Vector insertion failed: {response.error_message}")
            
    except Exception as e:
        print(f"\n❌ Exception during insertion: {e}")
        import traceback
        traceback.print_exc()
    
    # Cleanup
    try:
        client.delete_collection(collection_name)
        print(f"\n🧹 Cleaned up collection: {collection_name}")
    except:
        pass


if __name__ == "__main__":
    test_avro_serialization()