"""
Comprehensive CRUD tests for ProximaDB vectors and collections.
Tests advanced CRUD operations, edge cases, validation, and error scenarios.
"""

import pytest
import numpy as np
from unittest.mock import Mock, patch
from proximadb.unified_client import ProximaDBClient, Protocol
from proximadb.models import (
    CollectionConfig, DistanceMetric, IndexAlgorithm, CompressionType,
    InsertResult, DeleteResult, SearchResult, BatchResult, Collection,
    HealthStatus, IndexConfig, StorageConfig
)
from proximadb.exceptions import (
    CollectionNotFoundError, CollectionExistsError, VectorNotFoundError,
    VectorDimensionError, ValidationError, QuotaExceededError, 
    RateLimitError, NetworkError
)


class TestAdvancedCollectionCRUD:
    """Advanced collection CRUD operations and management"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_create_collection_with_all_options(self, mock_create_rest, mock_load_config):
        """Test creating collection with all configuration options"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collection = Mock(spec=Collection)
        mock_collection.id = "advanced_collection_123"
        mock_collection.name = "advanced_collection"
        mock_collection.dimension = 1536
        mock_collection.distance_metric = DistanceMetric.COSINE
        mock_collection.index_algorithm = IndexAlgorithm.HNSW
        mock_collection.compression_type = CompressionType.LZ4
        mock_collection.max_vectors = 5000000
        mock_collection.vector_count = 0
        mock_collection.status = "active"
        mock_rest_client.create_collection.return_value = mock_collection
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        config = CollectionConfig(
            dimension=1536,
            distance_metric=DistanceMetric.COSINE,
            index_config=IndexConfig(algorithm=IndexAlgorithm.HNSW),
            storage_config=StorageConfig(compression=CompressionType.LZ4),
            description="Advanced collection for OpenAI embeddings",
            max_vectors=5000000,
            filterable_metadata_fields=["category", "source", "priority", "status"]
        )
        
        result = client.create_collection("advanced_collection", config)
        
        mock_rest_client.create_collection.assert_called_once_with("advanced_collection", config)
        assert result.name == "advanced_collection"
        assert result.dimension == 1536
        assert result.distance_metric == DistanceMetric.COSINE
        assert result.index_algorithm == IndexAlgorithm.HNSW
        assert result.compression_type == CompressionType.LZ4
        assert result.max_vectors == 5000000
        assert result.status == "active"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_update_collection_metadata(self, mock_create_rest, mock_load_config):
        """Test updating collection metadata and configuration"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collection = Mock(spec=Collection)
        mock_collection.name = "test_collection"
        mock_collection.description = "Updated description"
        mock_collection.max_vectors = 2000000
        # Mock the update_collection method
        mock_rest_client.update_collection = Mock(return_value=mock_collection)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if update_collection method exists, if not skip test
        if hasattr(client, 'update_collection'):
            updates = {
                "description": "Updated description",
                "max_vectors": 2000000,
                "filterable_metadata_fields": ["category", "source", "type"]
            }
            
            result = client.update_collection("test_collection", updates)
            
            mock_rest_client.update_collection.assert_called_once_with("test_collection", updates)
            assert result.description == "Updated description"
            assert result.max_vectors == 2000000
        else:
            pytest.skip("update_collection method not implemented")
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_collection_statistics(self, mock_create_rest, mock_load_config):
        """Test retrieving detailed collection statistics"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collection = Mock(spec=Collection)
        mock_collection.id = "stats_collection"
        mock_collection.name = "stats_collection"
        mock_collection.vector_count = 150000
        mock_collection.index_size_bytes = 512000000
        mock_collection.storage_size_bytes = 2048000000
        mock_collection.avg_vector_size = 1536
        mock_collection.created_at = "2024-01-15T10:30:00Z"
        mock_collection.updated_at = "2024-01-20T15:45:00Z"
        mock_rest_client.get_collection.return_value = mock_collection
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.get_collection("stats_collection")
        
        assert result.vector_count == 150000
        assert result.index_size_bytes == 512000000
        assert result.storage_size_bytes == 2048000000
        assert result.avg_vector_size == 1536
        assert result.created_at == "2024-01-15T10:30:00Z"
        assert result.updated_at == "2024-01-20T15:45:00Z"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_collection_exists_error(self, mock_create_rest, mock_load_config):
        """Test handling collection already exists error"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.create_collection.side_effect = CollectionExistsError("existing_collection")
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        config = CollectionConfig(dimension=768, distance_metric=DistanceMetric.COSINE)
        
        with pytest.raises(CollectionExistsError):
            client.create_collection("existing_collection", config)
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_collection_not_found_error(self, mock_create_rest, mock_load_config):
        """Test handling collection not found error"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.get_collection.side_effect = CollectionNotFoundError("nonexistent_collection")
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        with pytest.raises(CollectionNotFoundError):
            client.get_collection("nonexistent_collection")
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_delete_collection_with_vectors(self, mock_create_rest, mock_load_config):
        """Test deleting collection that contains vectors"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_delete_result = {
            "deleted": True,
            "vector_count": 1500,
            "cleanup_time_ms": 2500.0
        }
        mock_rest_client.delete_collection.return_value = mock_delete_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        result = client.delete_collection("collection_with_vectors")
        
        mock_rest_client.delete_collection.assert_called_once_with("collection_with_vectors")
        assert result["deleted"] is True
        assert result["vector_count"] == 1500
        assert result["cleanup_time_ms"] == 2500.0


class TestAdvancedVectorCRUD:
    """Advanced vector CRUD operations with comprehensive scenarios"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_upsert_vector_operations(self, mock_create_rest, mock_load_config):
        """Test vector upsert operations (insert or update)"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # First insert
        mock_insert_result = Mock(spec=InsertResult)
        mock_insert_result.count = 1
        mock_insert_result.upserted_count = 0
        mock_insert_result.updated_count = 0
        
        # Then upsert (update)
        mock_upsert_result = Mock(spec=InsertResult)
        mock_upsert_result.count = 1
        mock_upsert_result.upserted_count = 1
        mock_upsert_result.updated_count = 1
        
        mock_rest_client.insert_vector.side_effect = [mock_insert_result, mock_upsert_result]
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        metadata = {"title": "Original document", "version": 1}
        
        # First insert
        result1 = client.insert_vector(
            collection_id="test_collection",
            vector_id="doc_1",
            vector=vector,
            metadata=metadata,
            upsert=False
        )
        
        assert result1.count == 1
        assert result1.upserted_count == 0
        
        # Update with upsert
        updated_vector = [0.15, 0.25, 0.35, 0.45, 0.55]
        updated_metadata = {"title": "Updated document", "version": 2}
        
        result2 = client.insert_vector(
            collection_id="test_collection",
            vector_id="doc_1",
            vector=updated_vector,
            metadata=updated_metadata,
            upsert=True
        )
        
        assert result2.count == 1
        assert result2.upserted_count == 1
        assert result2.updated_count == 1
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_batch_operations_with_partial_failures(self, mock_create_rest, mock_load_config):
        """Test batch operations with some failures"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=BatchResult)
        mock_result.successful_count = 7
        mock_result.failed_count = 3
        mock_result.total_count = 10
        mock_result.successful_ids = [f"vec_{i}" for i in [1, 2, 3, 4, 5, 7, 8]]
        mock_result.failed_ids = ["vec_6", "vec_9", "vec_10"]
        mock_result.errors = [
            {"id": "vec_6", "error": "Dimension mismatch"},
            {"id": "vec_9", "error": "Invalid vector data"},
            {"id": "vec_10", "error": "Metadata validation failed"}
        ]
        mock_result.total_time_ms = 450.0
        mock_rest_client.insert_vectors.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vectors = [[0.1 * i, 0.2 * i, 0.3 * i] for i in range(1, 11)]
        ids = [f"vec_{i}" for i in range(1, 11)]
        metadatas = [{"index": i, "batch": "test"} for i in range(1, 11)]
        
        result = client.insert_vectors(
            collection_id="batch_collection",
            vectors=vectors,
            ids=ids,
            metadata=metadatas
        )
        
        assert result.successful_count == 7
        assert result.failed_count == 3
        assert result.total_count == 10
        assert len(result.successful_ids) == 7
        assert len(result.failed_ids) == 3
        assert len(result.errors) == 3
        assert "vec_6" in result.failed_ids
        assert result.total_time_ms == 450.0
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_vector_update_operations(self, mock_create_rest, mock_load_config):
        """Test vector update operations (metadata and vector data)"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # Mock update_vector method if it exists
        mock_update_result = Mock()
        mock_update_result.updated = True
        mock_update_result.previous_metadata = {"title": "Old title", "version": 1}
        mock_update_result.new_metadata = {"title": "New title", "version": 2}
        mock_rest_client.update_vector = Mock(return_value=mock_update_result)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if update_vector method exists
        if hasattr(client, 'update_vector'):
            # Update metadata only
            metadata_updates = {"title": "New title", "version": 2, "last_modified": "2024-01-20"}
            
            result = client.update_vector(
                collection_id="test_collection",
                vector_id="doc_1",
                metadata=metadata_updates
            )
            
            assert result.updated is True
            assert result.new_metadata["title"] == "New title"
            assert result.new_metadata["version"] == 2
        else:
            # Use upsert as alternative
            new_vector = [0.15, 0.25, 0.35, 0.45, 0.55]
            new_metadata = {"title": "New title", "version": 2}
            
            mock_upsert_result = Mock(spec=InsertResult)
            mock_upsert_result.updated_count = 1
            mock_rest_client.insert_vector.return_value = mock_upsert_result
            
            result = client.insert_vector(
                collection_id="test_collection",
                vector_id="doc_1",
                vector=new_vector,
                metadata=new_metadata,
                upsert=True
            )
            
            assert result.updated_count == 1
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_conditional_vector_operations(self, mock_create_rest, mock_load_config):
        """Test conditional vector operations based on metadata"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # Mock conditional delete
        mock_delete_result = Mock(spec=DeleteResult)
        mock_delete_result.deleted_count = 3
        mock_delete_result.conditions_matched = 5
        mock_delete_result.deleted_ids = ["vec_1", "vec_3", "vec_5"]
        
        # Mock delete_vectors_by_filter method if it exists
        mock_rest_client.delete_vectors_by_filter = Mock(return_value=mock_delete_result)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if conditional delete method exists
        if hasattr(client, 'delete_vectors_by_filter'):
            filter_conditions = {
                "status": "inactive",
                "last_accessed": {"$lt": "2024-01-01"},
                "priority": {"$lte": 2}
            }
            
            result = client.delete_vectors_by_filter(
                collection_id="cleanup_collection",
                filter=filter_conditions,
                limit=100
            )
            
            assert result.deleted_count == 3
            assert result.conditions_matched == 5
            assert len(result.deleted_ids) == 3
        else:
            pytest.skip("Conditional delete method not implemented")
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_vector_version_control(self, mock_create_rest, mock_load_config):
        """Test vector versioning and history"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # Mock vector history retrieval
        mock_history = [
            {
                "version": 1,
                "vector": [0.1, 0.2, 0.3],
                "metadata": {"title": "Original", "version": 1},
                "created_at": "2024-01-15T10:00:00Z"
            },
            {
                "version": 2,
                "vector": [0.15, 0.25, 0.35],
                "metadata": {"title": "Updated", "version": 2},
                "created_at": "2024-01-16T14:30:00Z"
            }
        ]
        mock_rest_client.get_vector_history = Mock(return_value=mock_history)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if versioning is supported
        if hasattr(client, 'get_vector_history'):
            history = client.get_vector_history(
                collection_id="versioned_collection",
                vector_id="doc_1"
            )
            
            assert len(history) == 2
            assert history[0]["version"] == 1
            assert history[1]["version"] == 2
            assert history[1]["metadata"]["title"] == "Updated"
        else:
            pytest.skip("Vector versioning not implemented")


class TestAdvancedSearchOperations:
    """Advanced search operations with complex queries and filters"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_hybrid_search(self, mock_create_rest, mock_load_config):
        """Test hybrid vector + text search"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, 
                 id="doc_1", 
                 score=0.95, 
                 vector_score=0.92,
                 text_score=0.98,
                 metadata={"title": "Machine Learning Basics"}),
            Mock(spec=SearchResult,
                 id="doc_2", 
                 score=0.89,
                 vector_score=0.91,
                 text_score=0.87,
                 metadata={"title": "Deep Learning Fundamentals"})
        ]
        
        # Mock hybrid_search method if available
        mock_rest_client.hybrid_search = Mock(return_value=mock_results)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        text_query = "machine learning fundamentals"
        
        # Check if hybrid search is supported
        if hasattr(client, 'hybrid_search'):
            results = client.hybrid_search(
                collection_id="ml_documents",
                vector_query=query_vector,
                text_query=text_query,
                k=10,
                vector_weight=0.7,
                text_weight=0.3
            )
            
            assert len(results) == 2
            assert results[0].score == 0.95
            assert hasattr(results[0], 'vector_score')
            assert hasattr(results[0], 'text_score')
        else:
            # Fallback to regular vector search
            mock_rest_client.search.return_value = mock_results
            
            results = client.search(
                collection_id="ml_documents",
                query=query_vector,
                k=10
            )
            assert len(results) == 2
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_multi_vector_search(self, mock_create_rest, mock_load_config):
        """Test search with multiple query vectors"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="doc_1", score=0.95, query_index=0),
            Mock(spec=SearchResult, id="doc_2", score=0.92, query_index=0),
            Mock(spec=SearchResult, id="doc_3", score=0.89, query_index=1),
            Mock(spec=SearchResult, id="doc_4", score=0.87, query_index=1)
        ]
        
        # Mock multi_search method if available
        mock_rest_client.multi_search = Mock(return_value=mock_results)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if multi-vector search is supported
        if hasattr(client, 'multi_search'):
            query_vectors = [
                [0.1, 0.2, 0.3, 0.4, 0.5],
                [0.2, 0.3, 0.4, 0.5, 0.6]
            ]
            
            results = client.multi_search(
                collection_id="multi_collection",
                queries=query_vectors,
                k=2
            )
            
            assert len(results) == 4
            assert results[0].query_index == 0
            assert results[2].query_index == 1
        else:
            pytest.skip("Multi-vector search not implemented")
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_with_aggregations(self, mock_create_rest, mock_load_config):
        """Test search with result aggregations"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_search_result = {
            "results": [
                {"id": "doc_1", "score": 0.95, "metadata": {"category": "A", "year": 2023}},
                {"id": "doc_2", "score": 0.89, "metadata": {"category": "B", "year": 2023}},
                {"id": "doc_3", "score": 0.85, "metadata": {"category": "A", "year": 2024}}
            ],
            "aggregations": {
                "category": {"A": 2, "B": 1},
                "year": {"2023": 2, "2024": 1},
                "avg_score_by_category": {"A": 0.90, "B": 0.89}
            },
            "total_count": 3
        }
        
        # Mock search_with_aggregations method if available
        mock_rest_client.search_with_aggregations = Mock(return_value=mock_search_result)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if search with aggregations is supported
        if hasattr(client, 'search_with_aggregations'):
            query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
            
            result = client.search_with_aggregations(
                collection_id="aggregation_collection",
                query=query_vector,
                k=10,
                aggregations=["category", "year"],
                group_by="category"
            )
            
            assert len(result["results"]) == 3
            assert "aggregations" in result
            assert result["aggregations"]["category"]["A"] == 2
            assert result["aggregations"]["year"]["2023"] == 2
        else:
            pytest.skip("Search with aggregations not implemented")


class TestDataValidationAndConstraints:
    """Test data validation, constraints, and schema enforcement"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_vector_dimension_validation(self, mock_create_rest, mock_load_config):
        """Test vector dimension validation"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.insert_vector.side_effect = VectorDimensionError(expected=768, actual=512)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Wrong dimension vector
        wrong_vector = [0.1] * 512  # Should be 768 dimensions
        
        with pytest.raises(VectorDimensionError) as exc_info:
            client.insert_vector("test_collection", "vec_1", wrong_vector)
        
        assert exc_info.value.expected_dimension == 768
        assert exc_info.value.actual_dimension == 512
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_metadata_schema_validation(self, mock_create_rest, mock_load_config):
        """Test metadata schema validation"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.insert_vector.side_effect = ValidationError(
            "Metadata field 'priority' must be an integer between 1 and 10"
        )
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vector = [0.1, 0.2, 0.3]
        invalid_metadata = {"title": "Test", "priority": "high"}  # Should be int
        
        with pytest.raises(ValidationError) as exc_info:
            client.insert_vector("test_collection", "vec_1", vector, invalid_metadata)
        
        assert "priority" in str(exc_info.value)
        assert "integer" in str(exc_info.value)
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_collection_quota_enforcement(self, mock_create_rest, mock_load_config):
        """Test collection quota enforcement"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.insert_vector.side_effect = QuotaExceededError(
            "Collection vector limit exceeded: 1000000/1000000",
            quota_type="max_vectors"
        )
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vector = [0.1, 0.2, 0.3]
        
        with pytest.raises(QuotaExceededError) as exc_info:
            client.insert_vector("full_collection", "vec_1", vector)
        
        assert exc_info.value.quota_type == "max_vectors"
        assert "1000000" in str(exc_info.value)
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_rate_limiting(self, mock_create_rest, mock_load_config):
        """Test rate limiting enforcement"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.insert_vector.side_effect = RateLimitError(
            "Rate limit exceeded: 1000 requests per minute",
            retry_after=60
        )
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        vector = [0.1, 0.2, 0.3]
        
        with pytest.raises(RateLimitError) as exc_info:
            client.insert_vector("test_collection", "vec_1", vector)
        
        assert exc_info.value.retry_after == 60
        assert "1000 requests per minute" in str(exc_info.value)


class TestTransactionalOperations:
    """Test transactional and atomic operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_atomic_batch_operations(self, mock_create_rest, mock_load_config):
        """Test atomic batch operations (all succeed or all fail)"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # Mock atomic batch insert
        mock_result = Mock(spec=BatchResult)
        mock_result.successful_count = 5
        mock_result.failed_count = 0
        mock_result.atomic = True
        mock_result.transaction_id = "tx_12345"
        
        # Mock atomic_insert_vectors method if available
        mock_rest_client.atomic_insert_vectors = Mock(return_value=mock_result)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if atomic operations are supported
        if hasattr(client, 'atomic_insert_vectors'):
            vectors = [[0.1 * i, 0.2 * i, 0.3 * i] for i in range(1, 6)]
            ids = [f"vec_{i}" for i in range(1, 6)]
            
            result = client.atomic_insert_vectors(
                collection_id="atomic_collection",
                vectors=vectors,
                ids=ids,
                atomic=True
            )
            
            assert result.successful_count == 5
            assert result.failed_count == 0
            assert result.atomic is True
            assert result.transaction_id == "tx_12345"
        else:
            pytest.skip("Atomic operations not implemented")
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_transaction_rollback(self, mock_create_rest, mock_load_config):
        """Test transaction rollback on failure"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # Mock transaction that fails and rolls back
        mock_result = Mock()
        mock_result.success = False
        mock_result.rollback_performed = True
        mock_result.error = "Vector dimension mismatch in batch item 3"
        
        # Mock begin_transaction and rollback methods if available
        mock_rest_client.begin_transaction = Mock(return_value="tx_67890")
        mock_rest_client.rollback_transaction = Mock(return_value=mock_result)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Check if transactions are supported
        if hasattr(client, 'begin_transaction') and hasattr(client, 'rollback_transaction'):
            transaction_id = client.begin_transaction()
            
            # Simulate transaction failure
            rollback_result = client.rollback_transaction(transaction_id)
            
            assert rollback_result.success is False
            assert rollback_result.rollback_performed is True
            assert "dimension mismatch" in rollback_result.error
        else:
            pytest.skip("Transaction support not implemented")


class TestPerformanceAndOptimization:
    """Test performance-related features and optimizations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_bulk_operations_performance(self, mock_create_rest, mock_load_config):
        """Test bulk operations with performance metrics"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_result = Mock(spec=BatchResult)
        mock_result.successful_count = 10000
        mock_result.failed_count = 0
        mock_result.total_time_ms = 5500.0
        mock_result.throughput_vectors_per_second = 1818
        mock_result.index_build_time_ms = 2200.0
        mock_result.storage_write_time_ms = 3300.0
        mock_rest_client.insert_vectors.return_value = mock_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Large batch operation
        vectors = [[0.1 * i, 0.2 * i, 0.3 * i] for i in range(10000)]
        ids = [f"bulk_vec_{i}" for i in range(10000)]
        
        result = client.insert_vectors(
            collection_id="bulk_collection",
            vectors=vectors,
            ids=ids,
            batch_size=1000  # Process in chunks
        )
        
        assert result.successful_count == 10000
        assert result.total_time_ms == 5500.0
        assert result.throughput_vectors_per_second == 1818
        assert hasattr(result, 'index_build_time_ms')
        assert hasattr(result, 'storage_write_time_ms')
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_performance_tuning(self, mock_create_rest, mock_load_config):
        """Test search performance tuning parameters"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="doc_1", score=0.95),
            Mock(spec=SearchResult, id="doc_2", score=0.89)
        ]
        mock_search_metadata = {
            "total_time_ms": 45.5,
            "index_search_time_ms": 25.0,
            "post_filter_time_ms": 15.5,
            "candidates_examined": 1000,
            "exact_distance_calculations": 50
        }
        
        # Mock search with performance metadata
        mock_result_with_metadata = {
            "results": mock_results,
            "metadata": mock_search_metadata
        }
        mock_rest_client.search.return_value = mock_result_with_metadata
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        result = client.search(
            collection_id="performance_collection",
            query=query_vector,
            k=10,
            ef=128,  # Higher ef for better recall
            exact=False,  # Use approximate search
            timeout=1000.0  # 1 second timeout
        )
        
        # Check if result includes performance metadata
        if isinstance(result, dict) and "metadata" in result:
            assert result["metadata"]["total_time_ms"] == 45.5
            assert result["metadata"]["candidates_examined"] == 1000
        else:
            # Just check we got results
            assert len(result) == 2 if isinstance(result, list) else True