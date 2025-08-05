"""
Integration tests for gRPC transport
"""

import pytest
from proximadb import ProximaDBClient, Protocol
from proximadb.models import (
    CollectionConfig,
    VectorRecord,
    DistanceMetric,
    StorageEngine,
    IndexingAlgorithm,
)
from proximadb.config import ClientConfig


class TestGRPCIntegration:
    """Test gRPC transport integration with ProximaDB server"""
    
    @pytest.fixture
    def grpc_client(self):
        """Create gRPC client"""
        config = ClientConfig(
            url="grpc://localhost:5679",
            protocol=Protocol.GRPC,
            verify_ssl=False,
            timeout=30.0
        )
        client = ProximaDBClient(config=config)
        yield client
        # Note: ProximaDBClient manages connections automatically
    
    def test_grpc_collection_operations(self, grpc_client):
        """Test collection operations via gRPC"""
        collection_name = "grpc_test_collection"
        
        # ProximaDBClient handles connections automatically
        # Note: gRPC transport will be used based on the grpc:// URL scheme
        
        # Delete if exists
        try:
            grpc_client.delete_collection(collection_name)
        except:
            pass
        
        # Create collection
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            description="Test collection for gRPC"
        )
        collection = grpc_client.create_collection(collection_name, config=config)
        assert collection.name == collection_name
        assert collection.config.dimension == 128
        assert collection.config.distance_metric == DistanceMetric.COSINE
        assert collection.config.storage_engine == StorageEngine.VIPER
        
        # Get collection
        retrieved = grpc_client.get_collection(collection_name)
        assert retrieved.id == collection.id
        assert retrieved.name == collection_name
        assert retrieved.config.dimension == 128
        
        # List collections
        collections = grpc_client.list_collections()
        collection_names = [c.name for c in collections]
        assert collection_name in collection_names
        
        # Delete collection
        result = grpc_client.delete_collection(collection_name)
        assert result.success is True
    
    def test_grpc_vector_operations(self, grpc_client):
        """Test vector operations via gRPC"""
        collection_name = "grpc_vector_test"
        
        # ProximaDBClient handles connections automatically
        
        # Create collection
        config = CollectionConfig(
            name=collection_name,
            dimension=384,
            distance_metric=DistanceMetric.EUCLIDEAN,
            storage_engine=StorageEngine.SST
        )
        
        try:
            grpc_client.delete_collection(collection_name)
        except:
            pass
        
        grpc_client.create_collection(config)
        
        # Insert single vector
        vector = VectorRecord(
            id="grpc_vec_1",
            vector=[float(i) for i in range(384)],
            metadata={"type": "test", "value": 1, "active": True}
        )
        
        result = grpc_client.insert_vector(collection_name, vector)
        assert result.success is True
        
        # Insert batch
        vectors = [
            VectorRecord(
                id=f"grpc_vec_{i}",
                vector=[float(i + j) for j in range(384)],
                metadata={"type": "test", "value": i, "category": f"cat_{i % 3}"}
            )
            for i in range(2, 12)
        ]
        
        batch_result = grpc_client.insert_vectors(collection_name, vectors)
        assert batch_result.success is True
        assert batch_result.success_count == 10
        
        # Get vector
        retrieved = grpc_client.get_vector(collection_name, "grpc_vec_1")
        assert retrieved.id == "grpc_vec_1"
        assert retrieved.metadata["type"] == "test"
        assert retrieved.metadata["value"] == 1.0  # Note: numbers come back as floats
        # Note: booleans might come back as 1.0/0.0 due to proto conversion
        assert retrieved.metadata["active"] in (True, 1.0)
        assert len(retrieved.vector) == 384
        
        # Search vectors
        query_vector = [float(i) for i in range(384)]
        from proximadb.models import SearchOptions
        search_options = SearchOptions(
            top_k=5,
            include_metadata=True,
            include_vectors=False
        )
        
        search_result = grpc_client.search_vectors(
            collection_name,
            query_vector,
            search_options
        )
        
        assert len(search_result.results) == 5
        assert search_result.results[0].id == "grpc_vec_1"
        assert search_result.results[0].metadata["type"] == "test"
        
        # Search with filter
        filter_options = SearchOptions(
            top_k=3,
            filter_dict={"category": "cat_1"},
            include_metadata=True
        )
        
        filtered_result = grpc_client.search_vectors(
            collection_name,
            query_vector,
            filter_options
        )
        
        # Check that all results have the filtered category
        for result in filtered_result.results:
            assert result.metadata.get("category") == "cat_1"
        
        # Cleanup
        grpc_client.delete_collection(collection_name)
    
    def test_grpc_async_operations(self, grpc_client):
        """Test async operations via gRPC"""
        import asyncio
        
        async def run_async_test():
            collection_name = "grpc_async_test"
            
            # Async connect
            await grpc_client.aconnect()
            
            # Cleanup
            try:
                await grpc_client.adelete_collection(collection_name)
            except:
                pass
            
            # Async create collection
            config = CollectionConfig(
                name=collection_name,
                dimension=64,
                distance_metric=DistanceMetric.DOT_PRODUCT,
                storage_engine=StorageEngine.VIPER
            )
            collection = await grpc_client.acreate_collection(config)
            assert collection.name == collection_name
            
            # Async insert
            vector = VectorRecord(
                id="async_vec",
                vector=[0.5] * 64,
                metadata={"async": True}
            )
            result = await grpc_client.ainsert_vector(collection_name, vector)
            assert result.success is True
            
            # Async search
            search_result = await grpc_client.asearch_vectors(
                collection_name,
                [0.5] * 64,
                SearchOptions(top_k=1)
            )
            assert len(search_result.results) == 1
            assert search_result.results[0].id == "async_vec"
            
            # Cleanup
            await grpc_client.adelete_collection(collection_name)
            await grpc_client.adisconnect()
        
        # Run async test
        asyncio.run(run_async_test())
    
    def test_grpc_error_handling(self, grpc_client):
        """Test error handling via gRPC"""
        # ProximaDBClient handles connections automatically
        
        # Try to get non-existent collection
        with pytest.raises(Exception) as exc_info:
            grpc_client.get_collection("non_existent_collection")
        assert "TRANSPORT_ERROR" in str(exc_info.value)
        
        # Try to create collection with invalid parameters
        with pytest.raises(Exception) as exc_info:
            config = CollectionConfig(
                name="",  # Invalid empty name
                dimension=128,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.VIPER
            )
            grpc_client.create_collection(config)