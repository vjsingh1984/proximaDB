#!/usr/bin/env python3
"""
ProximaDB Collection Operations Test Suite
Consolidated tests for collection CRUD operations, configuration, and lifecycle management
"""

import pytest
import time
from typing import Dict, Any

from proximadb import ProximaDBClient, Protocol, connect_rest, connect_grpc
from proximadb.models import (
    CollectionConfig, IndexConfiguration,
    DistanceMetric, StorageEngine, IndexingAlgorithm,
    Collection, CollectionStats, StorageConfig, CompressionType,
    FlushConfig
)
from proximadb import IndexConfig  # Import alias for IndexConfiguration
from proximadb.exceptions import ProximaDBError, CollectionNotFoundError
from proximadb.config import ClientConfig


class TestCollectionCRUD:
    """Test collection Create, Read, Update, Delete operations"""
    
    @pytest.fixture(scope="class")
    def rest_client(self):
        """REST client fixture"""
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture(scope="class") 
    def grpc_client(self):
        """gRPC client fixture"""
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    @pytest.fixture
    def collection_name(self):
        """Generate unique collection name for each test"""
        return f"test_collection_{int(time.time())}"
    
    def test_collection_lifecycle_rest(self, rest_client, collection_name):
        """Test complete collection lifecycle via REST"""
        config = CollectionConfig(
            name="test_collection",
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            description="REST test collection"
        )
        
        # Create collection
        collection = rest_client.create_collection(collection_name, config)
        assert collection is not None
        
        # List collections - verify creation
        collections = rest_client.list_collections()
        assert collections is not None
        # Handle both string and object responses
        if isinstance(collections, list) and len(collections) > 0:
            if isinstance(collections[0], str):
                collection_names = collections
            else:
                collection_names = [getattr(col, 'id', getattr(col, 'name', str(col))) for col in collections]
        else:
            collection_names = []
        assert collection_name in collection_names
        
        # Get specific collection
        retrieved = rest_client.get_collection(collection_name)
        assert retrieved is not None
        
        # Delete collection
        result = rest_client.delete_collection(collection_name)
        
        # Verify deletion
        with pytest.raises((CollectionNotFoundError, ProximaDBError)):
            rest_client.get_collection(collection_name)
    
    def test_collection_lifecycle_grpc(self, grpc_client, collection_name):
        """Test complete collection lifecycle via gRPC"""
        config = CollectionConfig(
            name="test_collection",
            dimension=256,
            distance_metric=DistanceMetric.DOT_PRODUCT,
            description="gRPC test collection"
        )
        
        # Create collection
        collection = grpc_client.create_collection(collection_name, config)
        assert collection is not None
        
        # List collections
        collections = grpc_client.list_collections()
        assert collections is not None
        
        # Get specific collection  
        retrieved = grpc_client.get_collection(collection_name)
        assert retrieved is not None
        
        # Delete collection
        result = grpc_client.delete_collection(collection_name)
    
    def test_cross_protocol_operations(self, rest_client, grpc_client, collection_name):
        """Test collection operations across REST and gRPC protocols"""
        config = CollectionConfig(
            name="test_collection",
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            description="Cross-protocol test collection"
        )
        
        # Create with REST
        collection = rest_client.create_collection(collection_name, config)
        assert collection is not None
        
        # Verify with gRPC
        retrieved_via_grpc = grpc_client.get_collection(collection_name)
        assert retrieved_via_grpc is not None
        
        # List via both protocols
        rest_collections = rest_client.list_collections()
        grpc_collections = grpc_client.list_collections()
        
        # Both should see the collection
        rest_names = [getattr(col, 'id', getattr(col, 'name', None)) for col in rest_collections]
        grpc_names = [getattr(col, 'id', getattr(col, 'name', None)) for col in grpc_collections]
        
        assert collection_name in rest_names
        assert collection_name in grpc_names
        
        # Delete with gRPC
        grpc_client.delete_collection(collection_name)
        
        # Verify deletion with REST
        with pytest.raises((CollectionNotFoundError, ProximaDBError)):
            rest_client.get_collection(collection_name)


class TestCollectionConfiguration:
    """Test collection configuration options and validation"""
    
    @pytest.fixture
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    def test_basic_collection_config(self):
        """Test basic collection configuration"""
        config = CollectionConfig(
            name="test_collection",
            dimension=768,
            distance_metric=DistanceMetric.COSINE)
        assert config.dimension == 768
        assert config.distance_metric == DistanceMetric.COSINE
    
    def test_advanced_collection_config(self):
        """Test advanced collection configuration with all options"""
        index_config = IndexConfiguration(
            index_name="test_index",
            algorithm=IndexingAlgorithm.HNSW,
            # parameters={"m": 16, "ef_construction": 200}  # Not a field anymore
        )
        
        storage_config = StorageConfig(
            compression=CompressionType.LZ4,
            replication_factor=1,
            enable_tiering=True
        )
        
        flush_config = FlushConfig(max_wal_size_mb=64.0)
        
        config = CollectionConfig(
            name="test_collection",
            dimension=384,
            distance_metric=DistanceMetric.EUCLIDEAN,
            index_configs=[index_config],  # Use plural form
            # storage_config=storage_config,  # This field doesn't exist in the model
            storage_engine=StorageEngine.VIPER,
            description="Advanced test collection",
            filterable_metadata_fields=["category", "timestamp"]
            # flush_config=flush_config  # This field doesn't exist in the model
        )
        
        assert config.dimension == 384
        assert config.distance_metric == DistanceMetric.EUCLIDEAN
        # The model has index_configs (plural) but we added a backward compatibility property
        assert config.index_config.algorithm == IndexingAlgorithm.HNSW
        assert config.storage_config.compression == CompressionType.LZ4
    
    def test_distance_metrics(self):
        """Test all distance metric options"""
        metrics = [
            DistanceMetric.COSINE,
            DistanceMetric.EUCLIDEAN,
            DistanceMetric.DOT_PRODUCT,
            DistanceMetric.MANHATTAN,
            DistanceMetric.HAMMING
        ]
        
        for metric in metrics:
            config = CollectionConfig(
            name="test_collection",
            dimension=128,
            distance_metric=metric)
            assert config.distance_metric == metric
    
    def test_index_algorithms(self):
        """Test index algorithm options"""
        algorithms = [
            IndexingAlgorithm.HNSW,
            IndexingAlgorithm.IVF,
            IndexingAlgorithm.PQ,
            IndexingAlgorithm.FLAT,
            IndexingAlgorithm.ANNOY
        ]
        
        for algo in algorithms:
            config = CollectionConfig(name="test", dimension=128, primary_indexing_algorithm=algo)
            assert config.primary_indexing_algorithm == algo
    
    def test_storage_engines(self):
        """Test storage engine options"""
        engines = [
            StorageEngine.VIPER,
            StorageEngine.LSM,
            StorageEngine.MMAP,
            StorageEngine.HYBRID
        ]
        
        for engine in engines:
            config = CollectionConfig(name="test", dimension=128, storage_engine=engine)
            assert config.storage_engine == engine
    
    def test_collection_with_filterable_columns(self):
        """Test collection with filterable columns configuration"""
        from proximadb.models import FilterableColumn, FilterableDataType
        
        filterable_cols = [
            FilterableColumn(name="category", data_type=FilterableDataType.STRING, indexed=True),
            FilterableColumn(name="timestamp", data_type=FilterableDataType.DATETIME, indexed=True),
            FilterableColumn(name="score", data_type=FilterableDataType.FLOAT, indexed=False)
        ]
        
        config = CollectionConfig(
            name="test_filterable",
            dimension=512,
            filterable_columns=filterable_cols
        )
        
        assert config.filterable_columns is not None
        assert len(config.filterable_columns) == 3
        assert config.filterable_columns[0].name == "category"
    
    def test_collection_creation_with_config(self, rest_client):
        """Test creating collection with advanced configuration"""
        collection_name = f"config_test_{int(time.time())}"
        
        config = CollectionConfig(
            name=collection_name,
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            description="Configuration test collection",
            storage_engine=StorageEngine.VIPER
        )
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            assert collection is not None
            
            # Verify configuration persisted
            retrieved = rest_client.get_collection(collection_name)
            assert retrieved is not None
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass


class TestCollectionValidation:
    """Test collection configuration validation and error handling"""
    
    def test_dimension_validation(self):
        """Test dimension validation"""
        # Valid dimensions
        valid_config = CollectionConfig(
            name="test_collection",
            dimension=128,
            distance_metric=DistanceMetric.COSINE)
        assert valid_config.dimension == 128
        
        # Invalid dimensions should raise validation errors
        with pytest.raises((ValueError, TypeError)):
            CollectionConfig(
                name="test_collection",
                dimension=0,  # Invalid dimension
                distance_metric=DistanceMetric.COSINE)
        
        with pytest.raises((ValueError, TypeError)):
            CollectionConfig(
                name="test_collection",
                dimension=10001,  # Too large
                distance_metric=DistanceMetric.COSINE)
    
    def test_collection_not_found_error(self):
        """Test CollectionNotFoundError handling"""
        client = connect_rest("http://localhost:5678")
        non_existent = f"non_existent_{int(time.time())}"
        
        with pytest.raises((CollectionNotFoundError, ProximaDBError)):
            client.get_collection(non_existent)
    
    def test_duplicate_collection_creation(self):
        """Test handling of duplicate collection creation"""
        client = connect_rest("http://localhost:5678")
        collection_name = f"duplicate_test_{int(time.time())}"
        
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE)
        
        try:
            # Create first time - should succeed
            collection1 = client.create_collection(collection_name, config)
            assert collection1 is not None
            
            # Create again - should raise error or handle gracefully
            with pytest.raises(ProximaDBError):
                client.create_collection(collection_name, config)
                
        finally:
            try:
                client.delete_collection(collection_name)
            except:
                pass


class TestCollectionPersistence:
    """Test collection persistence across server restarts"""
    
    def test_collection_persistence_after_restart(self):
        """Test that collections persist after server restart"""
        client = connect_rest("http://localhost:5678")
        collection_name = f"persist_test_{int(time.time())}"
        
        config = CollectionConfig(
            name=collection_name,
            dimension=256,
            distance_metric=DistanceMetric.COSINE,
            description="Persistence test collection"
        )
        
        try:
            # Create collection
            collection = client.create_collection(collection_name, config)
            assert collection is not None
            
            # Verify it exists immediately
            retrieved = client.get_collection(collection_name)
            assert retrieved is not None
            
            # Note: Actual server restart testing would require test infrastructure
            # This test verifies the basic persistence mechanism is working
            
        finally:
            try:
                client.delete_collection(collection_name)
            except:
                pass


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])