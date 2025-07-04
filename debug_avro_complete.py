#!/usr/bin/env python3
"""
Complete debug script to test Avro serialization match with server
"""

import io
import json
import avro.schema
import avro.io
import binascii

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

def create_test_vectors(count=3):
    """Create test vectors like the client would"""
    vectors = []
    for i in range(count):
        vectors.append({
            "id": f"test_vec_{i}",
            "vector": [float(j) * 0.1 for j in range(i*4, (i+1)*4)],  # Different values for each vector
            "metadata": {"type": "test", "index": str(i), "batch": "debug"},
            "timestamp": 1234567890000 + i * 1000
        })
    return vectors

def serialize_like_client(vectors):
    """Serialize exactly like the Python client does"""
    print(f"🔧 Serializing {len(vectors)} vectors like the client...")
    
    # Parse schema
    schema = avro.schema.parse(VECTOR_BATCH_SCHEMA_V1)
    
    # Convert to Avro format (like the client does)
    avro_vectors = []
    for i, vec in enumerate(vectors):
        # Convert metadata to map of strings (Avro requirement)
        metadata = None
        if vec.get('metadata'):
            metadata = {k: str(v) for k, v in vec['metadata'].items()}
        
        # Get timestamp
        timestamp = vec.get('timestamp')
        if timestamp is None:
            timestamp = int(time.time() * 1000)
        
        # Create Avro vector record
        avro_vector = {
            "id": str(vec['id']),
            "vector": [float(x) for x in vec['vector']],
            "metadata": metadata,
            "timestamp": timestamp
        }
        avro_vectors.append(avro_vector)
    
    # Create batch structure
    batch = {
        "vectors": avro_vectors
    }
    
    # Serialize to Avro binary using datum writer
    bytes_writer = io.BytesIO()
    encoder = avro.io.BinaryEncoder(bytes_writer)
    datum_writer = avro.io.DatumWriter(schema)
    datum_writer.write(batch, encoder)
    
    avro_bytes = bytes_writer.getvalue()
    print(f"✅ Serialized to {len(avro_bytes)} bytes")
    
    return avro_bytes, batch

def deserialize_like_server(avro_bytes):
    """Deserialize exactly like the Rust server does"""
    print(f"🔧 Deserializing {len(avro_bytes)} bytes like the server...")
    
    # Parse schema
    schema = avro.schema.parse(VECTOR_BATCH_SCHEMA_V1)
    
    # Deserialize from Avro binary datum (like the server does)
    reader = io.BytesIO(avro_bytes)
    decoder = avro.io.BinaryDecoder(reader)
    datum_reader = avro.io.DatumReader(schema)
    avro_value = datum_reader.read(decoder)
    
    print(f"✅ Deserialized Avro value")
    
    # Extract vectors (like the server does)
    vectors = avro_value.get('vectors', [])
    print(f"   Found {len(vectors)} vectors in batch")
    
    # Process each vector (like the server does)
    vector_records = []
    for i, avro_vector in enumerate(vectors):
        record = {
            "id": avro_vector['id'],
            "vector": avro_vector['vector'],
            "metadata": avro_vector.get('metadata', {}),
            "timestamp": avro_vector.get('timestamp')
        }
        vector_records.append(record)
        print(f"   Vector {i+1}: ID={record['id']}, Vector_len={len(record['vector'])}")
    
    return vector_records

def test_complete_flow():
    """Test the complete serialization/deserialization flow"""
    print("🧪 Testing Complete Avro Flow (Client → Server)")
    print("=" * 60)
    
    # Create test vectors
    test_vectors = create_test_vectors(5)  # Test with 5 vectors
    print(f"📝 Created {len(test_vectors)} test vectors")
    
    # Serialize like client
    avro_bytes, original_batch = serialize_like_client(test_vectors)
    
    # Print hex dump for debugging
    print(f"\n📋 Hex dump of Avro bytes:")
    hex_dump = binascii.hexlify(avro_bytes).decode()
    for i in range(0, len(hex_dump), 32):
        print(f"   {hex_dump[i:i+32]}")
    
    # Deserialize like server
    vector_records = deserialize_like_server(avro_bytes)
    
    # Compare results
    print(f"\n📊 Comparison:")
    print(f"   Input vectors: {len(test_vectors)}")
    print(f"   Serialized vectors: {len(original_batch['vectors'])}")
    print(f"   Deserialized vectors: {len(vector_records)}")
    
    # Check if all vectors are preserved
    if len(test_vectors) == len(vector_records):
        print("✅ Vector count matches!")
        
        # Check individual vectors
        for i, (original, deserialized) in enumerate(zip(test_vectors, vector_records)):
            print(f"   Vector {i+1}: {original['id']} → {deserialized['id']}")
            if original['id'] != deserialized['id']:
                print(f"      ❌ ID mismatch!")
            if len(original['vector']) != len(deserialized['vector']):
                print(f"      ❌ Vector length mismatch!")
    else:
        print("❌ Vector count mismatch!")
    
    # Save the Avro bytes for further testing
    with open('/tmp/debug_avro_batch.bin', 'wb') as f:
        f.write(avro_bytes)
    print(f"💾 Saved Avro bytes to /tmp/debug_avro_batch.bin")
    
    return avro_bytes

if __name__ == "__main__":
    test_complete_flow()