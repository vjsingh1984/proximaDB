#!/usr/bin/env python3
"""
OBSOLETE: Debug Avro Serialization - ProximaDB now uses Protocol Buffers as default
This file is kept for historical reference only. Proto is the default serialization format.
"""

import pytest

pytestmark = pytest.mark.skip(reason="Avro serialization is obsolete, Proto is now the default")

import time
import numpy as np
import avro.schema
import avro.io
from io import BytesIO
from proximadb import ProximaDBClient, Protocol


def test_proto_serialization():
    """Test Proto serialization with ProximaDB client"""
    
    print("🔍 Testing Proto Serialization")
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
    
    # Now test with gRPC client (Proto-first architecture)
    print("\n\n🔍 Testing with gRPC Client (Proto-first)")
    print("=" * 60)
    
    client = ProximaDBClient(url="localhost", protocol=Protocol.GRPC)
    
    # Create collection
    collection_name = f"proto_test_{int(time.time())}"
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
        # Create VectorRecord objects for proto-first architecture
        from proximadb.models import VectorRecord
        
        vector_records = [
            VectorRecord(
                id="test_vec_1",
                vector=[0.1, 0.2, 0.3, 0.4],
                metadata={"test": "value"}
            )
        ]
        
        print(f"✅ Created {len(vector_records)} VectorRecord objects")
        print(f"   ID: {vector_records[0].id}")
        print(f"   Vector: {vector_records[0].vector}")
        print(f"   Metadata: {vector_records[0].metadata}")
        
        # Now try the actual insert using VectorRecord objects
        response = client.insert_vectors(collection_name, vector_records)
        
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
    test_proto_serialization()