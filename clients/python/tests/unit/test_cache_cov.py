"""Offline unit tests for proximadb_sdk.cache.

Pure module (no network / heavy deps). Exercises eviction strategies,
TTL expiry (time patched), compression, collection-aware invalidation,
multi-level SmartCache, and ObjectPool reuse + stats.
"""

import pickle
import zlib
from unittest import mock

import pytest

from proximadb_sdk.cache import (
    CacheEntry,
    CacheLevel,
    CacheMetrics,
    CacheStrategy,
    MemoryCacheBackend,
    ObjectPool,
    ObjectPoolMetrics,
    ResponseCache,
    SmartCache,
)


# --------------------------------------------------------------------------
# CacheMetrics
# --------------------------------------------------------------------------
def test_cache_metrics_hit_rate_zero_requests():
    m = CacheMetrics()
    assert m.hit_rate == 0.0
    assert m.miss_rate == 1.0


def test_cache_metrics_hit_rate():
    m = CacheMetrics(hits=3, total_requests=4)
    assert m.hit_rate == 0.75
    assert m.miss_rate == 0.25


# --------------------------------------------------------------------------
# CacheEntry
# --------------------------------------------------------------------------
def test_cache_entry_not_expired_without_ttl():
    e = CacheEntry(key="k", value=1)
    assert e.is_expired() is False


def test_cache_entry_expired_with_ttl():
    e = CacheEntry(key="k", value=1, ttl=10.0)
    e.timestamp = 100.0
    with mock.patch("proximadb_sdk.cache.time.time", return_value=200.0):
        assert e.is_expired() is True


def test_cache_entry_not_expired_within_ttl():
    e = CacheEntry(key="k", value=1, ttl=100.0)
    e.timestamp = 100.0
    with mock.patch("proximadb_sdk.cache.time.time", return_value=150.0):
        assert e.is_expired() is False


def test_cache_entry_access_increments():
    e = CacheEntry(key="k", value=1)
    assert e.access_count == 0
    e.access()
    e.access()
    assert e.access_count == 2
    assert e.last_access > 0


# --------------------------------------------------------------------------
# MemoryCacheBackend - basic get/set/delete/clear/size
# --------------------------------------------------------------------------
def test_memory_set_get_miss():
    c = MemoryCacheBackend(max_size=10)
    assert c.get("missing") is None
    assert c.metrics.misses == 1
    assert c.metrics.total_requests == 1


def test_memory_set_get_hit():
    c = MemoryCacheBackend(max_size=10)
    assert c.set("a", 123) is True
    assert c.get("a") == 123
    assert c.metrics.hits == 1
    assert c.size() == 1


def test_memory_delete():
    c = MemoryCacheBackend()
    c.set("a", 1)
    assert c.delete("a") is True
    assert c.delete("a") is False
    assert c.metrics.invalidations == 1


def test_memory_clear():
    c = MemoryCacheBackend()
    c.set("a", 1)
    c.set("b", 2)
    assert c.clear() == 2
    assert c.size() == 0
    assert c.metrics.cache_size_bytes == 0


def test_memory_get_expired_entry_removed():
    c = MemoryCacheBackend()
    c.set("a", "v", ttl=10.0)
    # pin the entry's timestamp into the past so it is expired on read
    c._cache["a"].timestamp = 1000.0
    with mock.patch("proximadb_sdk.cache.time.time", return_value=2000.0):
        assert c.get("a") is None
    assert c.size() == 0
    assert c.metrics.misses == 1


# --------------------------------------------------------------------------
# MemoryCacheBackend - compression path
# --------------------------------------------------------------------------
def test_memory_compression_for_big_compressible_value():
    c = MemoryCacheBackend(compression_threshold=10)
    big = "x" * 5000  # highly compressible
    assert c.set("big", big) is True
    entry = c._cache["big"]
    assert entry.compressed is True
    # stored value should be compressed bytes that decompress to original
    restored = pickle.loads(zlib.decompress(entry.value))
    assert restored == big
    # get path must decompress
    assert c.get("big") == big


def test_memory_no_compression_for_incompressible_value():
    c = MemoryCacheBackend(compression_threshold=10)
    # random-ish bytes won't compress to <90%
    import os

    data = os.urandom(2000)
    c.set("rand", data)
    entry = c._cache["rand"]
    assert entry.compressed is False
    assert c.get("rand") == data


# --------------------------------------------------------------------------
# Eviction strategies
# --------------------------------------------------------------------------
def test_evict_lru():
    c = MemoryCacheBackend(max_size=2, strategy=CacheStrategy.LRU)
    c.set("a", 1)
    c.set("b", 2)
    c.get("a")  # a now most recent
    c.set("c", 3)  # should evict b
    assert c.get("b") is None
    assert c.get("a") == 1
    assert c.get("c") == 3
    assert c.metrics.evictions == 1


def test_evict_lfu():
    c = MemoryCacheBackend(max_size=2, strategy=CacheStrategy.LFU)
    c.set("a", 1)
    c.set("b", 2)
    # access a multiple times -> b least frequently used
    c.get("a")
    c.get("a")
    c.set("c", 3)  # evicts b (lowest access_count)
    assert c.get("b") is None
    assert c.get("a") == 1
    assert c.metrics.evictions == 1


def test_evict_ttl_strategy():
    c = MemoryCacheBackend(max_size=2, strategy=CacheStrategy.TTL)
    with mock.patch("proximadb_sdk.cache.time.time", return_value=100.0):
        c.set("a", 1)
    with mock.patch("proximadb_sdk.cache.time.time", return_value=200.0):
        c.set("b", 2)
    with mock.patch("proximadb_sdk.cache.time.time", return_value=300.0):
        c.set("c", 3)  # evicts oldest timestamp = a
    assert "a" not in c._cache
    assert c.metrics.evictions == 1


def test_evict_adaptive_with_low_access_candidates():
    c = MemoryCacheBackend(max_size=2, strategy=CacheStrategy.ADAPTIVE)
    c.set("a", 1)
    c.set("b", 2)
    c.set("c", 3)  # both have access_count < 3 -> evict by last_access
    assert c.size() == 2
    assert c.metrics.evictions == 1


def test_evict_adaptive_no_candidates_falls_back():
    c = MemoryCacheBackend(max_size=2, strategy=CacheStrategy.ADAPTIVE)
    c.set("a", 1)
    c.set("b", 2)
    # bump both above access threshold (>=3)
    for _ in range(5):
        c.get("a")
        c.get("b")
    c.set("c", 3)  # no low-access candidates -> popitem(last=False)
    assert c.size() == 2
    assert c.metrics.evictions == 1


def test_evict_one_empty_noop():
    c = MemoryCacheBackend(max_size=2)
    # call private evict on empty cache - should not raise
    c._evict_one()
    assert c.size() == 0


# --------------------------------------------------------------------------
# ResponseCache
# --------------------------------------------------------------------------
def test_response_cache_key_deterministic():
    rc = ResponseCache()
    k1 = rc.cache_key("search", a=1, b=2)
    k2 = rc.cache_key("search", b=2, a=1)
    assert k1 == k2
    assert isinstance(k1, str) and len(k1) == 64


def test_response_cache_get_miss_no_fetch():
    rc = ResponseCache()
    assert rc.get("op", {"x": 1}) is None


def test_response_cache_get_with_fetch_populates():
    rc = ResponseCache(default_ttl=60)
    calls = []

    def fetch():
        calls.append(1)
        return {"result": 42}

    first = rc.get("op", {"x": 1}, fetch_func=fetch)
    assert first == {"result": 42}
    # second call should hit cache, not fetch again
    second = rc.get("op", {"x": 1}, fetch_func=fetch)
    assert second == {"result": 42}
    assert len(calls) == 1


def test_response_cache_fetch_returns_none_not_cached():
    rc = ResponseCache()
    result = rc.get("op", {"x": 1}, fetch_func=lambda: None)
    assert result is None


def test_response_cache_set_with_collection_tracking():
    rc = ResponseCache(collection_aware=True)
    rc.set("search", {"q": "a"}, "v1", collection_id="coll1")
    rc.set("search", {"q": "b"}, "v2", collection_id="coll1")
    assert len(rc._collection_keys["coll1"]) == 2
    n = rc.invalidate_collection("coll1")
    assert n == 2
    assert rc.get("search", {"q": "a"}) is None
    assert rc._collection_keys.get("coll1") is None


def test_response_cache_invalidate_collection_disabled():
    rc = ResponseCache(collection_aware=False)
    assert rc.invalidate_collection("anything") == 0


def test_response_cache_invalidate_pattern_matching_op():
    rc = ResponseCache()
    rc.set("search", {"q": "a"}, "v1")
    rc.set("get_vector", {"id": "1"}, "v2")
    count = rc.invalidate_pattern("search")
    assert count == 2  # clears whole backend


def test_response_cache_invalidate_pattern_non_matching_op():
    rc = ResponseCache()
    rc.set("custom_op", {"q": "a"}, "v1")
    assert rc.invalidate_pattern("custom_op") == 0


def test_response_cache_close_clears():
    rc = ResponseCache()
    rc.set("search", {"q": "a"}, "v1", collection_id="c1")
    rc.close()
    assert rc.backend.size() == 0
    assert len(rc._collection_keys) == 0
    assert len(rc._key_collections) == 0


def test_response_cache_get_metrics_from_backend():
    rc = ResponseCache()
    rc.set("search", {"q": "a"}, "v1")
    rc.get("search", {"q": "a"})
    m = rc.get_metrics()
    assert isinstance(m, CacheMetrics)
    assert m.hits >= 1


def test_response_cache_get_metrics_no_metrics_backend():
    class Dummy:
        def get(self, k):
            return None

        def set(self, k, v, ttl=None):
            return True

        def clear(self):
            return 0

    rc = ResponseCache(backend=Dummy())
    m = rc.get_metrics()
    assert isinstance(m, CacheMetrics)


# --------------------------------------------------------------------------
# SmartCache
# --------------------------------------------------------------------------
def test_smartcache_l1_hit():
    sc = SmartCache()
    sc.set("k", "v")
    assert sc.get("k") == "v"


def test_smartcache_l1_miss_no_l2():
    sc = SmartCache(l2_backend=None)
    assert sc.get("missing") is None


def test_smartcache_l2_promote_to_l1():
    l1 = MemoryCacheBackend(max_size=10)
    l2 = MemoryCacheBackend(max_size=10)
    sc = SmartCache(l1_backend=l1, l2_backend=l2)
    l2.set("k", "v2")
    assert sc.get("k") == "v2"
    # promoted to L1
    assert l1.get("k") == "v2"


def test_smartcache_set_l2_level():
    l1 = MemoryCacheBackend()
    l2 = MemoryCacheBackend()
    sc = SmartCache(l1_backend=l1, l2_backend=l2)
    sc.set("k", "v", level=CacheLevel.L2_DISK)
    assert l2.get("k") == "v"
    assert l1.get("k") is None


def test_smartcache_set_l2_level_without_l2_falls_back_l1():
    l1 = MemoryCacheBackend()
    sc = SmartCache(l1_backend=l1, l2_backend=None)
    sc.set("k", "v", level=CacheLevel.L2_DISK)
    assert l1.get("k") == "v"


def test_smartcache_prefetch_disabled():
    sc = SmartCache(enable_prefetch=False)
    called = []
    sc.prefetch(["a", "b"], lambda k: called.append(k) or "v")
    assert called == []


def test_smartcache_prefetch_fetches_missing():
    sc = SmartCache(enable_prefetch=True)
    fetched = []

    def fetch(k):
        fetched.append(k)
        return f"val-{k}"

    sc.prefetch(["a", "b"], fetch)
    assert sc.get("a") == "val-a"
    assert sc.get("b") == "val-b"
    assert set(fetched) == {"a", "b"}


def test_smartcache_prefetch_swallows_errors():
    sc = SmartCache(enable_prefetch=True)

    def boom(k):
        raise RuntimeError("nope")

    sc.prefetch(["a"], boom)  # must not raise
    assert sc.get("a") is None


def test_smartcache_prefetch_skips_present():
    sc = SmartCache(enable_prefetch=True)
    sc.set("a", "existing")
    fetched = []
    sc.prefetch(["a"], lambda k: fetched.append(k) or "new")
    assert sc.get("a") == "existing"
    assert fetched == []


def test_smartcache_get_metrics():
    l1 = MemoryCacheBackend()
    l2 = MemoryCacheBackend()
    sc = SmartCache(l1_backend=l1, l2_backend=l2)
    m = sc.get_metrics()
    assert "l1" in m
    assert "l2" in m
    assert isinstance(m["l1"], CacheMetrics)


def test_smartcache_get_metrics_no_l2():
    sc = SmartCache(l2_backend=None)
    m = sc.get_metrics()
    assert "l1" in m
    assert "l2" not in m


def test_smartcache_track_access_pattern_builds():
    sc = SmartCache(enable_prefetch=True, prefetch_threshold=2)
    # seed a pattern bucket so _track_access has something to append to
    sc._access_patterns["seed"] = ["other"]
    sc.set("k", "v")
    sc.get("k")  # triggers _track_access
    assert "k" in sc._access_patterns["seed"]


def test_smartcache_track_access_continue_same_key():
    sc = SmartCache(enable_prefetch=True, prefetch_threshold=2)
    # bucket whose last element == the accessed key -> hits the `continue`
    sc._access_patterns["b1"] = ["k"]
    # bucket already at 10 entries -> append makes 11 (>10) -> pop(0) trims to 10
    sc._access_patterns["b2"] = [str(i) for i in range(10)]
    sc.set("k", "v")
    sc.get("k")
    # b1 skipped (still just ["k"]); b2 appended then trimmed back to 10
    assert sc._access_patterns["b1"] == ["k"]
    assert len(sc._access_patterns["b2"]) == 10
    assert sc._access_patterns["b2"][-1] == "k"


def test_smartcache_track_access_disabled():
    sc = SmartCache(enable_prefetch=False)
    sc.set("k", "v")
    sc.get("k")
    assert sc._access_patterns == {}


# --------------------------------------------------------------------------
# ObjectPool  (patch the background cleanup thread so nothing sleeps)
# --------------------------------------------------------------------------
@pytest.fixture
def no_cleanup_thread():
    with mock.patch.object(ObjectPool, "_cleanup_loop", lambda self: None):
        yield


class _Cfg:
    def __init__(self, strategy="s", chunk_size=10):
        self.strategy = strategy
        self.chunk_size = chunk_size


def _key(cfg):
    return f"{cfg.strategy}_{cfg.chunk_size}"


def test_objectpool_acquire_creates_then_reuses(no_cleanup_thread):
    created = []

    def factory(cfg):
        obj = mock.MagicMock()
        obj._pool_key = None
        created.append(obj)
        return obj

    pool = ObjectPool(factory=factory, key_func=_key)
    cfg = _Cfg()
    obj1 = pool.acquire(cfg)
    assert len(created) == 1
    assert obj1._pool_key == _key(cfg)
    assert pool.metrics.misses == 1
    assert pool.metrics.objects_created == 1

    pool.release(obj1, cfg)
    assert pool.metrics.releases == 1

    obj2 = pool.acquire(cfg)
    assert obj2 is obj1  # reused from pool
    assert pool.metrics.hits == 1
    assert len(created) == 1


def test_objectpool_release_by_config_no_pool_key(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key)
    cfg = _Cfg()
    obj = object()  # no _pool_key attr
    pool.release(obj, cfg)
    assert len(pool._pools[_key(cfg)]) == 1


def test_objectpool_release_no_key_no_config_noop(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key)
    obj = object()
    pool.release(obj)  # cannot determine pool -> returns silently
    assert sum(len(p) for p in pool._pools.values()) == 0


def test_objectpool_release_full_pool_discards(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key, max_pool_size=1)
    cfg = _Cfg()
    pool.release(object(), cfg)  # fills pool
    pool.release(object(), cfg)  # full -> discarded
    assert pool.metrics.objects_discarded == 1
    assert len(pool._pools[_key(cfg)]) == 1


def test_objectpool_release_no_metrics(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key, enable_metrics=False)
    assert pool.metrics is None
    cfg = _Cfg()
    obj = pool.acquire(cfg)
    pool.release(obj, cfg)  # no metrics branch
    assert len(pool._pools[_key(cfg)]) == 1


def test_objectpool_clear_pool_and_all(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key)
    cfg1 = _Cfg("s", 10)
    cfg2 = _Cfg("t", 20)
    pool.release(object(), cfg1)
    pool.release(object(), cfg2)
    assert pool.clear_pool(_key(cfg1)) == 1
    assert pool.clear_all() == 1  # cfg2 remaining


def test_objectpool_get_stats_with_metrics(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key)
    cfg = _Cfg()
    o = pool.acquire(cfg)
    pool.release(o, cfg)
    pool.acquire(cfg)  # hit
    stats = pool.get_stats()
    assert stats["active_pools"] >= 1
    assert "hit_rate_percent" in stats
    assert stats["total_acquisitions"] == 2
    assert stats["cache_hits"] == 1
    assert stats["cache_misses"] == 1


def test_objectpool_get_stats_no_metrics(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key, enable_metrics=False)
    pool.release(object(), _Cfg())
    stats = pool.get_stats()
    assert "hit_rate_percent" not in stats
    assert stats["total_objects"] == 1


def test_objectpool_get_stats_zero_acquisitions_hit_rate(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key)
    stats = pool.get_stats()
    assert stats["hit_rate_percent"] == 0


def test_objectpool_cleanup_idle_pools_removes_stale(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key, max_idle_time=10.0)
    cfg = _Cfg()
    pool.acquire(cfg)  # records last_access
    key = _key(cfg)
    # force last_access far in the past
    pool._last_access[key] = 0.0
    with mock.patch("proximadb_sdk.cache.time.time", return_value=1000.0):
        pool._cleanup_idle_pools()
    assert key not in pool._pools
    assert key not in pool._last_access
    assert pool.metrics.pools_cleaned == 1


def test_objectpool_cleanup_idle_pools_keeps_fresh(no_cleanup_thread):
    pool = ObjectPool(factory=lambda c: object(), key_func=_key, max_idle_time=10000.0)
    cfg = _Cfg()
    pool.acquire(cfg)
    key = _key(cfg)
    pool._cleanup_idle_pools()
    assert key in pool._last_access


def test_objectpool_metrics_dataclass_defaults():
    m = ObjectPoolMetrics()
    assert m.acquisitions == 0
    assert m.objects_cleaned == 0
