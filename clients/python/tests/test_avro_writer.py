#!/usr/bin/env python3
"""
Test Avro serialization using Writer (like the server)
"""

import time
import avro.schema
import avro.io
import avro.datafile
from io import BytesIO


def test_avro_writer():
    """Test using Avro Writer like the server does"""
    
    print("🔍 Testing Avro Writer Serialization")
    print("=" * 60)
    
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
    
    schema = avro.schema.parse(schema_str)
    
    # Test 1: Using DatumWriter (current approach)
    print("\n1️⃣ Using DatumWriter:")
    
    test_data = {
        "vectors": [{
            "id": "test1",
            "collection_id": "test_coll",
            "vector": [0.1, 0.2, 0.3, 0.4],
            "metadata": {"key": "value"},
            "timestamp": int(time.time()),
            "expires_at": None,
            "version": 1
        }]
    }
    
    try:
        bytes_writer = BytesIO()
        encoder = avro.io.BinaryEncoder(bytes_writer)
        writer = avro.io.DatumWriter(schema)
        writer.write(test_data, encoder)
        
        datum_bytes = bytes_writer.getvalue()
        print(f"✅ DatumWriter serialization: {len(datum_bytes)} bytes")
        print(f"   First 20 bytes: {datum_bytes[:20].hex()}")
        
        # Try to read it back
        bytes_reader = BytesIO(datum_bytes)
        decoder = avro.io.BinaryDecoder(bytes_reader)
        reader = avro.io.DatumReader(schema)
        result = reader.read(decoder)
        print(f"✅ DatumWriter data deserializes correctly")
        
    except Exception as e:
        print(f"❌ DatumWriter failed: {e}")
    
    # Test 2: Using DataFileWriter (like apache_avro Writer)
    print("\n2️⃣ Using DataFileWriter:")
    
    try:
        bytes_writer = BytesIO()
        writer = avro.datafile.DataFileWriter(bytes_writer, avro.io.DatumWriter(), schema)
        writer.append(test_data)
        writer.flush()
        
        file_bytes = bytes_writer.getvalue()
        print(f"✅ DataFileWriter serialization: {len(file_bytes)} bytes")
        print(f"   First 20 bytes: {file_bytes[:20].hex()}")
        
        # The DataFileWriter includes headers, so it's much larger
        writer.close()
        
    except Exception as e:
        print(f"❌ DataFileWriter failed: {e}")
    
    # Test 3: Check for differences in Union encoding
    print("\n3️⃣ Testing Union encoding:")
    
    # Server code shows: Value::Union(1, Box::new(Value::String(id)))
    # This suggests index-based union encoding
    
    test_with_null_id = {
        "vectors": [{
            "id": None,  # This should be Union index 0
            "collection_id": "test_coll",
            "vector": [0.1, 0.2, 0.3, 0.4],
            "metadata": None,  # Also Union index 0
            "timestamp": int(time.time()),
            "expires_at": None,
            "version": 1
        }]
    }
    
    try:
        bytes_writer = BytesIO()
        encoder = avro.io.BinaryEncoder(bytes_writer)
        writer = avro.io.DatumWriter(schema)
        writer.write(test_with_null_id, encoder)
        
        null_bytes = bytes_writer.getvalue()
        print(f"✅ Null ID serialization: {len(null_bytes)} bytes")
        
        # Compare with non-null
        test_with_id = test_with_null_id.copy()
        test_with_id["vectors"][0]["id"] = "test1"
        
        bytes_writer = BytesIO()
        encoder = avro.io.BinaryEncoder(bytes_writer)
        writer.write(test_with_id, encoder)
        
        id_bytes = bytes_writer.getvalue()
        print(f"✅ With ID serialization: {len(id_bytes)} bytes")
        print(f"   Difference: {len(id_bytes) - len(null_bytes)} bytes")
        
    except Exception as e:
        print(f"❌ Union test failed: {e}")
    
    print("\n" + "=" * 60)
    print("Summary: The Python avro library should work the same way.")
    print("The issue might be in the exact data format or field values.")


if __name__ == "__main__":
    test_avro_writer()