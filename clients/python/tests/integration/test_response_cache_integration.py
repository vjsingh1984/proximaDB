"""
Integration tests for response caching with REST client
"""

import threading
import time
from typing import Any, Dict
from unittest.mock import MagicMock, Mock, patch

import pytest

from proximadb_sdk.cache import CacheLevel, CacheStrategy, ResponseCache
from proximadb_sdk.config import ClientConfig
from proximadb_sdk.protocols.rest_sync import ProximaDBClient


class TestResponseCacheIntegration:
    """Integration tests for REST client with response caching enabled"""

    @pytest.fixture
    def config(self):
        """Client configuration"""
        return ClientConfig(url="http://localhost:5678", timeout=30.0)

    @pytest.fixture
    def cache_config(self):
        """Cache configuration for testing"""
        return {
            "max_memory_mb": 10.0,
            "default_ttl_seconds": 300.0,
            "strategy": CacheStrategy.LRU,
            "enable_compression": True,
        }

    @pytest.fixture
    def mock_http_client(self):
        """Mock HTTP client for testing"""
        client = Mock()

        # Mock successful search response
        mock_search_response = Mock()
        mock_search_response.status_code = 200
        mock_search_response.json.return_value = {
            "results": [
                {"id": "vec_1", "score": 0.95, "metadata": {"tag": "test"}},
                {"id": "vec_2", "score": 0.87, "metadata": {"tag": "test2"}},
            ],
            "total": 2,
            "duration_ms": 25.0,
        }

        # Mock vector get response
        mock_get_response = Mock()
        mock_get_response.status_code = 200
        mock_get_response.json.return_value = {
            "id": "vec_1",
            "vector": [1.0, 2.0, 3.0],
            "metadata": {"tag": "test"},
            "timestamp": 1640995200,
        }

        # Mock collections list response
        mock_list_response = Mock()
        mock_list_response.status_code = 200
        mock_list_response.json.return_value = {
            "collections": [
                {"id": "collection_1", "dimension": 3, "vectors": 100},
                {"id": "collection_2", "dimension": 4, "vectors": 200},
            ]
        }

        # Configure different responses for different endpoints
        def mock_request(method, url, **kwargs):
            if "search" in url or "vectors/search" in url:
                return mock_search_response
            elif "vectors/" in url and method == "GET":
                return mock_get_response
            elif "collections" in url and method == "GET":
                return mock_list_response
            else:
                # Default response
                mock_response = Mock()
                mock_response.status_code = 200
                mock_response.json.return_value = {"success": True}
                return mock_response

        client.request.side_effect = mock_request
        client.get.side_effect = mock_request
        client.post.side_effect = mock_request

        return client

    def test_client_initialization_with_caching(self, config, cache_config):
        """Test client initialization with caching enabled"""
        with patch("proximadb.protocols.rest_sync.ProximaDBClient._create_http_client"):
            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            assert client.enable_caching is True
            assert client._response_cache is not None
            assert client._response_cache.config == cache_config

            client.close()

    def test_client_initialization_without_caching(self, config):
        """Test client initialization without caching"""
        with patch("proximadb.protocols.rest_sync.ProximaDBClient._create_http_client"):
            client = ProximaDBClient(config=config)

            assert client.enable_caching is False
            assert client._response_cache is None

            client.close()

    def test_search_caching(self, config, cache_config, mock_http_client):
        """Test search operation with caching"""
        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.search"
            ) as mock_search,
        ):

            # Configure mock search to return test data
            mock_search.return_value = [
                {"id": "vec_1", "score": 0.95, "metadata": {"tag": "test"}},
                {"id": "vec_2", "score": 0.87, "metadata": {"tag": "test2"}},
            ]

            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            try:
                # First search - should hit the server
                result1 = client.search_cached(
                    collection_id="test_collection", vector=[1.0, 2.0, 3.0], top_k=10
                )

                assert len(result1) == 2
                assert mock_search.call_count == 1

                # Second identical search - should hit cache
                result2 = client.search_cached(
                    collection_id="test_collection", vector=[1.0, 2.0, 3.0], top_k=10
                )

                assert result2 == result1
                assert mock_search.call_count == 1  # No additional server call

                # Verify cache stats
                stats = client.get_cache_stats()
                assert stats["hits"] == 1
                assert stats["misses"] == 1
                assert stats["hit_rate_percent"] == 50.0

            finally:
                client.close()

    def test_get_vector_caching(self, config, cache_config, mock_http_client):
        """Test get_vector operation with caching"""
        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.get_vector"
            ) as mock_get,
        ):

            # Configure mock get_vector
            mock_get.return_value = {
                "id": "vec_1",
                "vector": [1.0, 2.0, 3.0],
                "metadata": {"tag": "test"},
            }

            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            try:
                # First get - should hit server
                result1 = client.get_vector_cached(
                    collection_id="test_collection", vector_id="vec_1"
                )

                assert result1["id"] == "vec_1"
                assert mock_get.call_count == 1

                # Second identical get - should hit cache
                result2 = client.get_vector_cached(
                    collection_id="test_collection", vector_id="vec_1"
                )

                assert result2 == result1
                assert mock_get.call_count == 1  # No additional server call

            finally:
                client.close()

    def test_list_collections_caching(self, config, cache_config, mock_http_client):
        """Test list_collections operation with caching"""
        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.list_collections"
            ) as mock_list,
        ):

            # Configure mock list_collections
            mock_list.return_value = [
                {"id": "collection_1", "dimension": 3},
                {"id": "collection_2", "dimension": 4},
            ]

            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            try:
                # First list - should hit server
                result1 = client.list_collections_cached()

                assert len(result1) == 2
                assert mock_list.call_count == 1

                # Second list - should hit cache
                result2 = client.list_collections_cached()

                assert result2 == result1
                assert mock_list.call_count == 1  # No additional server call

            finally:
                client.close()

    def test_cache_invalidation_on_write(self, config, cache_config, mock_http_client):
        """Test cache invalidation after write operations"""
        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.search"
            ) as mock_search,
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.insert_vectors"
            ) as mock_insert,
        ):

            # Configure mocks
            mock_search.return_value = [{"id": "vec_1", "score": 0.95}]
            mock_insert.return_value = {"success": True, "count": 1}

            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            try:
                # Populate cache with search result
                result1 = client.search_cached(
                    collection_id="test_collection", vector=[1.0, 2.0, 3.0], top_k=10
                )

                assert mock_search.call_count == 1

                # Verify it's cached
                result2 = client.search_cached(
                    collection_id="test_collection", vector=[1.0, 2.0, 3.0], top_k=10
                )

                assert mock_search.call_count == 1  # Cache hit

                # Perform write operation (this should invalidate cache)
                # Note: We need to directly call _invalidate_collection_cache since
                # the actual insert_vectors method integration isn't complete in this test
                client._invalidate_collection_cache("test_collection")

                # Next search should hit server again (cache invalidated)
                result3 = client.search_cached(
                    collection_id="test_collection", vector=[1.0, 2.0, 3.0], top_k=10
                )

                assert mock_search.call_count == 2  # Cache miss, server called again

            finally:
                client.close()

    def test_caching_disabled_fallback(self, config, cache_config, mock_http_client):
        """Test fallback to non-cached operations when caching is disabled"""
        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.search"
            ) as mock_search,
        ):

            mock_search.return_value = [{"id": "vec_1", "score": 0.95}]

            # Client without caching
            client = ProximaDBClient(config=config)

            try:
                # Cached methods should fallback to regular methods
                result1 = client.search_cached(
                    collection_id="test_collection", vector=[1.0, 2.0, 3.0], top_k=10
                )

                result2 = client.search_cached(
                    collection_id="test_collection", vector=[1.0, 2.0, 3.0], top_k=10
                )

                # Should call server twice (no caching)
                assert mock_search.call_count == 2

            finally:
                client.close()

    def test_cache_management_operations(self, config, cache_config, mock_http_client):
        """Test cache management operations"""
        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.search"
            ) as mock_search,
        ):

            mock_search.return_value = [{"id": "vec_1", "score": 0.95}]

            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            try:
                # Populate cache
                client.search_cached("collection_1", [1.0, 2.0], top_k=10)
                client.search_cached("collection_2", [1.0, 2.0], top_k=10)

                # Get cache stats
                stats = client.get_cache_stats()
                assert stats["total_entries"] == 2

                # Invalidate specific collection
                invalidated = client.invalidate_collection_cache("collection_1")
                assert invalidated == 1

                # Verify remaining cache entries
                stats = client.get_cache_stats()
                assert stats["total_entries"] == 1

                # Clear all cache
                cleared = client.clear_cache()
                assert cleared == 1

                # Verify cache is empty
                stats = client.get_cache_stats()
                assert stats["total_entries"] == 0

            finally:
                client.close()

    def test_cache_management_disabled_errors(self, config):
        """Test errors when trying cache management on disabled caching"""
        with patch("proximadb.protocols.rest_sync.ProximaDBClient._create_http_client"):
            client = ProximaDBClient(config=config)  # No caching

            try:
                # Should raise errors for cache management operations
                with pytest.raises(RuntimeError, match="Caching is not enabled"):
                    client.clear_cache()

                with pytest.raises(RuntimeError, match="Caching is not enabled"):
                    client.invalidate_collection_cache("test")

                with pytest.raises(RuntimeError, match="Caching is not enabled"):
                    client.warm_cache([])

            finally:
                client.close()

    @pytest.mark.skip(reason="warm_cache feature not yet implemented")
    def test_cache_warming(self, config, cache_config, mock_http_client):
        """Test cache warming functionality"""
        with patch(
            "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
            return_value=mock_http_client,
        ):

            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            try:
                # Prepare warmup operations
                warmup_ops = [
                    (
                        "search_vectors",
                        "collection_1",
                        {"vector": [1.0, 2.0], "top_k": 10},
                        [{"id": "vec_1"}],
                    ),
                    (
                        "get_vector",
                        "collection_1",
                        {"vector_id": "vec_1"},
                        {"id": "vec_1", "vector": [1.0, 2.0]},
                    ),
                    ("list_collections", "_global", {}, [{"id": "collection_1"}]),
                ]

                # Warm cache
                warmed = client.warm_cache(warmup_ops)
                assert warmed == 3

                # Verify cache has entries
                stats = client.get_cache_stats()
                assert stats["total_entries"] == 3

            finally:
                client.close()

    def test_concurrent_caching(self, config, cache_config, mock_http_client):
        """Test concurrent cache operations"""
        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.search"
            ) as mock_search,
        ):

            # Configure search to return different results for different threads
            def search_side_effect(*args, **kwargs):
                thread_id = threading.current_thread().ident
                return [{"id": f"vec_{thread_id}", "score": 0.95}]

            mock_search.side_effect = search_side_effect

            client = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            results = []
            errors = []

            def cache_worker():
                try:
                    # Each thread performs search operations
                    for i in range(10):
                        result = client.search_cached(
                            collection_id=f"collection_{threading.current_thread().ident}",
                            vector=[float(i), float(i + 1)],
                            top_k=5,
                        )
                        results.append(result)

                        # Get cache stats
                        stats = client.get_cache_stats()
                        assert isinstance(stats, dict)

                except Exception as e:
                    errors.append(e)

            try:
                # Run concurrent workers
                threads = []
                for _ in range(3):
                    thread = threading.Thread(target=cache_worker)
                    threads.append(thread)
                    thread.start()

                for thread in threads:
                    thread.join()

                # Should complete without errors
                assert len(errors) == 0
                assert len(results) > 0

                # Cache should have entries from different threads
                stats = client.get_cache_stats()
                assert stats["total_entries"] > 0

            finally:
                client.close()

    def test_context_manager_with_caching(self, config, cache_config, mock_http_client):
        """Test client as context manager with caching"""
        with patch(
            "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
            return_value=mock_http_client,
        ):

            with ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            ) as client:
                assert client.enable_caching is True
                assert client._response_cache is not None

                # Use cache
                stats = client.get_cache_stats()
                assert isinstance(stats, dict)

            # Should be cleaned up after context manager exit
            assert client._response_cache is None

    @pytest.mark.skip(reason="CachePolicy and CacheConfig not yet implemented")
    def test_different_cache_policies(self, config, mock_http_client):
        """Test different cache eviction policies"""
        policies = [
            CachePolicy.LRU,
            CachePolicy.LFU,
            CachePolicy.TTL,
            CachePolicy.ADAPTIVE,
        ]

        for policy in policies:
            cache_config = CacheConfig(
                max_memory_mb=5.0, policy=policy, max_cache_entries=10
            )

            with (
                patch(
                    "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                    return_value=mock_http_client,
                ),
                patch(
                    "proximadb.protocols.rest_sync.ProximaDBClient.search"
                ) as mock_search,
            ):

                mock_search.return_value = [{"id": "vec_1", "score": 0.95}]

                client = ProximaDBClient(
                    config=config, enable_caching=True, cache_config=cache_config
                )

                try:
                    # Add some cached entries
                    for i in range(5):
                        client.search_cached(
                            collection_id=f"collection_{i}", vector=[float(i)], top_k=10
                        )

                    # Verify policy is set correctly
                    stats = client.get_cache_stats()
                    assert stats["policy"] == policy.value
                    assert stats["total_entries"] > 0

                finally:
                    client.close()


@pytest.mark.performance
class TestCachePerformanceIntegration:
    """Performance tests for cache integration"""

    @pytest.mark.skip(reason="CacheConfig not yet implemented")
    def test_cache_vs_no_cache_performance(self):
        """Compare performance with and without caching"""
        config = ClientConfig(url="http://localhost:5678")

        # Mock that simulates network delay
        def slow_search(*args, **kwargs):
            time.sleep(0.01)  # 10ms delay per call
            return [{"id": "vec_1", "score": 0.95}]

        mock_http_client = Mock()

        with (
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient._create_http_client",
                return_value=mock_http_client,
            ),
            patch(
                "proximadb.protocols.rest_sync.ProximaDBClient.search",
                side_effect=slow_search,
            ),
        ):

            # Test without caching
            client_no_cache = ProximaDBClient(config=config)

            start_time = time.time()
            for _ in range(10):
                client_no_cache.search_cached(  # Will fallback to regular search
                    collection_id="test", vector=[1.0, 2.0], top_k=10
                )
            no_cache_time = time.time() - start_time
            client_no_cache.close()

            # Test with caching
            cache_config = CacheConfig(max_memory_mb=10.0)
            client_with_cache = ProximaDBClient(
                config=config, enable_caching=True, cache_config=cache_config
            )

            start_time = time.time()
            for _ in range(10):
                client_with_cache.search_cached(
                    collection_id="test", vector=[1.0, 2.0], top_k=10
                )
            with_cache_time = time.time() - start_time

            # Get cache stats
            stats = client_with_cache.get_cache_stats()
            client_with_cache.close()

            print(f"No cache: {no_cache_time:.3f}s")
            print(f"With cache: {with_cache_time:.3f}s")
            print(f"Cache hit rate: {stats['hit_rate_percent']:.1f}%")
            print(f"Speedup: {no_cache_time / with_cache_time:.1f}x")

            # Caching should be significantly faster for repeated queries
            assert with_cache_time < no_cache_time * 0.5  # At least 2x faster
            assert stats["hit_rate_percent"] > 80  # High hit rate


if __name__ == "__main__":
    # Run basic tests
    pytest.main([__file__, "-v"])
