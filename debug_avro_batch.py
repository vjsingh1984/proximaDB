#!/usr/bin/env python3
"""
Debug script to test Avro batch serialization/deserialization
"""

import io
import json
import avro.schema
import avro.io

# The exact schema from the server
VECTOR_BATCH_SCHEMA_V1 = '''
{
  "type": "record",
  "name": "VectorBatch",
  "namespace": "ai.proximadb.vectors",
  "fields": [
    {"name": "vectors", "type": {
      "type": "array",
      "items": {
        "type": "record",
        "name": "Vector",
        "fields": [
          {"name": "id", "type": "string"},
          {"name": "vector", "type": {"type": "array", "items": "float"}},
          {"name": "metadata", "type": ["null", {"type": "map", "values": "string"}], "default": null},
          {"name": "timestamp", "type": ["null", "long"], "default": null}
        ]
      }
    }}
  ]
}
'''

def test_avro_serialization():
    """Test Avro batch serialization with multiple vectors"""
    
    # Create test data with multiple vectors
    test_vectors = [
        {
            "id": "vec1",
            "vector": [0.1, 0.2, 0.3, 0.4],
            "metadata": {"type": "test", "index": "0"},
            "timestamp": 1234567890000
        },
        {
            "id": "vec2", 
            "vector": [0.5, 0.6, 0.7, 0.8],
            "metadata": {"type": "test", "index": "1"},
            "timestamp": 1234567891000
        },
        {
            "id": "vec3",
            "vector": [0.9, 1.0, 1.1, 1.2],
            "metadata": {"type": "test", "index": "2"},
            "timestamp": 1234567892000
        }
    ]
    
    batch = {
        "vectors": test_vectors
    }
    
    print(f"🧪 Testing Avro batch serialization with {len(test_vectors)} vectors")
    print(f"Input batch: {json.dumps(batch, indent=2)}")
    
    # Parse schema
    schema = avro.schema.parse(VECTOR_BATCH_SCHEMA_V1)
    print(f"✅ Schema parsed successfully")
    
    # Serialize
    bytes_writer = io.BytesIO()
    encoder = avro.io.BinaryEncoder(bytes_writer)
    datum_writer = avro.io.DatumWriter(schema)
    datum_writer.write(batch, encoder)
    
    avro_bytes = bytes_writer.getvalue()
    print(f"✅ Serialized to {len(avro_bytes)} bytes")
    
    # Deserialize
    bytes_reader = io.BytesIO(avro_bytes)
    decoder = avro.io.BinaryDecoder(bytes_reader)
    datum_reader = avro.io.DatumReader(schema)
    deserialized = datum_reader.read(decoder)
    
    print(f"✅ Deserialized successfully")
    print(f"Deserialized batch: {json.dumps(deserialized, indent=2)}")
    
    # Verify
    input_count = len(batch["vectors"])
    output_count = len(deserialized["vectors"])
    
    print(f"\n📊 Results:")
    print(f"   Input vectors: {input_count}")
    print(f"   Output vectors: {output_count}")
    print(f"   Match: {'✅' if input_count == output_count else '❌'}")
    
    # Verify individual vectors
    if input_count == output_count:
        for i, (input_vec, output_vec) in enumerate(zip(batch["vectors"], deserialized["vectors"])):
            print(f"   Vector {i+1}: ID={output_vec['id']}, Vector_len={len(output_vec['vector'])}, Metadata={output_vec['metadata']}")
    
    return avro_bytes

if __name__ == "__main__":
    test_avro_serialization()