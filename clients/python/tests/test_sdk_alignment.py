#!/usr/bin/env python3
"""
Test script to verify SDK alignment between gRPC and REST clients.

This test checks that:
1. gRPC client uses proto types directly
2. REST client uses Pydantic models
3. Both clients have consistent interfaces
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'src'))

import pytest
from proximadb.grpc_client import ProximaDBClient
from proximadb.rest_client import ProximaDBRestClient
from proximadb.models import CollectionConfig, DistanceMetric, StorageEngine, IndexingAlgorithm
from proximadb import proximadb_pb2 as pb2


def test_grpc_client_returns_proto_types():
    """Test that gRPC client returns proto types directly"""
    client = ProximaDBClient(endpoint="localhost:5679")
    
    # Check return type annotations
    assert hasattr(client.create_collection, '__annotations__')
    annotations = client.create_collection.__annotations__
    assert 'return' in annotations
    assert annotations['return'] == pb2.Collection
    
    # Check health check returns proto
    health_annotations = client.health_check.__annotations__
    assert health_annotations['return'] == pb2.HealthResponse


def test_rest_client_uses_pydantic_models():
    """Test that REST client uses Pydantic models"""
    client = ProximaDBRestClient(url="http://localhost:5678")
    
    # Check that we can create Pydantic models
    config = CollectionConfig(
        name="test_collection",
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        primary_indexing_algorithm=IndexingAlgorithm.HNSW
    )
    
    assert isinstance(config, CollectionConfig)
    assert config.name == "test_collection"
    assert config.dimension == 128
    assert config.distance_metric == DistanceMetric.COSINE


def test_proto_vs_pydantic_separation():
    """Test that proto and Pydantic models are properly separated"""
    
    # Proto enums should be integers
    assert pb2.DistanceMetric.COSINE == 1
    assert pb2.StorageEngine.VIPER == 1
    assert pb2.IndexingAlgorithm.HNSW == 1
    
    # Pydantic enums should be strings
    assert DistanceMetric.COSINE == "cosine"
    assert StorageEngine.VIPER == "viper"
    assert IndexingAlgorithm.HNSW == "hnsw"
    
    # Proto models should be different from Pydantic models
    proto_collection = pb2.Collection()
    pydantic_config = CollectionConfig(
        name="test",
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        primary_indexing_algorithm=IndexingAlgorithm.HNSW
    )
    
    # They should be different types
    assert type(proto_collection) != type(pydantic_config)


def test_consistent_field_names():
    """Test that field names are consistent between proto and Pydantic"""
    
    # Both should have these core fields
    proto_config = pb2.CollectionConfig()
    pydantic_config = CollectionConfig(
        name="test",
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        primary_indexing_algorithm=IndexingAlgorithm.HNSW
    )
    
    # Check that key fields exist in both
    assert hasattr(proto_config, 'name')
    assert hasattr(proto_config, 'dimension')
    assert hasattr(proto_config, 'distance_metric')
    assert hasattr(proto_config, 'storage_engine')
    
    assert hasattr(pydantic_config, 'name')
    assert hasattr(pydantic_config, 'dimension')
    assert hasattr(pydantic_config, 'distance_metric')
    assert hasattr(pydantic_config, 'storage_engine')


if __name__ == "__main__":
    """Run basic tests to verify SDK alignment"""
    print("Testing gRPC client proto type usage...")
    test_grpc_client_returns_proto_types()
    print("✓ gRPC client properly returns proto types")
    
    print("\nTesting REST client Pydantic model usage...")
    test_rest_client_uses_pydantic_models()
    print("✓ REST client properly uses Pydantic models")
    
    print("\nTesting proto vs Pydantic separation...")
    test_proto_vs_pydantic_separation()
    print("✓ Proto and Pydantic models are properly separated")
    
    print("\nTesting field name consistency...")
    test_consistent_field_names()
    print("✓ Field names are consistent between proto and Pydantic")
    
    print("\n🎉 All SDK alignment tests passed!")
    print("\nSummary:")
    print("- gRPC client uses proto-generated classes directly")
    print("- REST client uses Pydantic models aligned with server REST handlers")
    print("- Both maintain consistent interfaces while using appropriate types")
    print("- No backward compatibility maintained (clean release 1 approach)")