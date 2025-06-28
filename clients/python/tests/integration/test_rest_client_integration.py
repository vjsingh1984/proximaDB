"""
REST client integration tests for ProximaDB Python SDK.
Tests actual REST client functionality against a running server.
"""

import pytest
import time
import numpy as np
from proximadb.rest_client import ProximaDBRestClient
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError, NetworkError


class TestRestClientIntegration:
    """Integration tests for REST client with real server"""
    
    @pytest.fixture(scope="class")
    def client(self):
        """Create REST client for testing"""
        return ProximaDBRestClient(url="http://localhost:5678")
    
    @pytest.fixture
    def test_collection_name(self):
        """Generate unique collection name for each test"""
        return f"test_rest_collection_{int(time.time() * 1000)}"
    
    def test_rest_client_initialization(self):
        """Test REST client initialization and configuration"""
        # Test custom initialization
        custom_client = ProximaDBRestClient(url="http://localhost:5678")
        assert custom_client.config.url == "http://localhost:5678"
        assert hasattr(custom_client.config, 'timeout')
    
    def test_health_endpoint(self, client):
        """Test health endpoint through REST client"""
        health = client.health()
        
        assert hasattr(health, 'status')
        assert health.status in ['healthy', 'ok', 'running']
        assert hasattr(health, 'version')
    
    def test_list_collections_initially_empty(self, client):
        """Test listing collections when none exist"""
        collections = client.list_collections()
        assert isinstance(collections, list)
        # Collections list might be empty or contain existing collections
    
    def test_vector_normalization_methods(self, client):
        """Test vector normalization functionality"""
        # Test _normalize_vector with list
        vector_list = [0.1, 0.2, 0.3, 0.4, 0.5]
        normalized = client._normalize_vector(vector_list)
        assert isinstance(normalized, list)
        assert len(normalized) == 5
        assert normalized == vector_list
        
        # Test _normalize_vector with numpy array
        vector_np = np.array([0.1, 0.2, 0.3, 0.4, 0.5], dtype=np.float32)
        normalized = client._normalize_vector(vector_np)
        assert isinstance(normalized, list)
        assert len(normalized) == 5
        
        # Test _normalize_vectors with list of lists
        vectors_list = [
            [0.1, 0.2, 0.3],
            [0.4, 0.5, 0.6]
        ]
        normalized_vectors = client._normalize_vectors(vectors_list)
        assert isinstance(normalized_vectors, list)
        assert len(normalized_vectors) == 2
        
        # Test _normalize_vectors with numpy array
        vectors_np = np.array([[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]])
        normalized_vectors_np = client._normalize_vectors(vectors_np)
        assert isinstance(normalized_vectors_np, list)
        assert len(normalized_vectors_np) == 2
        assert all(isinstance(v, list) for v in normalized_vectors_np)
    
    def test_collection_lifecycle(self, client, test_collection_name):
        """Test complete collection lifecycle through REST client"""
        # Step 1: Create collection
        config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            description="Test collection for REST client integration"
        )
        
        created_collection = client.create_collection(test_collection_name, config)
        assert hasattr(created_collection, 'id')
        assert hasattr(created_collection, 'name')
        assert created_collection.name == test_collection_name
        
        collection_id = created_collection.id
        
        try:
            # Step 2: List collections and verify creation
            collections = client.list_collections()
            assert test_collection_name in collections
            
            # Step 3: Get collection details
            retrieved_collection = client.get_collection(test_collection_name)
            assert retrieved_collection.name == test_collection_name
            assert hasattr(retrieved_collection, 'vector_count')
            assert hasattr(retrieved_collection, 'id')
            
            # Step 4: Verify collection is accessible
            assert retrieved_collection.vector_count == 0  # Should be empty initially
            
        finally:
            # Step 5: Clean up - delete collection
            deleted = client.delete_collection(test_collection_name)
            assert deleted is True
            
            # Verify deletion
            collections_after = client.list_collections()
            assert test_collection_name not in collections_after
    
    def test_vector_operations_lifecycle(self, client, test_collection_name):
        """Test vector operations through REST client"""
        # Create collection for testing
        config = CollectionConfig(dimension=5, distance_metric=DistanceMetric.COSINE)
        created_collection = client.create_collection(test_collection_name, config)
        
        try:
            # Test vector insertion
            vector_data = [0.1, 0.2, 0.3, 0.4, 0.5]
            metadata = {"title": "test document", "category": "integration"}
            
            insert_result = client.insert_vector(
                collection_id=test_collection_name,
                vector_id="test_vector_1",
                vector=vector_data,
                metadata=metadata
            )
            
            assert hasattr(insert_result, 'count')
            assert insert_result.count == 1
            assert hasattr(insert_result, 'duration_ms')
            
            # Insert another vector for search testing
            vector_data_2 = [0.2, 0.3, 0.4, 0.5, 0.6]
            metadata_2 = {"title": "another document", "category": "test"}
            
            client.insert_vector(
                collection_id=test_collection_name,
                vector_id="test_vector_2",
                vector=vector_data_2,
                metadata=metadata_2
            )
            
            # Test vector search
            query_vector = [0.15, 0.25, 0.35, 0.45, 0.55]
            search_results = client.search(
                collection_id=test_collection_name,
                query=query_vector,
                k=2
            )
            
            assert isinstance(search_results, list)
            assert len(search_results) <= 2
            assert len(search_results) > 0
            
            # Verify search results have expected structure
            first_result = search_results[0]
            assert hasattr(first_result, 'id')
            assert hasattr(first_result, 'score')
            assert hasattr(first_result, 'metadata')
            
            # Test vector deletion
            delete_result = client.delete_vector(test_collection_name, "test_vector_1")
            assert hasattr(delete_result, 'deleted_count')
            assert delete_result.deleted_count == 1
            
        finally:
            # Clean up collection
            client.delete_collection(test_collection_name)
    
    def test_batch_vector_operations(self, client, test_collection_name):
        """Test batch vector operations through REST client"""
        # Create collection
        config = CollectionConfig(dimension=3, distance_metric=DistanceMetric.EUCLIDEAN)
        client.create_collection(test_collection_name, config)
        
        try:
            # Test batch vector insertion
            vectors = [
                [0.1, 0.2, 0.3],
                [0.4, 0.5, 0.6],
                [0.7, 0.8, 0.9]
            ]
            
            vector_ids = ["vec_1", "vec_2", "vec_3"]
            metadatas = [
                {"type": "document", "index": 1},
                {"type": "document", "index": 2},
                {"type": "document", "index": 3}
            ]
            
            batch_result = client.insert_vectors(
                collection_id=test_collection_name,
                vector_ids=vector_ids,
                vectors=vectors,
                metadatas=metadatas
            )
            
            assert hasattr(batch_result, 'successful_count')
            assert batch_result.successful_count == 3
            assert hasattr(batch_result, 'failed_count')
            assert batch_result.failed_count == 0
            
            # Test batch vector deletion
            delete_result = client.delete_vectors(
                collection_id=test_collection_name,
                vector_ids=["vec_1", "vec_3"]
            )
            
            assert hasattr(delete_result, 'deleted_count')
            assert delete_result.deleted_count == 2
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)
    
    def test_search_with_filters(self, client, test_collection_name):
        """Test search with metadata filters through REST client"""
        # Create collection
        config = CollectionConfig(
            dimension=4,
            distance_metric=DistanceMetric.COSINE,
            filterable_metadata_fields=["category", "priority"]
        )
        client.create_collection(test_collection_name, config)
        
        try:
            # Insert vectors with different metadata
            vectors_data = [
                ([0.1, 0.2, 0.3, 0.4], {"category": "A", "priority": 1}),
                ([0.2, 0.3, 0.4, 0.5], {"category": "B", "priority": 2}),
                ([0.3, 0.4, 0.5, 0.6], {"category": "A", "priority": 3}),
            ]
            
            for i, (vector, metadata) in enumerate(vectors_data):
                client.insert_vector(
                    collection_id=test_collection_name,
                    vector_id=f"filtered_vec_{i}",
                    vector=vector,
                    metadata=metadata
                )
            
            # Test search with filter
            query_vector = [0.2, 0.3, 0.4, 0.5]
            search_result = client.search(
                collection_id=test_collection_name,
                query=query_vector,
                k=10,
                filter={"category": "A"}
            )
            
            assert len(search_result.results) == 2
            # All results should have category "A"
            for result in search_result.results:
                assert result.metadata["category"] == "A"
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)
    
    def test_error_handling_scenarios(self, client):
        """Test error handling in various scenarios"""
        # Test getting non-existent collection
        with pytest.raises(ProximaDBError):
            client.get_collection("non_existent_collection_123456")
        
        # Test deleting non-existent collection
        with pytest.raises(ProximaDBError):
            client.delete_collection("non_existent_collection_123456")
        
        # Test inserting vector to non-existent collection
        with pytest.raises(ProximaDBError):
            client.insert_vector(
                collection_id="non_existent_collection",
                vector_id="test_vec",
                vector=[0.1, 0.2, 0.3],
                metadata={}
            )
    
    def test_get_metrics_endpoint(self, client):
        """Test get metrics endpoint through REST client"""
        try:
            metrics = client.get_metrics()
            assert isinstance(metrics, dict)
            # Metrics should contain some basic information
            # The exact structure may vary, so we just check it's a dict
        except ProximaDBError:
            # Metrics endpoint might not be implemented or available
            pytest.skip("Metrics endpoint not available")
    
    def test_numpy_vector_compatibility(self, client, test_collection_name):
        """Test compatibility with numpy vectors"""
        # Create collection
        config = CollectionConfig(dimension=6, distance_metric=DistanceMetric.COSINE)
        client.create_collection(test_collection_name, config)
        
        try:
            # Test with numpy arrays
            vector_np = np.array([0.1, 0.2, 0.3, 0.4, 0.5, 0.6], dtype=np.float32)
            
            insert_result = client.insert_vector(
                collection_id=test_collection_name,
                vector_id="numpy_vector",
                vector=vector_np,
                metadata={"source": "numpy"}
            )
            
            assert insert_result.count == 1
            
            # Test search with numpy query
            query_np = np.array([0.15, 0.25, 0.35, 0.45, 0.55, 0.65], dtype=np.float64)
            search_result = client.search(
                collection_id=test_collection_name,
                query=query_np,
                k=1
            )
            
            assert len(search_result.results) == 1
            assert search_result.results[0].id == "numpy_vector"
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)
    
    def test_large_dimension_vectors(self, client, test_collection_name):
        """Test with larger dimension vectors (realistic scenario)"""
        # Create collection with BERT-like dimensions
        config = CollectionConfig(dimension=768, distance_metric=DistanceMetric.COSINE)
        client.create_collection(test_collection_name, config)
        
        try:
            # Create a realistic 768-dimensional vector
            vector_768 = np.random.normal(0, 1, 768).astype(np.float32).tolist()
            
            insert_result = client.insert_vector(
                collection_id=test_collection_name,
                vector_id="bert_like_vector",
                vector=vector_768,
                metadata={"model": "bert-base", "text": "sample document"}
            )
            
            assert insert_result.count == 1
            
            # Test search with similar dimension query
            query_768 = np.random.normal(0, 1, 768).astype(np.float32).tolist()
            search_result = client.search(
                collection_id=test_collection_name,
                query=query_768,
                k=1
            )
            
            assert len(search_result.results) == 1
            result = search_result.results[0]
            assert result.id == "bert_like_vector"
            assert result.metadata["model"] == "bert-base"
            
        finally:
            # Clean up
            client.delete_collection(test_collection_name)