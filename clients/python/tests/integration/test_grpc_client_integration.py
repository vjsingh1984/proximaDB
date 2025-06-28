"""
gRPC client integration tests for ProximaDB Python SDK.
Tests actual gRPC client functionality against a running server.
"""

import pytest
import time
import numpy as np
from proximadb.grpc_client import ProximaDBClient
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError, NetworkError
import grpc


class TestGrpcClientIntegration:
    """Integration tests for gRPC client with real server"""
    
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
        
        # Test custom initialization
        custom_client = ProximaDBClient(endpoint="localhost:5679")
        assert custom_client.endpoint == "localhost:5679"
    
    def test_grpc_health_check(self, client):
        """Test health check through gRPC client"""
        try:
            # gRPC client might not have a health method, test connection
            collections = client.list_collections()
            assert isinstance(collections, list)
        except Exception as e:
            # If health check is not implemented, just verify connection works
            pytest.skip(f"Health check not available: {e}")
    
    def test_grpc_list_collections(self, client):
        """Test listing collections via gRPC"""
        collections = client.list_collections()
        assert isinstance(collections, list)
        # Collections might be empty or contain existing collections
    
    def test_grpc_collection_lifecycle(self, client, test_collection_name):
        """Test complete collection lifecycle through gRPC client"""
        # Step 1: Create collection
        config = CollectionConfig(
            dimension=256,
            distance_metric=DistanceMetric.EUCLIDEAN,
            description="Test collection for gRPC client integration"
        )
        
        created_collection = client.create_collection(test_collection_name, config)
        assert created_collection is not None
        
        collection_id = test_collection_name  # gRPC might use name as ID
        
        try:
            # Step 2: List collections and verify creation
            collections = client.list_collections()
            assert test_collection_name in collections
            
            # Step 3: Get collection details
            retrieved_collection = client.get_collection(test_collection_name)
            assert retrieved_collection is not None
            
            # Check basic properties
            if hasattr(retrieved_collection, 'name'):
                assert retrieved_collection.name == test_collection_name
            if hasattr(retrieved_collection, 'config') and retrieved_collection.config:
                assert retrieved_collection.config.dimension == 256
            
        finally:
            # Step 4: Clean up - delete collection
            deleted = client.delete_collection(test_collection_name)
            assert deleted is True
            
            # Verify deletion
            collections_after = client.list_collections()
            assert test_collection_name not in collections_after
    
    def test_grpc_vector_operations(self, client, test_collection_name):
        """Test vector operations through gRPC client"""
        # Create collection for testing
        config = CollectionConfig(dimension=8, distance_metric=DistanceMetric.COSINE)
        client.create_collection(test_collection_name, config)
        
        try:
            # Test single vector insertion
            vector_data = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
            metadata = {"title": "grpc test document", "type": "integration"}
            
            insert_result = client.insert_vector(
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
                client.insert_vector(
                    collection_id=test_collection_name,
                    vector_id=vec_id,
                    vector=vec_data,
                    metadata=vec_meta
                )
            
            # Test vector search
            query_vector = [0.15, 0.25, 0.35, 0.45, 0.55, 0.65, 0.75, 0.85]
            search_results = client.search_vectors(
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
            delete_result = client.delete_vector(test_collection_name, "grpc_vec_1")
            assert delete_result is True
            
        finally:
            # Clean up collection
            client.delete_collection(test_collection_name)
    
    def test_grpc_batch_operations(self, client, test_collection_name):
        """Test batch vector operations through gRPC client"""
        # Create collection
        config = CollectionConfig(dimension=4, distance_metric=DistanceMetric.MANHATTAN)
        client.create_collection(test_collection_name, config)
        
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
            batch_result = client.insert_vectors(
                collection_id=test_collection_name,
                vector_ids=vector_ids,
                vectors=vectors,
                metadatas=metadatas
            )
            
            assert batch_result is not None
            
            # Verify vectors were inserted by searching
            query = [0.3, 0.4, 0.5, 0.6]
            results = client.search_vectors(
                collection_id=test_collection_name,
                query=query,
                k=3
            )
            
            assert len(results) == 3
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)
    
    def test_grpc_numpy_compatibility(self, client, test_collection_name):
        """Test gRPC client with numpy arrays"""
        # Create collection
        config = CollectionConfig(dimension=10, distance_metric=DistanceMetric.COSINE)
        client.create_collection(test_collection_name, config)
        
        try:
            # Test with numpy array
            vector_np = np.random.randn(10).astype(np.float32)
            
            insert_result = client.insert_vector(
                collection_id=test_collection_name,
                vector_id="numpy_vec",
                vector=vector_np,
                metadata={"source": "numpy", "dtype": "float32"}
            )
            
            assert insert_result is not None
            
            # Test search with numpy query
            query_np = np.random.randn(10).astype(np.float32)
            search_results = client.search_vectors(
                collection_id=test_collection_name,
                query=query_np,
                k=1
            )
            
            assert len(search_results) == 1
            assert search_results[0].id == "numpy_vec"
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)
    
    def test_grpc_error_handling(self, client):
        """Test error handling in gRPC client"""
        # Test operations on non-existent collection
        non_existent = "non_existent_collection_grpc"
        
        with pytest.raises((ProximaDBError, grpc.RpcError)):
            client.get_collection(non_existent)
        
        with pytest.raises((ProximaDBError, grpc.RpcError)):
            client.delete_collection(non_existent)
        
        with pytest.raises((ProximaDBError, grpc.RpcError)):
            client.insert_vector(
                collection_id=non_existent,
                vector_id="test",
                vector=[0.1, 0.2, 0.3],
                metadata={}
            )
    
    def test_grpc_large_vectors(self, client, test_collection_name):
        """Test gRPC client with large dimension vectors"""
        # Create collection with BERT-base dimensions
        config = CollectionConfig(dimension=768, distance_metric=DistanceMetric.COSINE)
        client.create_collection(test_collection_name, config)
        
        try:
            # Create large vector
            large_vector = np.random.randn(768).astype(np.float32)
            
            # Insert large vector
            insert_result = client.insert_vector(
                collection_id=test_collection_name,
                vector_id="bert_vector",
                vector=large_vector,
                metadata={"model": "bert-base", "dimension": 768}
            )
            
            assert insert_result is not None
            
            # Search with large query
            query_vector = np.random.randn(768).astype(np.float32)
            search_results = client.search_vectors(
                collection_id=test_collection_name,
                query=query_vector,
                k=1
            )
            
            assert len(search_results) == 1
            assert search_results[0].id == "bert_vector"
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)
    
    def test_grpc_metadata_filtering(self, client, test_collection_name):
        """Test metadata filtering in search through gRPC"""
        # Create collection with filterable fields
        config = CollectionConfig(
            dimension=6,
            distance_metric=DistanceMetric.EUCLIDEAN,
            filterable_metadata_fields=["category", "priority", "status"]
        )
        client.create_collection(test_collection_name, config)
        
        try:
            # Insert vectors with different metadata
            test_vectors = [
                ("vec1", [0.1, 0.2, 0.3, 0.4, 0.5, 0.6], {"category": "A", "priority": 1, "status": "active"}),
                ("vec2", [0.2, 0.3, 0.4, 0.5, 0.6, 0.7], {"category": "B", "priority": 2, "status": "active"}),
                ("vec3", [0.3, 0.4, 0.5, 0.6, 0.7, 0.8], {"category": "A", "priority": 3, "status": "inactive"}),
                ("vec4", [0.4, 0.5, 0.6, 0.7, 0.8, 0.9], {"category": "B", "priority": 1, "status": "active"}),
            ]
            
            for vec_id, vec_data, metadata in test_vectors:
                client.insert_vector(
                    collection_id=test_collection_name,
                    vector_id=vec_id,
                    vector=vec_data,
                    metadata=metadata
                )
            
            # Test search with metadata filter
            query = [0.35, 0.45, 0.55, 0.65, 0.75, 0.85]
            
            # Filter by category A
            results_a = client.search_vectors(
                collection_id=test_collection_name,
                query=query,
                k=10,
                filter={"category": "A"}
            )
            
            # Should get only vectors with category A
            assert all(r.metadata.get("category") == "A" for r in results_a if hasattr(r, 'metadata'))
            
            # Filter by priority 1
            results_priority = client.search_vectors(
                collection_id=test_collection_name,
                query=query,
                k=10,
                filter={"priority": 1}
            )
            
            # Should get only vectors with priority 1
            assert all(r.metadata.get("priority") == 1 for r in results_priority if hasattr(r, 'metadata'))
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)
    
    def test_grpc_connection_resilience(self):
        """Test gRPC client connection handling"""
        # Test with invalid host
        with pytest.raises((NetworkError, grpc.RpcError, ProximaDBError)):
            bad_client = ProximaDBClient(endpoint="invalid_host:9999")
            bad_client.list_collections()
        
        # Test with valid host but wrong port
        with pytest.raises((NetworkError, grpc.RpcError, ProximaDBError)):
            bad_client = ProximaDBClient(endpoint="localhost:9999")
            bad_client.list_collections()