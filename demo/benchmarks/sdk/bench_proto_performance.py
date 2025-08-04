def test_proto_performance():
    """Compare proto serialization performance"""
    
    print("\n🚀 Testing Proto Performance")
    print("=" * 60)
    
    # Create test data
    num_vectors = 1000
    dimension = 256
    
    # Time proto serialization
    start_time = time.time()
    
    batch = pb2.VectorBatchRequest()
    batch.collection_id = "perf_test"
    
    for i in range(num_vectors):
        record = batch.vectors.add()
        record.id = f"perf_{i:06d}"
        record.vector.extend(np.random.rand(dimension).tolist())
        
        # Add metadata items
        index_item = record.metadata.add()
        index_item.key = "index"
        index_item.string_value = str(i)
        
        timestamp_item = record.metadata.add()
        timestamp_item.key = "timestamp"
        timestamp_item.string_value = str(time.time())
    
    proto_create_time = time.time() - start_time
    
    # Serialize
    start_time = time.time()
    serialized = batch.SerializeToString()
    proto_serialize_time = time.time() - start_time
    
    # Deserialize
    start_time = time.time()
    deserialized = pb2.VectorBatchRequest()
    deserialized.ParseFromString(serialized)
    proto_deserialize_time = time.time() - start_time
    
    print(f"\nVectors: {num_vectors}, Dimension: {dimension}")
    print(f"Proto creation: {proto_create_time:.3f}s")
    print(f"Proto serialization: {proto_serialize_time:.3f}s")
    print(f"Proto deserialization: {proto_deserialize_time:.3f}s")
    print(f"Serialized size: {len(serialized):,} bytes")
    print(f"Bytes per vector: {len(serialized) / num_vectors:.0f}")


if __name__ == "__main__":
    test_proto_serialization()
    test_proto_performance()