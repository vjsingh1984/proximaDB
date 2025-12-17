"""
Tests for unified response caching functionality

Tests response caching, smart caching, and object pooling using
the new unified cache system.
"""

import pytest
import time
import threading
from pathlib import Path
import sys
from unittest.mock import Mock, patch, MagicMock

# Add utils to path
sys.path.insert(0, str(Path(__file__).parent.parent))
from utils.base_test import BaseProximaDBTest
from utils.server_utils import ensure_server_running

from proximadb_sdk.cache import (
    CacheStrategy,
    CacheLevel,
    CacheMetrics,
    CacheEntry,
    MemoryCacheBackend,
    ResponseCache,
    SmartCache,
    ObjectPool,
    ObjectPoolMetrics
)
# Legacy imports removed - using unified cache system only


class TestCacheMetrics:
    """Test unified cache metrics"""
    
    def test_metrics_initialization(self):
        """Test metrics initialization"""
        metrics = CacheMetrics()
        
        assert metrics.hits == 0
        assert metrics.misses == 0
        assert metrics.evictions == 0
        assert metrics.total_requests == 0
        assert metrics.hit_rate == 0.0
        assert metrics.miss_rate == 1.0
    
    def test_hit_rate_calculation(self):
        """Test hit rate calculation"""
        metrics = CacheMetrics(hits=8, total_requests=10)

        assert metrics.hit_rate == pytest.approx(0.8)
        assert metrics.miss_rate == pytest.approx(0.2)
    
    def test_edge_cases(self):
        """Test edge cases in metrics"""
        metrics = CacheMetrics(total_requests=0)
        
        assert metrics.hit_rate == 0.0
        assert metrics.miss_rate == 1.0


class TestCacheEntry:
    """Test unified cache entry functionality"""
    
    def test_entry_creation(self):
        """Test cache entry creation"""
        entry = CacheEntry(
            key="test_key",
            value={"data": "test"},
            ttl=300.0
        )
        
        assert entry.key == "test_key"
        assert entry.value == {"data": "test"}
        assert entry.access_count == 0
        assert entry.ttl == 300.0
        assert not entry.is_expired()
    
    def test_entry_expiration(self):
        """Test entry expiration checking"""
        # Create expired entry
        entry = CacheEntry(
            key="test_key",
            value="test",
            timestamp=time.time() - 1000,  # 1000 seconds ago
            ttl=300.0  # 5 minutes TTL
        )
        
        assert entry.is_expired()
    
    def test_access_tracking(self):
        """Test access tracking"""
        entry = CacheEntry(key="test_key", value="test")
        
        old_access_time = entry.last_access
        old_count = entry.access_count
        
        entry.access()
        
        assert entry.last_access >= old_access_time
        assert entry.access_count == old_count + 1
    
    def test_no_expiration(self):
        """Test entry without TTL doesn't expire"""
        entry = CacheEntry(
            key="test_key",
            value="test",
            timestamp=time.time() - 1000,  # 1000 seconds ago
            ttl=None  # No TTL
        )
        
        assert not entry.is_expired()


class TestMemoryCacheBackend:
    """Test memory cache backend with different strategies"""
    
    @pytest.fixture
    def cache(self):
        """Create cache backend with small size for testing"""
        return MemoryCacheBackend(max_size=3, strategy=CacheStrategy.LRU)
    
    def test_backend_initialization(self, cache):
        """Test backend initialization"""
        assert cache.max_size == 3
        assert cache.strategy == CacheStrategy.LRU
        assert cache.size() == 0
        assert isinstance(cache.metrics, CacheMetrics)
    
    def test_set_and_get(self, cache):
        """Test basic set and get operations"""
        # Set value
        success = cache.set("key1", "value1", ttl=300.0)
        assert success is True
        assert cache.size() == 1
        
        # Get value
        value = cache.get("key1")
        assert value == "value1"
        assert cache.metrics.hits == 1
        assert cache.metrics.misses == 0
    
    def test_cache_miss(self, cache):
        """Test cache miss behavior"""
        # Try to get non-existent key
        result = cache.get("nonexistent")
        
        assert result is None
        assert cache.metrics.hits == 0
        assert cache.metrics.misses == 1
    
    def test_ttl_expiration(self, cache):
        """Test TTL-based expiration"""
        # Set with very short TTL
        cache.set("short_ttl", "value", ttl=0.1)
        
        # Should be available immediately
        assert cache.get("short_ttl") == "value"
        
        # Wait for expiration
        time.sleep(0.2)
        
        # Should be expired now
        assert cache.get("short_ttl") is None
        assert cache.metrics.misses > 0
    
    def test_lru_eviction(self, cache):
        """Test LRU eviction policy"""
        # Fill cache to capacity
        cache.set("key1", "value1")
        cache.set("key2", "value2")
        cache.set("key3", "value3")
        
        assert cache.size() == 3
        
        # Access first key to make it recently used
        cache.get("key1")
        
        # Add fourth key - should evict least recently used (key2)
        cache.set("key4", "value4")
        
        assert cache.size() == 3
        assert cache.get("key1") == "value1"  # Should still exist
        assert cache.get("key2") is None      # Should be evicted
        assert cache.get("key3") == "value3"  # Should still exist
        assert cache.get("key4") == "value4"  # Should exist
        assert cache.metrics.evictions == 1
    
    def test_lfu_eviction(self):
        """Test LFU eviction policy"""
        cache = MemoryCacheBackend(max_size=3, strategy=CacheStrategy.LFU)
        
        # Fill cache
        cache.set("key1", "value1")
        cache.set("key2", "value2")
        cache.set("key3", "value3")
        
        # Access keys different numbers of times
        for _ in range(5):
            cache.get("key1")  # Most frequently used
        cache.get("key2")      # Less frequently used
        # key3 not accessed    # Least frequently used
        
        # Add fourth key - should evict least frequently used (key3)
        cache.set("key4", "value4")
        
        assert cache.get("key1") == "value1"  # Should still exist
        assert cache.get("key2") == "value2"  # Should still exist
        assert cache.get("key3") is None      # Should be evicted
        assert cache.get("key4") == "value4"  # Should exist
    
    def test_compression(self):
        """Test compression for large values"""
        cache = MemoryCacheBackend(compression_threshold=100)
        
        # Create large value that should trigger compression
        large_value = {"data": "x" * 500}
        
        success = cache.set("large_key", large_value)
        assert success is True
        
        # Should be able to retrieve and decompress
        retrieved = cache.get("large_key")
        assert retrieved == large_value
    
    def test_delete(self, cache):
        """Test key deletion"""
        cache.set("key1", "value1")
        assert cache.get("key1") == "value1"
        
        deleted = cache.delete("key1")
        assert deleted is True
        assert cache.get("key1") is None
        assert cache.metrics.invalidations == 1
        
        # Delete non-existent key
        deleted = cache.delete("nonexistent")
        assert deleted is False
    
    def test_clear(self, cache):
        """Test cache clearing"""
        # Add some entries
        cache.set("key1", "value1")
        cache.set("key2", "value2")
        cache.set("key3", "value3")
        
        assert cache.size() == 3
        
        # Clear cache
        cleared = cache.clear()
        assert cleared == 3
        assert cache.size() == 0
        assert cache.metrics.invalidations == 3


class TestResponseCache:
    """Test high-level response cache functionality"""
    
    @pytest.fixture
    def cache(self):
        """Create response cache with memory backend"""
        backend = MemoryCacheBackend(max_size=100)
        return ResponseCache(
            backend=backend,
            default_ttl=300,
            collection_aware=True
        )
    
    def test_cache_initialization(self, cache):
        """Test cache initialization"""
        assert cache.default_ttl == 300
        assert cache.collection_aware is True
        assert isinstance(cache.backend, MemoryCacheBackend)
    
    def test_cache_key_generation(self, cache):
        """Test cache key generation"""
        key1 = cache.cache_key("search_vectors", query=[1.0, 2.0], k=10)
        key2 = cache.cache_key("search_vectors", query=[1.0, 2.0], k=10)
        key3 = cache.cache_key("search_vectors", query=[1.0, 2.0], k=5)
        
        # Same parameters should generate same key
        assert key1 == key2
        
        # Different parameters should generate different key
        assert key1 != key3
        
        # Keys should be reasonable length (SHA256 hash)
        assert len(key1) == 64
    
    def test_get_set_with_collection_tracking(self, cache):
        """Test get/set with collection tracking"""
        params = {"query": [1.0, 2.0], "k": 10}
        response = {"results": [{"id": "vec1", "score": 0.95}]}
        
        # Set with collection tracking
        success = cache.set("search_vectors", params, response, collection_id="test_collection")
        assert success is True
        
        # Get should return cached response
        cached_response = cache.get("search_vectors", params)
        assert cached_response == response
    
    def test_get_with_fetch_function(self, cache):
        """Test get with fetch function on cache miss"""
        params = {"query": [1.0, 2.0], "k": 10}
        expected_response = {"results": [{"id": "vec1", "score": 0.95}]}
        
        # Real fetch function
        call_count = 0
        def fetch_func():
            nonlocal call_count
            call_count += 1
            return expected_response
        
        # Get with fetch function (cache miss)
        result = cache.get("search_vectors", params, fetch_func=fetch_func)
        
        assert result == expected_response
        assert call_count == 1
        
        # Second call should hit cache
        result = cache.get("search_vectors", params, fetch_func=fetch_func)
        assert result == expected_response
        assert call_count == 1  # Not called again
    
    def test_collection_invalidation(self, cache):
        """Test collection-based cache invalidation"""
        # Add entries for multiple collections
        for i in range(3):
            cache.set(
                "search_vectors",
                {"k": i},
                {"result": f"c1_{i}"},
                collection_id="collection_1"
            )
            cache.set(
                "search_vectors",
                {"k": i + 10},
                {"result": f"c2_{i}"},
                collection_id="collection_2"
            )
        
        # Should have 6 entries
        assert cache.backend.size() == 6
        
        # Invalidate collection_1
        invalidated = cache.invalidate_collection("collection_1")
        
        assert invalidated == 3
        assert cache.backend.size() == 3
        
        # collection_1 entries should be gone
        for i in range(3):
            result = cache.get("search_vectors", {"k": i})
            assert result is None
        
        # collection_2 entries should still exist
        for i in range(3):
            result = cache.get("search_vectors", {"k": i + 10})
            assert result == {"result": f"c2_{i}"}
    
    def test_pattern_invalidation(self, cache):
        """Test pattern-based cache invalidation"""
        # Add entries for different operations
        cache.set("search_vectors", {"k": 1}, {"search": "result1"})
        cache.set("get_vector", {"id": "vec1"}, {"get": "result1"})
        cache.set("search_vectors", {"k": 2}, {"search": "result2"})
        
        assert cache.backend.size() == 3
        
        # Invalidate search operations (simplified - clears all)
        invalidated = cache.invalidate_pattern("search")
        
        # This implementation clears all for search patterns
        assert invalidated == 3
        assert cache.backend.size() == 0
    
    def test_get_metrics(self, cache):
        """Test getting cache metrics"""
        # Perform some operations
        cache.set("search", {"k": 1}, {"result": 1})
        cache.get("search", {"k": 1})
        cache.get("search", {"k": 2})  # Miss
        
        metrics = cache.get_metrics()
        
        assert isinstance(metrics, CacheMetrics)
        assert metrics.hits >= 1
        assert metrics.misses >= 1
        assert metrics.total_requests >= 2


class TestSmartCache:
    """Test smart cache with multi-level support"""
    
    @pytest.fixture
    def smart_cache(self):
        """Create smart cache with L1 and L2 backends"""
        l1 = MemoryCacheBackend(max_size=10, strategy=CacheStrategy.LRU)
        l2 = MemoryCacheBackend(max_size=100, strategy=CacheStrategy.LFU)
        return SmartCache(l1_backend=l1, l2_backend=l2, enable_prefetch=True)
    
    def test_smart_cache_initialization(self, smart_cache):
        """Test smart cache initialization"""
        assert smart_cache.l1 is not None
        assert smart_cache.l2 is not None
        assert smart_cache.enable_prefetch is True
        assert smart_cache.prefetch_threshold == 3
    
    def test_l1_cache_hit(self, smart_cache):
        """Test L1 cache hit"""
        smart_cache.set("key1", "value1", level=CacheLevel.L1_MEMORY)
        
        # Should hit L1
        value = smart_cache.get("key1")
        assert value == "value1"
    
    def test_l2_promotion(self, smart_cache):
        """Test L2 to L1 promotion"""
        # Set in L2 only
        smart_cache.l2.set("key1", "value1")
        
        # Get should find in L2 and promote to L1
        value = smart_cache.get("key1")
        assert value == "value1"
        
        # Should now be in L1 too
        l1_value = smart_cache.l1.get("key1")
        assert l1_value == "value1"
    
    def test_cache_miss(self, smart_cache):
        """Test cache miss in both levels"""
        value = smart_cache.get("nonexistent")
        assert value is None
    
    def test_level_specific_setting(self, smart_cache):
        """Test setting values at specific cache levels"""
        # Set in L1
        success = smart_cache.set("key1", "value1", level=CacheLevel.L1_MEMORY)
        assert success is True
        assert smart_cache.l1.get("key1") == "value1"
        
        # Set in L2
        success = smart_cache.set("key2", "value2", level=CacheLevel.L2_DISK)
        assert success is True
        assert smart_cache.l2.get("key2") == "value2"
    
    def test_prefetch_functionality(self, smart_cache):
        """Test prefetching functionality"""
        # Mock fetch function
        def fetch_func(key):
            return f"fetched_{key}"
        
        keys_to_prefetch = ["key1", "key2", "key3"]
        
        # Prefetch should populate cache
        smart_cache.prefetch(keys_to_prefetch, fetch_func)
        
        # Keys should now be in cache
        for key in keys_to_prefetch:
            value = smart_cache.get(key)
            assert value == f"fetched_{key}"
    
    def test_get_metrics(self, smart_cache):
        """Test getting metrics from all levels"""
        # Perform some operations
        smart_cache.set("key1", "value1")
        smart_cache.get("key1")
        smart_cache.get("nonexistent")
        
        metrics = smart_cache.get_metrics()
        
        assert 'l1' in metrics
        assert isinstance(metrics['l1'], CacheMetrics)
        
        if smart_cache.l2:
            assert 'l2' in metrics
            assert isinstance(metrics['l2'], CacheMetrics)


class TestObjectPool:
    """Test object pool functionality"""
    
    @pytest.fixture
    def object_pool(self):
        """Create object pool for testing"""
        def factory(config):
            """Mock factory function"""
            return {"config": config, "created_at": time.time()}
        
        def key_func(config):
            """Mock key function"""
            return f"pool_{config.get('type', 'default')}"
        
        return ObjectPool(
            factory=factory,
            key_func=key_func,
            max_pool_size=3,
            enable_metrics=True
        )
    
    def test_pool_initialization(self, object_pool):
        """Test object pool initialization"""
        assert object_pool.max_pool_size == 3
        assert object_pool.enable_metrics is True
        assert isinstance(object_pool.metrics, ObjectPoolMetrics)
    
    def test_acquire_and_release(self, object_pool):
        """Test acquiring and releasing objects"""
        config = {"type": "test"}
        
        # Acquire object (should create new one)
        obj1 = object_pool.acquire(config)
        assert obj1 is not None
        assert obj1["config"] == config
        assert object_pool.metrics.acquisitions == 1
        assert object_pool.metrics.objects_created == 1
        
        # Release object back to pool
        object_pool.release(obj1, config)
        assert object_pool.metrics.releases == 1
        
        # Acquire again (should reuse from pool)
        obj2 = object_pool.acquire(config)
        assert obj2 is obj1  # Same object
        assert object_pool.metrics.acquisitions == 2
        assert object_pool.metrics.hits == 1
    
    def test_pool_size_limit(self, object_pool):
        """Test pool size limitation"""
        config = {"type": "test"}
        objects = []
        
        # Create more objects than pool size
        for i in range(5):
            obj = object_pool.acquire(config)
            objects.append(obj)
        
        # Release all objects
        for obj in objects:
            object_pool.release(obj, config)
        
        # Pool should only keep max_pool_size objects
        stats = object_pool.get_stats()
        pool_size = stats['pool_details'].get('pool_test', 0)
        assert pool_size <= object_pool.max_pool_size
        assert object_pool.metrics.objects_discarded > 0
    
    def test_multiple_pools(self, object_pool):
        """Test multiple pools for different configurations"""
        config1 = {"type": "type1"}
        config2 = {"type": "type2"}
        
        # Acquire objects from different pools
        obj1 = object_pool.acquire(config1)
        obj2 = object_pool.acquire(config2)
        
        # Release them
        object_pool.release(obj1, config1)
        object_pool.release(obj2, config2)
        
        # Should have created 2 separate pools
        stats = object_pool.get_stats()
        assert stats['active_pools'] == 2
        assert 'pool_type1' in stats['pool_details']
        assert 'pool_type2' in stats['pool_details']
    
    def test_clear_operations(self, object_pool):
        """Test clearing pools"""
        config = {"type": "test"}
        
        # Add some objects
        obj1 = object_pool.acquire(config)
        obj2 = object_pool.acquire(config)
        object_pool.release(obj1, config)
        object_pool.release(obj2, config)
        
        # Clear specific pool
        cleared = object_pool.clear_pool("pool_test")
        assert cleared == 2
        
        stats = object_pool.get_stats()
        assert stats['pool_details'].get('pool_test', 0) == 0
    
    def test_get_stats(self, object_pool):
        """Test getting pool statistics"""
        config = {"type": "test"}
        
        # Perform some operations
        obj = object_pool.acquire(config)
        object_pool.release(obj, config)
        object_pool.acquire(config)  # Should be cache hit
        
        stats = object_pool.get_stats()
        
        assert 'active_pools' in stats
        assert 'total_objects' in stats
        assert 'hit_rate_percent' in stats
        assert 'total_acquisitions' in stats
        assert stats['total_acquisitions'] == 2
        assert stats['cache_hits'] == 1


# Backward compatibility tests removed - legacy modules no longer exist


class TestResponseCacheIntegration(BaseProximaDBTest):
    """Integration tests with real server"""
    
    def test_cache_with_real_operations(self):
        """Test cache integration with real operations"""
        ensure_server_running()
        
        # Create response cache
        backend = MemoryCacheBackend(max_size=100)
        cache = ResponseCache(backend=backend, collection_aware=True)
        
        # Mock a real operation
        def mock_search_operation():
            return {
                "results": [
                    {"id": "vec1", "score": 0.95},
                    {"id": "vec2", "score": 0.87}
                ]
            }
        
        # Test cache miss and fetch
        params = {"collection_id": "test_collection", "query": [1.0, 2.0], "k": 10}
        result = cache.get("search_vectors", params, fetch_func=mock_search_operation)
        
        assert result is not None
        assert "results" in result
        assert len(result["results"]) == 2
        
        # Test cache hit (should not call fetch function again)
        fetch_call_count = 0
        def fetch_that_shouldnt_be_called():
            nonlocal fetch_call_count
            fetch_call_count += 1
            return {"should_not_see": "this"}
            
        result = cache.get("search_vectors", params, fetch_func=fetch_that_shouldnt_be_called)
        
        # Should get cached result, not new result
        assert result["results"][0]["id"] == "vec1"
        assert fetch_call_count == 0


@pytest.mark.performance
class TestCachePerformance:
    """Performance tests for unified caching system"""
    
    def test_memory_cache_performance(self):
        """Test memory cache access performance"""
        cache = MemoryCacheBackend(max_size=1000)
        
        # Populate cache
        for i in range(500):
            cache.set(f"key_{i}", {"data": f"value_{i}", "index": i})
        
        # Measure access performance
        start_time = time.time()
        
        for i in range(5000):
            cache.get(f"key_{i % 500}")
        
        end_time = time.time()
        duration = end_time - start_time
        accesses_per_second = 5000 / duration
        
        print(f"Memory cache accesses: {accesses_per_second:.0f} per second")
        print(f"Hit rate: {cache.metrics.hit_rate:.1%}")
        
        # Should be very fast (>10K accesses/sec)
        assert accesses_per_second > 10000
        assert cache.metrics.hit_rate > 0.9  # >90% hit rate
    
    def test_concurrent_access(self):
        """Test concurrent cache access performance"""
        cache = MemoryCacheBackend(max_size=1000)
        results = []
        errors = []
        
        def cache_worker():
            try:
                for i in range(100):
                    # Set operation
                    cache.set(
                        f"key_{threading.current_thread().ident}_{i}",
                        {"data": f"value_{i}"}
                    )
                    
                    # Get operation
                    result = cache.get(f"key_{threading.current_thread().ident}_{i}")
                    if result:
                        results.append(result)
                        
            except Exception as e:
                errors.append(e)
        
        # Run concurrent workers
        threads = []
        start_time = time.time()
        
        for _ in range(10):
            thread = threading.Thread(target=cache_worker)
            threads.append(thread)
            thread.start()
        
        for thread in threads:
            thread.join()
        
        end_time = time.time()
        
        # Should complete without errors
        assert len(errors) == 0
        assert len(results) > 0
        
        # Cache should have entries
        assert cache.size() > 0
        
        print(f"Concurrent test completed in {end_time - start_time:.2f}s")
        print(f"Total operations: {len(results)}")
        print(f"Cache size: {cache.size()}")


if __name__ == "__main__":
    # Run basic tests
    pytest.main([__file__, "-v"])