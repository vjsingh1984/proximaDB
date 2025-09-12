#!/usr/bin/env python3
"""
Test Proto serialization for ProximaDB (default choice replacing Avro)
"""

import time
import numpy as np
import pytest
from proximadb import proximadb_pb2 as pb2
from google.protobuf import json_format


def test_proto_serialization():
    """Test using Proto serialization like the server does"""
    
    print("🔍 Testing Proto Serialization")
    print("=" * 60)
    
    # Test 1: Create a VectorRecord
    print("\n1️⃣ Creating VectorRecord:")
    
    vector_record = pb2.VectorRecord()
    vector_record.id = "test_vector_001"
    vector_record.vector.extend(np.random.rand(128).tolist())
    
    # Add metadata - metadata is a repeated field of MetadataItem
    metadata_items = {
        "category": "electronics",
        "price": "99.99", 
        "in_stock": "true"
    }
    for key, value in metadata_items.items():
        metadata_item = vector_record.metadata.add()
        metadata_item.key = key
        metadata_item.string_value = value
    
    print(f"Vector ID: {vector_record.id}")
    print(f"Vector dimensions: {len(vector_record.vector)}")
    print(f"Metadata: {[(item.key, item.string_value) for item in vector_record.metadata]}")
    
    # Test 2: Serialize to bytes
    print("\n2️⃣ Serializing to bytes:")
    
    serialized = vector_record.SerializeToString()
    print(f"Serialized size: {len(serialized)} bytes")
    
    # Test 3: Deserialize back
    print("\n3️⃣ Deserializing from bytes:")
    
    deserialized = pb2.VectorRecord()
    deserialized.ParseFromString(serialized)
    
    print(f"Deserialized ID: {deserialized.id}")
    print(f"Deserialized vector dims: {len(deserialized.vector)}")
    print(f"Deserialized metadata: {[(item.key, item.string_value) for item in deserialized.metadata]}")
    
    # Test 4: Batch serialization
    print("\n4️⃣ Batch serialization:")
    
    batch = pb2.VectorBatchRequest()
    batch.collection_id = "test_collection"
    
    for i in range(10):
        record = batch.vectors.add()
        record.id = f"batch_vector_{i:03d}"
        record.vector.extend(np.random.rand(128).tolist())
        
        # Add metadata items
        index_item = record.metadata.add()
        index_item.key = "index"
        index_item.string_value = str(i)
        
        batch_item = record.metadata.add()
        batch_item.key = "batch"
        batch_item.string_value = "true"
    
    batch_serialized = batch.SerializeToString()
    print(f"Batch with {len(batch.vectors)} vectors serialized to {len(batch_serialized)} bytes")
    
    # Test 5: JSON conversion
    print("\n5️⃣ JSON conversion:")
    
    json_str = json_format.MessageToJson(vector_record, preserving_proto_field_name=True)
    print(f"JSON representation ({len(json_str)} chars):")
    print(json_str[:200] + "..." if len(json_str) > 200 else json_str)
    
    # Test 6: Collection config
    print("\n6️⃣ Collection config:")
    
    config = pb2.CollectionConfig()
    config.name = "proto_test_collection"
    config.dimension = 384
    config.distance_metric = pb2.COSINE
    config.storage_engine = pb2.StorageEngine.SST
    config.primary_indexing_algorithm = pb2.HNSW
    
    print(f"Collection: {config.name}")
    print(f"Dimension: {config.dimension}")
    print(f"Distance metric: {pb2.DistanceMetric.Name(config.distance_metric)}")
    print(f"Storage engine: {pb2.StorageEngine.Name(config.storage_engine)}")
    print(f"Indexing: {pb2.IndexingAlgorithm.Name(config.primary_indexing_algorithm)}")
    
    print("\n✅ Proto serialization test complete!")
    print("Proto is now the default serialization format for ProximaDB")


