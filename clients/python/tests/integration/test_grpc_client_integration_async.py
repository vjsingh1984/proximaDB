"""
Async gRPC client integration tests for ProximaDB Python SDK.
Tests actual gRPC client functionality against a running server.
"""

import pytest
import asyncio
import time
import numpy as np
from proximadb.grpc_client import ProximaDBClient
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError, NetworkError
import grpc


class TestGrpcClientIntegrationAsync:
    """Integration tests for async gRPC client with real server"""
    
    @pytest.fixture(scope="class")
    def client(self):
        """Create gRPC client for testing"""
        # gRPC server runs on port 5679
        return ProximaDBClient(endpoint="localhost:5679")
    
    @pytest.fixture
    def test_collection_name(self):
        """Generate unique collection name for each test"""
        return f"test_grpc_collection_{int(time.time() * 1000)}"
    
    def test_grpc_client_initialization(self):
        """Test gRPC client initialization and configuration"""
        # Test default initialization
        client = ProximaDBClient()
        assert hasattr(client, 'channel')
        assert hasattr(client, 'stub')
        assert client.endpoint == "localhost:5679"  # default
        
        # Test custom initialization
        custom_client = ProximaDBClient(endpoint="localhost:5679", timeout=60.0)
        assert custom_client.endpoint == "localhost:5679"
        assert custom_client.timeout == 60.0
    
    @pytest.mark.asyncio
    async def test_grpc_list_collections(self, client):
        """Test listing collections via gRPC"""
        collections = await client.list_collections()
        assert isinstance(collections, list)
        # Collections might be empty or contain existing collections
    
    @pytest.mark.asyncio
    async def test_grpc_collection_lifecycle(self, client, test_collection_name):
        """Test complete collection lifecycle through gRPC client"""
        # Step 1: Create collection
        # gRPC client expects individual parameters, not CollectionConfig
        created_collection = await client.create_collection(
            name=test_collection_name,
            dimension=256,
            distance_metric=2,  # EUCLIDEAN = 2
            indexing_algorithm=1,  # HNSW = 1
            storage_engine=1  # VIPER = 1
        )
        assert created_collection is not None
        
        try:
            # Step 2: List collections and verify creation
            collections = await client.list_collections()
            assert test_collection_name in collections
            
            # Step 3: Get collection details
            retrieved_collection = await client.get_collection(test_collection_name)
            assert retrieved_collection is not None
            
            # Check basic properties
            if hasattr(retrieved_collection, 'name'):
                assert retrieved_collection.name == test_collection_name
            if hasattr(retrieved_collection, 'dimension'):
                assert retrieved_collection.dimension == 256
            
        finally:
            # Step 4: Clean up - delete collection
            deleted = await client.delete_collection(test_collection_name)
            assert deleted is True
            
            # Verify deletion
            collections_after = await client.list_collections()
            assert test_collection_name not in collections_after
    
    @pytest.mark.asyncio
    async def test_grpc_vector_operations(self, client, test_collection_name):
        """Test vector operations through gRPC client"""
        # Create collection for testing
        await client.create_collection(
            name=test_collection_name,
            dimension=8,
            distance_metric=1  # COSINE = 1
        )
        
        try:
            # Test single vector insertion
            vector_data = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
            metadata = {"title": "grpc test document", "type": "integration"}
            
            insert_result = await client.insert_vector(
                collection_id=test_collection_name,
                vector_id="grpc_vec_1",
                vector=vector_data,
                metadata=metadata
            )
            
            assert insert_result is not None
            
            # Insert more vectors for search
            vectors_to_insert = [
                ("grpc_vec_2", [0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9], {"type": "test"}),
                ("grpc_vec_3", [0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0], {"type": "demo"}),
            ]
            
            for vec_id, vec_data, vec_meta in vectors_to_insert:
                await client.insert_vector(
                    collection_id=test_collection_name,
                    vector_id=vec_id,
                    vector=vec_data,
                    metadata=vec_meta
                )
            
            # Test vector search
            query_vector = [0.15, 0.25, 0.35, 0.45, 0.55, 0.65, 0.75, 0.85]
            search_results = await client.search_vectors(
                collection_id=test_collection_name,
                query=query_vector,
                k=3
            )
            
            assert search_results is not None
            assert len(search_results) > 0
            assert len(search_results) <= 3
            
            # Verify search result structure
            first_result = search_results[0]
            assert hasattr(first_result, 'id')
            assert hasattr(first_result, 'score')
            
            # Test vector deletion
            delete_result = await client.delete_vector(test_collection_name, "grpc_vec_1")
            assert delete_result is True
            
        finally:
            # Clean up collection
            await client.delete_collection(test_collection_name)
    
    @pytest.mark.asyncio
    async def test_grpc_batch_operations(self, client, test_collection_name):
        """Test batch vector operations through gRPC client"""
        # Create collection
        await client.create_collection(
            name=test_collection_name,
            dimension=4,
            distance_metric=3  # MANHATTAN = 3
        )
        
        try:
            # Test batch vector insertion
            vectors = [
                [0.1, 0.2, 0.3, 0.4],
                [0.5, 0.6, 0.7, 0.8],
                [0.9, 1.0, 1.1, 1.2]
            ]
            
            vector_ids = ["batch_vec_1", "batch_vec_2", "batch_vec_3"]
            metadatas = [
                {"batch": "test", "index": 1},
                {"batch": "test", "index": 2},
                {"batch": "test", "index": 3}
            ]
            
            # Insert vectors in batch
            batch_result = await client.insert_vectors(
                collection_id=test_collection_name,
                vector_ids=vector_ids,
                vectors=vectors,
                metadatas=metadatas
            )
            
            assert batch_result is not None
            
            # Verify vectors were inserted by searching
            query = [0.3, 0.4, 0.5, 0.6]
            results = await client.search_vectors(
                collection_id=test_collection_name,
                query=query,
                k=3
            )
            
            assert len(results) == 3
            
        finally:
            # Clean up
            await client.delete_collection(test_collection_name)
    
    @pytest.mark.asyncio
    async def test_grpc_numpy_compatibility(self, client, test_collection_name):
        """Test gRPC client with numpy arrays"""
        # Create collection
        await client.create_collection(
            name=test_collection_name,
            dimension=10,
            distance_metric=1  # COSINE = 1
        )
        
        try:
            # Test with numpy array
            vector_np = np.random.randn(10).astype(np.float32)
            
            insert_result = await client.insert_vector(
                collection_id=test_collection_name,
                vector_id="numpy_vec",
                vector=vector_np,
                metadata={"source": "numpy", "dtype": "float32"}
            )
            
            assert insert_result is not None
            
            # Test search with numpy query
            query_np = np.random.randn(10).astype(np.float32)
            search_results = await client.search_vectors(
                collection_id=test_collection_name,
                query=query_np,
                k=1
            )
            
            assert len(search_results) == 1
            assert search_results[0].id == "numpy_vec"
            
        finally:
            # Clean up
            await client.delete_collection(test_collection_name)
    
    @pytest.mark.asyncio
    async def test_grpc_error_handling(self, client):
        """Test error handling in gRPC client"""
        # Test operations on non-existent collection
        non_existent = "non_existent_collection_grpc"
        
        with pytest.raises((ProximaDBError, grpc.RpcError)):
            await client.get_collection(non_existent)
        
        with pytest.raises((ProximaDBError, grpc.RpcError)):
            await client.delete_collection(non_existent)
        
        with pytest.raises((ProximaDBError, grpc.RpcError)):
            await client.insert_vector(
                collection_id=non_existent,
                vector_id="test",
                vector=[0.1, 0.2, 0.3],
                metadata={}
            )
    
    @pytest.mark.asyncio
    async def test_grpc_large_vectors(self, client, test_collection_name):
        """Test gRPC client with large dimension vectors"""
        # Create collection with BERT-base dimensions
        await client.create_collection(
            name=test_collection_name,
            dimension=768,
            distance_metric=1  # COSINE = 1
        )
        
        try:
            # Create large vector
            large_vector = np.random.randn(768).astype(np.float32)
            
            # Insert large vector
            insert_result = await client.insert_vector(
                collection_id=test_collection_name,
                vector_id="bert_vector",
                vector=large_vector,
                metadata={"model": "bert-base", "dimension": 768}
            )
            
            assert insert_result is not None
            
            # Search with large query
            query_vector = np.random.randn(768).astype(np.float32)
            search_results = await client.search_vectors(
                collection_id=test_collection_name,
                query=query_vector,
                k=1
            )
            
            assert len(search_results) == 1
            assert search_results[0].id == "bert_vector"
            
        finally:
            # Clean up
            await client.delete_collection(test_collection_name)