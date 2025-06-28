"""
Advanced search functionality tests for ProximaDB.
Tests complex search scenarios, filtering, and search optimizations.
"""

import pytest
import numpy as np
from unittest.mock import Mock, patch
from proximadb.unified_client import ProximaDBClient, Protocol
from proximadb.models import SearchResult, DistanceMetric
from proximadb.exceptions import ValidationError, TimeoutError, NetworkError


class TestAdvancedFiltering:
    """Test advanced metadata filtering in search operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_complex_metadata_filters(self, mock_create_rest, mock_load_config):
        """Test complex metadata filters with nested conditions"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="doc_1", score=0.95, metadata={"category": "A", "priority": 8}),
            Mock(spec=SearchResult, id="doc_2", score=0.89, metadata={"category": "B", "priority": 9})
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        # Complex nested filter
        complex_filter = {
            "$and": [
                {
                    "$or": [
                        {"category": {"$in": ["A", "B", "C"]}},
                        {"type": "premium"}
                    ]
                },
                {"priority": {"$gte": 7}},
                {"created_at": {"$gte": "2024-01-01", "$lt": "2024-12-31"}},
                {
                    "$not": {
                        "status": {"$in": ["deleted", "archived"]}
                    }
                }
            ]
        }
        
        results = client.search(
            collection_id="filtered_collection",
            query=query_vector,
            k=10,
            filter=complex_filter
        )
        
        mock_rest_client.search.assert_called_once_with(
            "filtered_collection", query_vector, 10, complex_filter, False, True, None, False, None
        )
        assert len(results) == 2
        assert all(result.metadata["priority"] >= 7 for result in results)
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_geospatial_filters(self, mock_create_rest, mock_load_config):
        """Test geospatial metadata filters"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, 
                 id="location_1", 
                 score=0.92,
                 metadata={"lat": 37.7749, "lon": -122.4194, "city": "San Francisco"}),
            Mock(spec=SearchResult,
                 id="location_2", 
                 score=0.88,
                 metadata={"lat": 37.8044, "lon": -122.2711, "city": "Oakland"})
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        # Geospatial bounding box filter
        geo_filter = {
            "lat": {"$gte": 37.7, "$lte": 37.9},
            "lon": {"$gte": -122.5, "$lte": -122.2},
            "$geoWithin": {
                "center": [37.7749, -122.4194],
                "radius_km": 50
            }
        }
        
        results = client.search(
            collection_id="locations_collection",
            query=query_vector,
            k=5,
            filter=geo_filter
        )
        
        assert len(results) == 2
        for result in results:
            assert 37.7 <= result.metadata["lat"] <= 37.9
            assert -122.5 <= result.metadata["lon"] <= -122.2
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_text_search_filters(self, mock_create_rest, mock_load_config):
        """Test text-based metadata filters"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, 
                 id="text_1", 
                 score=0.95,
                 metadata={"title": "Machine Learning with Python", "tags": ["ml", "python"]}),
            Mock(spec=SearchResult,
                 id="text_2", 
                 score=0.87,
                 metadata={"title": "Deep Learning Fundamentals", "tags": ["dl", "neural"]})
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        # Text search filters
        text_filter = {
            "$text": {
                "$search": "machine learning",
                "$caseSensitive": False
            },
            "tags": {"$all": ["ml"]},
            "title": {"$regex": ".*Learning.*", "$options": "i"}
        }
        
        results = client.search(
            collection_id="documents_collection",
            query=query_vector,
            k=10,
            filter=text_filter
        )
        
        assert len(results) == 2
        assert all("Learning" in result.metadata["title"] for result in results)


class TestSearchOptimization:
    """Test search performance optimizations and tuning"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_with_ef_parameter(self, mock_create_rest, mock_load_config):
        """Test search with ef parameter for HNSW tuning"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="opt_1", score=0.98),
            Mock(spec=SearchResult, id="opt_2", score=0.95),
            Mock(spec=SearchResult, id="opt_3", score=0.92)
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        # High ef for better recall
        results = client.search(
            collection_id="optimized_collection",
            query=query_vector,
            k=10,
            ef=200,  # Higher ef for better quality
            exact=False
        )
        
        mock_rest_client.search.assert_called_once_with(
            "optimized_collection", query_vector, 10, None, False, True, 200, False, None
        )
        assert len(results) == 3
        assert results[0].score == 0.98
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_exact_vs_approximate_search(self, mock_create_rest, mock_load_config):
        """Test exact vs approximate search modes"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # Mock exact search results (slower but precise)
        exact_results = [
            Mock(spec=SearchResult, id="exact_1", score=0.9876),
            Mock(spec=SearchResult, id="exact_2", score=0.9854)
        ]
        
        # Mock approximate search results (faster but less precise)
        approx_results = [
            Mock(spec=SearchResult, id="approx_1", score=0.9870),
            Mock(spec=SearchResult, id="approx_2", score=0.9850)
        ]
        
        mock_rest_client.search.side_effect = [exact_results, approx_results]
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        # Exact search
        exact_results = client.search(
            collection_id="precision_collection",
            query=query_vector,
            k=5,
            exact=True
        )
        
        # Approximate search
        approx_results = client.search(
            collection_id="precision_collection",
            query=query_vector,
            k=5,
            exact=False
        )
        
        assert len(exact_results) == 2
        assert len(approx_results) == 2
        assert mock_rest_client.search.call_count == 2
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_with_timeout(self, mock_create_rest, mock_load_config):
        """Test search with timeout configuration"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.search.side_effect = TimeoutError("Search timeout after 5000ms")
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        with pytest.raises(TimeoutError) as exc_info:
            client.search(
                collection_id="timeout_collection",
                query=query_vector,
                k=10,
                timeout=5.0  # 5 second timeout
            )
        
        assert "timeout" in str(exc_info.value).lower()
        mock_rest_client.search.assert_called_once_with(
            "timeout_collection", query_vector, 10, None, False, True, None, False, 5.0
        )


class TestVectorSimilarityMethods:
    """Test different vector similarity methods and distance metrics"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_cosine_similarity_search(self, mock_create_rest, mock_load_config):
        """Test search with cosine similarity"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="cosine_1", score=0.95, distance=0.05),
            Mock(spec=SearchResult, id="cosine_2", score=0.89, distance=0.11)
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Normalized vector for cosine similarity
        query_vector = np.array([0.1, 0.2, 0.3, 0.4, 0.5])
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        results = client.search(
            collection_id="cosine_collection",
            query=query_vector.tolist(),
            k=10
        )
        
        assert len(results) == 2
        assert results[0].score > results[1].score  # Higher score = better match
        assert hasattr(results[0], 'distance')
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_euclidean_distance_search(self, mock_create_rest, mock_load_config):
        """Test search with Euclidean distance"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="euclidean_1", score=0.92, distance=1.23),
            Mock(spec=SearchResult, id="euclidean_2", score=0.85, distance=2.45)
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [1.0, 2.0, 3.0, 4.0, 5.0]
        
        results = client.search(
            collection_id="euclidean_collection",
            query=query_vector,
            k=10
        )
        
        assert len(results) == 2
        assert results[0].distance < results[1].distance  # Lower distance = better match
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_dot_product_search(self, mock_create_rest, mock_load_config):
        """Test search with dot product similarity"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, id="dot_1", score=0.98, dot_product=15.7),
            Mock(spec=SearchResult, id="dot_2", score=0.91, dot_product=12.3)
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [2.0, 3.0, 1.5, 4.0, 2.5]
        
        results = client.search(
            collection_id="dot_product_collection",
            query=query_vector,
            k=5
        )
        
        assert len(results) == 2
        assert hasattr(results[0], 'dot_product')
        assert results[0].dot_product > results[1].dot_product


class TestSearchResultHandling:
    """Test search result processing and formatting"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_with_vector_inclusion(self, mock_create_rest, mock_load_config):
        """Test search results with vector data included"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, 
                 id="vec_1", 
                 score=0.95,
                 vector=[0.1, 0.2, 0.3, 0.4, 0.5],
                 metadata={"title": "Document 1"}),
            Mock(spec=SearchResult,
                 id="vec_2", 
                 score=0.87,
                 vector=[0.2, 0.3, 0.4, 0.5, 0.6],
                 metadata={"title": "Document 2"})
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.15, 0.25, 0.35, 0.45, 0.55]
        
        results = client.search(
            collection_id="vector_collection",
            query=query_vector,
            k=10,
            include_vectors=True,
            include_metadata=True
        )
        
        mock_rest_client.search.assert_called_once_with(
            "vector_collection", query_vector, 10, None, True, True, None, False, None
        )
        assert len(results) == 2
        assert hasattr(results[0], 'vector')
        assert len(results[0].vector) == 5
        assert hasattr(results[0], 'metadata')
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_metadata_only(self, mock_create_rest, mock_load_config):
        """Test search with metadata only (no vectors)"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [
            Mock(spec=SearchResult, 
                 id="meta_1", 
                 score=0.93,
                 metadata={"title": "Doc 1", "category": "A", "size": 1024}),
            Mock(spec=SearchResult,
                 id="meta_2", 
                 score=0.88,
                 metadata={"title": "Doc 2", "category": "B", "size": 2048})
        ]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        results = client.search(
            collection_id="metadata_collection",
            query=query_vector,
            k=10,
            include_vectors=False,
            include_metadata=True
        )
        
        assert len(results) == 2
        assert not hasattr(results[0], 'vector')
        assert hasattr(results[0], 'metadata')
        assert results[0].metadata["title"] == "Doc 1"
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_empty_search_results(self, mock_create_rest, mock_load_config):
        """Test handling of empty search results"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.search.return_value = []
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.9, 0.8, 0.7, 0.6, 0.5]  # Very different vector
        
        results = client.search(
            collection_id="sparse_collection",
            query=query_vector,
            k=10
        )
        
        assert len(results) == 0
        assert isinstance(results, list)


class TestSearchErrorHandling:
    """Test error handling in search operations"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_invalid_vector_dimension_search(self, mock_create_rest, mock_load_config):
        """Test search with invalid vector dimensions"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        from proximadb.exceptions import VectorDimensionError
        mock_rest_client.search.side_effect = VectorDimensionError(expected=768, actual=512)
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Wrong dimension vector
        wrong_vector = [0.1] * 512  # Should be 768
        
        with pytest.raises(VectorDimensionError):
            client.search("test_collection", wrong_vector, k=10)
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_invalid_filter_syntax(self, mock_create_rest, mock_load_config):
        """Test search with invalid filter syntax"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.search.side_effect = ValidationError(
            "Invalid filter syntax: '$invalidOperator' is not supported"
        )
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        invalid_filter = {"field": {"$invalidOperator": "value"}}
        
        with pytest.raises(ValidationError) as exc_info:
            client.search(
                collection_id="test_collection",
                query=query_vector,
                k=10,
                filter=invalid_filter
            )
        
        assert "invalidOperator" in str(exc_info.value)
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_network_error(self, mock_create_rest, mock_load_config):
        """Test search with network connectivity issues"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.search.side_effect = NetworkError(
            "Connection failed: Unable to reach server"
        )
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        with pytest.raises(NetworkError) as exc_info:
            client.search("test_collection", query_vector, k=10)
        
        assert "Connection failed" in str(exc_info.value)


class TestSearchPagination:
    """Test search result pagination and large result handling"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_paginated_search_results(self, mock_create_rest, mock_load_config):
        """Test paginated search for large result sets"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        
        # First page
        page1_results = [Mock(spec=SearchResult, id=f"doc_{i}", score=0.9-i*0.01) for i in range(10)]
        # Second page
        page2_results = [Mock(spec=SearchResult, id=f"doc_{i}", score=0.8-i*0.01) for i in range(10, 20)]
        
        mock_rest_client.search.side_effect = [page1_results, page2_results]
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        query_vector = [0.1, 0.2, 0.3, 0.4, 0.5]
        
        # Mock paginated search method if available
        if hasattr(client, 'search_paginated'):
            all_results = []
            page = 0
            page_size = 10
            
            while True:
                results = client.search_paginated(
                    collection_id="large_collection",
                    query=query_vector,
                    k=page_size,
                    offset=page * page_size
                )
                
                if not results:
                    break
                    
                all_results.extend(results)
                page += 1
                
                if page >= 2:  # Limit for test
                    break
            
            assert len(all_results) == 20
        else:
            # Fallback to multiple searches
            results1 = client.search("large_collection", query_vector, k=10)
            results2 = client.search("large_collection", query_vector, k=10)
            
            assert len(results1) == 10
            assert len(results2) == 10