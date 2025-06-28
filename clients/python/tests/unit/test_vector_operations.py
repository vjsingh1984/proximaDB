"""
Unit tests for ProximaDB vector operations.
Tests vector CRUD operations, batch operations, and search functionality.
"""

import pytest
import numpy as np
from unittest.mock import Mock, patch
from proximadb.unified_client import ProximaDBClient, Protocol
from proximadb.models import CollectionConfig, DistanceMetric, InsertResult, DeleteResult, SearchResult, BatchResult
from proximadb.exceptions import VectorNotFoundError, VectorDimensionError, ValidationError


class TestVectorInsertOperations:
    """Test vector insertion operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_insert_single_vector(self, mock_create_rest, mock_load_config):
        """Test inserting a single vector"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=InsertResult)
        mock_result.count = 1
        mock_result.duration_ms = 25.5
        mock_result.successful_ids = ["vec_1"]
        mock_rest_client.insert_vector.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        metadata = {"title": "test document", "category": "test"}
        
        result = client.insert_vector(
            collection_id="test_collection",
            vector_id="vec_1",
            vector=vector,
            metadata=metadata
        )
        
        mock_rest_client.insert_vector.assert_called_once_with(
            "test_collection", "vec_1", vector, metadata, False
        )
        assert result.count == 1
        assert result.duration_ms == 25.5
        assert "vec_1" in result.successful_ids
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_insert_vector_with_numpy(self, mock_create_rest, mock_load_config):
        """Test inserting vector with numpy array"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=InsertResult)
        mock_result.count = 1
        mock_rest_client.insert_vector.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Create numpy vector
        vector = np.array([0.1, 0.2, 0.3, 0.4, 0.5], dtype=np.float32)
        metadata = {"source": "numpy", "dimension": 5}
        
        result = client.insert_vector(
            collection_id="test_collection",
            vector_id="numpy_vec",
            vector=vector,
            metadata=metadata
        )
        
        mock_rest_client.insert_vector.assert_called_once_with(
            "test_collection", "numpy_vec", vector, metadata, False
        )
        assert result.count == 1
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_insert_vector_with_upsert(self, mock_create_rest, mock_load_config):
        """Test inserting vector with upsert option"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=InsertResult)
        mock_result.count = 1
        mock_result.upserted_count = 1
        mock_rest_client.insert_vector.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        result = client.insert_vector(
            collection_id="test_collection",
            vector_id="upsert_vec",
            vector=vector,
            upsert=True
        )
        
        mock_rest_client.insert_vector.assert_called_once_with(
            "test_collection", "upsert_vec", vector, None, True
        )
        assert result.upserted_count == 1
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_insert_batch_vectors(self, mock_create_rest, mock_load_config):
        """Test batch vector insertion"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=BatchResult)
        mock_result.successful_count = 3
        mock_result.failed_count = 0
        mock_result.total_time_ms = 150.0
        mock_result.successful_ids = ["vec_1", "vec_2", "vec_3"]
        mock_rest_client.insert_vectors.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vectors = [
            [0.1, 0.2, 0.3],
            [0.4, 0.5, 0.6],
            [0.7, 0.8, 0.9]
        ]
        ids = ["vec_1", "vec_2", "vec_3"]
        metadatas = [
            {"index": 1},
            {"index": 2},
            {"index": 3}
        ]
        
        result = client.insert_vectors(
            collection_id="test_collection",
            vectors=vectors,
            ids=ids,
            metadata=metadatas,
            batch_size=100
        )
        
        mock_rest_client.insert_vectors.assert_called_once_with(
            "test_collection", vectors, ids, metadatas, False, 100
        )
        assert result.successful_count == 3
        assert result.failed_count == 0
        assert len(result.successful_ids) == 3


class TestVectorSearchOperations:
    """Test vector search operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_basic_vector_search(self, mock_create_rest, mock_load_config):
        """Test basic vector search"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="vec_1", score=0.95, metadata={"title": "doc1"}),
            Mock(spec=SearchResult, id="vec_2", score=0.87, metadata={"title": "doc2"})
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        results = client.search(
            collection_id="test_collection",
            query=query_vector,
            k=10
        )
        
        mock_rest_client.search.assert_called_once_with(
            "test_collection", query_vector, 10, None, False, True, None, False, None
        )
        assert len(results) == 2
        assert results[0].id == "vec_1"
        assert results[0].score == 0.95
        assert results[1].id == "vec_2"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_with_filters(self, mock_create_rest, mock_load_config):
        """Test vector search with metadata filters"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="vec_1", score=0.95, metadata={"category": "A"})
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        filter_dict = {"category": "A", "priority": {"$gte": 5}}
        
        results = client.search(
            collection_id="test_collection",
            query=query_vector,
            k=5,
            filter=filter_dict,
            include_vectors=True,
            include_metadata=True
        )
        
        mock_rest_client.search.assert_called_once_with(
            "test_collection", query_vector, 5, filter_dict, True, True, None, False, None
        )
        assert len(results) == 1
        assert results[0].metadata["category"] == "A"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_with_advanced_options(self, mock_create_rest, mock_load_config):
        """Test vector search with advanced options"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [Mock(spec=SearchResult, id="vec_1", score=0.95)]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = np.array([0.1, 0.2, 0.3, 0.4, 0.5], dtype=np.float32)
        
        results = client.search(
            collection_id="test_collection",
            query=query_vector,
            k=20,
            ef=64,
            exact=True,
            timeout=30.0
        )
        
        mock_rest_client.search.assert_called_once_with(
            "test_collection", query_vector, 20, None, False, True, 64, True, 30.0
        )
        assert len(results) == 1


class TestVectorRetrievalOperations:
    """Test vector retrieval operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_get_vector_by_id(self, mock_create_rest, mock_load_config):
        """Test retrieving a vector by ID"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_vector_data = {
            "id": "vec_1",
            "vector": [0.1, 0.2, 0.3, 0.4, 0.5],
            "metadata": {"title": "test doc", "category": "A"}
        }
        mock_rest_client.get_vector.return_value = mock_vector_data
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.get_vector(
            collection_id="test_collection",
            vector_id="vec_1",
            include_vector=True,
            include_metadata=True
        )
        
        mock_rest_client.get_vector.assert_called_once_with(
            "test_collection", "vec_1", True, True
        )
        assert result["id"] == "vec_1"
        assert result["vector"] == [0.1, 0.2, 0.3, 0.4, 0.5]
        assert result["metadata"]["title"] == "test doc"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_get_vector_metadata_only(self, mock_create_rest, mock_load_config):
        """Test retrieving vector metadata only"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_vector_data = {
            "id": "vec_1",
            "metadata": {"title": "test doc", "category": "A"}
        }
        mock_rest_client.get_vector.return_value = mock_vector_data
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.get_vector(
            collection_id="test_collection",
            vector_id="vec_1",
            include_vector=False,
            include_metadata=True
        )
        
        mock_rest_client.get_vector.assert_called_once_with(
            "test_collection", "vec_1", False, True
        )
        assert "vector" not in result
        assert result["metadata"]["title"] == "test doc"


class TestVectorDeleteOperations:
    """Test vector deletion operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_delete_single_vector(self, mock_create_rest, mock_load_config):
        """Test deleting a single vector"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=DeleteResult)
        mock_result.deleted_count = 1
        mock_result.duration_ms = 15.0
        mock_result.deleted_ids = ["vec_1"]
        mock_rest_client.delete_vector.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.delete_vector(
            collection_id="test_collection",
            vector_id="vec_1"
        )
        
        mock_rest_client.delete_vector.assert_called_once_with(
            "test_collection", "vec_1"
        )
        assert result.deleted_count == 1
        assert result.duration_ms == 15.0
        assert "vec_1" in result.deleted_ids
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_delete_batch_vectors(self, mock_create_rest, mock_load_config):
        """Test batch vector deletion"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=DeleteResult)
        mock_result.deleted_count = 3
        mock_result.failed_count = 0
        mock_result.duration_ms = 45.0
        mock_result.deleted_ids = ["vec_1", "vec_2", "vec_3"]
        mock_rest_client.delete_vectors.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vector_ids = ["vec_1", "vec_2", "vec_3"]
        
        result = client.delete_vectors(
            collection_id="test_collection",
            vector_ids=vector_ids
        )
        
        mock_rest_client.delete_vectors.assert_called_once_with(
            "test_collection", vector_ids
        )
        assert result.deleted_count == 3
        assert result.failed_count == 0
        assert len(result.deleted_ids) == 3


class TestCollectionOperations:
    """Test collection CRUD operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_create_collection_basic(self, mock_create_rest, mock_load_config):
        """Test creating a basic collection"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collection = Mock()
        mock_collection.id = "test_collection_123"
        mock_collection.name = "test_collection"
        mock_collection.dimension = 768
        mock_collection.distance_metric = DistanceMetric.COSINE
        mock_rest_client.create_collection.return_value = mock_collection
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        config = CollectionConfig(
            dimension=768,
            distance_metric=DistanceMetric.COSINE,
            description="Test collection for embeddings"
        )
        
        result = client.create_collection("test_collection", config)
        
        mock_rest_client.create_collection.assert_called_once_with(
            "test_collection", config
        )
        assert result.name == "test_collection"
        assert result.dimension == 768
        assert result.distance_metric == DistanceMetric.COSINE
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_create_collection_with_advanced_config(self, mock_create_rest, mock_load_config):
        """Test creating collection with advanced configuration"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collection = Mock()
        mock_collection.name = "advanced_collection"
        mock_rest_client.create_collection.return_value = mock_collection
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        config = CollectionConfig(
            dimension=1024,
            distance_metric=DistanceMetric.EUCLIDEAN,
            description="Advanced collection with HNSW indexing",
            max_vectors=1000000,
            filterable_metadata_fields=["category", "type", "priority"]
        )
        
        result = client.create_collection("advanced_collection", config)
        
        mock_rest_client.create_collection.assert_called_once_with(
            "advanced_collection", config
        )
        assert result.name == "advanced_collection"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_get_collection_details(self, mock_create_rest, mock_load_config):
        """Test retrieving collection details"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collection = Mock()
        mock_collection.id = "test_collection_123"
        mock_collection.name = "test_collection"
        mock_collection.dimension = 768
        mock_collection.vector_count = 1500
        mock_collection.status = "active"
        mock_rest_client.get_collection.return_value = mock_collection
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.get_collection("test_collection")
        
        mock_rest_client.get_collection.assert_called_once_with("test_collection")
        assert result.name == "test_collection"
        assert result.vector_count == 1500
        assert result.status == "active"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_list_all_collections(self, mock_create_rest, mock_load_config):
        """Test listing all collections"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collections = [
            Mock(spec=['name', 'dimension', 'vector_count']),
            Mock(spec=['name', 'dimension', 'vector_count']),
            Mock(spec=['name', 'dimension', 'vector_count'])
        ]
        mock_collections[0].name = "collection_1"
        mock_collections[0].dimension = 768
        mock_collections[0].vector_count = 100
        mock_collections[1].name = "collection_2"
        mock_collections[1].dimension = 512
        mock_collections[1].vector_count = 250
        mock_collections[2].name = "collection_3"
        mock_collections[2].dimension = 1024
        mock_collections[2].vector_count = 75
        mock_rest_client.list_collections.return_value = mock_collections
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.list_collections()
        
        mock_rest_client.list_collections.assert_called_once()
        assert len(result) == 3
        assert result[0].name == "collection_1"
        assert result[1].vector_count == 250
        assert result[2].dimension == 1024
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_delete_collection(self, mock_create_rest, mock_load_config):
        """Test deleting a collection"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.delete_collection.return_value = True
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.delete_collection("test_collection")
        
        mock_rest_client.delete_collection.assert_called_once_with("test_collection")
        assert result is True


class TestVectorOperationErrors:
    """Test error handling in vector operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_vector_not_found_error(self, mock_create_rest, mock_load_config):
        """Test vector not found error handling"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.get_vector.side_effect = VectorNotFoundError("vec_999")
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        with pytest.raises(VectorNotFoundError):
            client.get_vector("test_collection", "vec_999")
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_vector_dimension_mismatch_error(self, mock_create_rest, mock_load_config):
        """Test vector dimension mismatch error"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.insert_vector.side_effect = VectorDimensionError(
            expected=768, actual=512
        )
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Wrong dimension vector
        wrong_vector = [0.1, 0.2, 0.3]  # Should be 768 dimensions
        
        with pytest.raises(VectorDimensionError):
            client.insert_vector("test_collection", "vec_1", wrong_vector)
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_validation_error_empty_vector(self, mock_create_rest, mock_load_config):
        """Test validation error for empty vector"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.insert_vector.side_effect = ValidationError(
            "Vector cannot be empty"
        )
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        with pytest.raises(ValidationError, match="Vector cannot be empty"):
            client.insert_vector("test_collection", "vec_1", [])


class TestAdvancedVectorOperations:
    """Test advanced vector operations and edge cases"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_large_batch_insert(self, mock_create_rest, mock_load_config):
        """Test inserting large batch of vectors"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=BatchResult)
        mock_result.successful_count = 1000
        mock_result.failed_count = 0
        mock_result.total_time_ms = 2500.0
        mock_rest_client.insert_vectors.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Generate large batch
        vectors = [[0.1 * i, 0.2 * i, 0.3 * i] for i in range(1000)]
        ids = [f"vec_{i}" for i in range(1000)]
        metadatas = [{"batch": "large", "index": i} for i in range(1000)]
        
        result = client.insert_vectors(
            collection_id="large_collection",
            vectors=vectors,
            ids=ids,
            metadata=metadatas,
            batch_size=100  # Process in smaller batches
        )
        
        mock_rest_client.insert_vectors.assert_called_once()
        assert result.successful_count == 1000
        assert result.failed_count == 0
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_with_complex_filters(self, mock_create_rest, mock_load_config):
        """Test search with complex metadata filters"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="vec_1", score=0.95),
            Mock(spec=SearchResult, id="vec_5", score=0.89)
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        complex_filter = {
            "$and": [
                {"category": {"$in": ["A", "B"]}},
                {"priority": {"$gte": 5}},
                {"timestamp": {"$between": ["2024-01-01", "2024-12-31"]}},
                {"$or": [
                    {"status": "active"},
                    {"featured": True}
                ]}
            ]
        }
        
        results = client.search(
            collection_id="complex_collection",
            query=query_vector,
            k=10,
            filter=complex_filter
        )
        
        mock_rest_client.search.assert_called_once_with(
            "complex_collection", query_vector, 10, complex_filter, False, True, None, False, None
        )
        assert len(results) == 2
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_high_dimensional_vectors(self, mock_create_rest, mock_load_config):
        """Test operations with high-dimensional vectors"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=InsertResult)
        mock_result.count = 1
        mock_rest_client.insert_vector.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # High-dimensional vector (e.g., from large language models)
        high_dim_vector = np.random.randn(4096).astype(np.float32)
        
        result = client.insert_vector(
            collection_id="high_dim_collection",
            vector_id="llm_embedding",
            vector=high_dim_vector,
            metadata={"model": "gpt-4", "dimension": 4096}
        )
        
        mock_rest_client.insert_vector.assert_called_once()
        assert result.count == 1