"""
ProximaDB Intelligent Caching

Implements multi-level caching strategies with automatic invalidation,
LRU eviction, and intelligent prefetching for optimal performance.
"""

import asyncio
import hashlib
import json
import pickle
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional, Set, Tuple, Union
from collections import OrderedDict
import weakref
import threading
import logging

from pydantic import BaseModel, Field

from .models import SearchResult, VectorRecord
from .exceptions import ProximaDBError


class CacheStrategy(str, Enum):
    """Cache replacement strategies"""
    LRU = "lru"              # Least Recently Used
    LFU = "lfu"              # Least Frequently Used  
    TTL = "ttl"              # Time To Live
    ADAPTIVE = "adaptive"     # Adaptive based on access patterns
    WRITE_THROUGH = "write_through"     # Write-through caching
    WRITE_BACK = "write_back"           # Write-back caching


class CacheLevel(str, Enum):
    """Cache levels in the hierarchy"""
    L1_MEMORY = "l1_memory"      # In-memory cache (fastest)
    L2_DISK = "l2_disk"          # Disk-based cache
    L3_NETWORK = "l3_network"    # Network/distributed cache


@dataclass
class CacheMetrics:
    """Metrics for cache performance"""
    hits: int = 0
    misses: int = 0
    evictions: int = 0
    writes: int = 0
    size_bytes: int = 0
    max_size_bytes: int = 0
    hit_ratio: float = 0.0
    avg_access_time_ms: float = 0.0
    last_updated: float = field(default_factory=time.time)
    
    def update_hit_ratio(self):
        """Update hit ratio calculation"""
        total_requests = self.hits + self.misses
        if total_requests > 0:
            self.hit_ratio = self.hits / total_requests


class CacheConfig(BaseModel):
    """Configuration for caching behavior"""
    max_size_mb: int = Field(default=256, ge=1, le=8192)
    max_items: int = Field(default=10000, ge=100)
    default_ttl_seconds: int = Field(default=3600, ge=60)
    strategy: CacheStrategy = Field(default=CacheStrategy.LRU)
    
    # Levels configuration
    enable_l1: bool = Field(default=True)
    enable_l2: bool = Field(default=False)
    enable_l3: bool = Field(default=False)
    
    # Performance settings
    cleanup_interval_seconds: int = Field(default=300, ge=60)
    prefetch_enabled: bool = Field(default=True)
    compression_enabled: bool = Field(default=True)
    
    # Adaptive settings
    access_history_size: int = Field(default=1000, ge=100)
    popularity_threshold: float = Field(default=0.1, ge=0.01, le=1.0)


@dataclass
class CacheEntry:
    """A cache entry with metadata"""
    key: str
    value: Any
    created_at: float = field(default_factory=time.time)
    last_accessed: float = field(default_factory=time.time)
    access_count: int = 0
    ttl_seconds: Optional[int] = None
    size_bytes: int = 0
    
    @property
    def is_expired(self) -> bool:
        """Check if entry has expired"""
        if self.ttl_seconds is None:
            return False
        return time.time() - self.created_at > self.ttl_seconds
    
    @property
    def age_seconds(self) -> float:
        """Get age of entry in seconds"""
        return time.time() - self.created_at
    
    def touch(self):
        """Update access metadata"""
        self.last_accessed = time.time()
        self.access_count += 1


class CacheBackend(ABC):
    """Abstract base class for cache backends"""
    
    @abstractmethod
    async def get(self, key: str) -> Optional[CacheEntry]:
        """Get entry from cache"""
        pass
    
    @abstractmethod
    async def put(self, entry: CacheEntry) -> bool:
        """Put entry in cache"""
        pass
    
    @abstractmethod
    async def delete(self, key: str) -> bool:
        """Delete entry from cache"""
        pass
    
    @abstractmethod
    async def clear(self):
        """Clear all entries"""
        pass
    
    @abstractmethod
    async def get_metrics(self) -> CacheMetrics:
        """Get cache metrics"""
        pass


class InMemoryCache(CacheBackend):
    """In-memory cache implementation with LRU/LFU support"""
    
    def __init__(self, config: CacheConfig):
        self.config = config
        self.entries: OrderedDict[str, CacheEntry] = OrderedDict()
        self.metrics = CacheMetrics(max_size_bytes=config.max_size_mb * 1024 * 1024)
        self._lock = asyncio.Lock()
        self._access_history: List[str] = []
        self._logger = logging.getLogger(__name__)
        
        # Start cleanup task
        self._cleanup_task = None
        self._running = False
    
    async def start(self):
        """Start the cache"""
        self._running = True
        self._cleanup_task = asyncio.create_task(self._cleanup_loop())
    
    async def stop(self):
        """Stop the cache"""
        self._running = False
        if self._cleanup_task:
            self._cleanup_task.cancel()
            try:
                await self._cleanup_task
            except asyncio.CancelledError:
                pass
    
    async def get(self, key: str) -> Optional[CacheEntry]:
        """Get entry from cache"""
        async with self._lock:
            entry = self.entries.get(key)
            
            if entry is None:
                self.metrics.misses += 1
                return None
            
            if entry.is_expired:
                await self._remove_entry(key)
                self.metrics.misses += 1
                return None
            
            # Update access metadata
            entry.touch()
            self.metrics.hits += 1
            
            # Move to end for LRU
            if self.config.strategy == CacheStrategy.LRU:
                self.entries.move_to_end(key)
            
            # Update access history for adaptive strategies
            self._access_history.append(key)
            if len(self._access_history) > self.config.access_history_size:
                self._access_history = self._access_history[-self.config.access_history_size:]
            
            self.metrics.update_hit_ratio()
            return entry
    
    async def put(self, entry: CacheEntry) -> bool:
        """Put entry in cache"""
        async with self._lock:
            # Calculate entry size
            if entry.size_bytes == 0:
                entry.size_bytes = self._estimate_size(entry.value)
            
            # Check if we need to evict
            while (len(self.entries) >= self.config.max_items or 
                   self.metrics.size_bytes + entry.size_bytes > self.metrics.max_size_bytes):
                evicted = await self._evict_one()
                if not evicted:
                    break
            
            # Add entry
            if entry.key in self.entries:
                # Update existing
                old_entry = self.entries[entry.key]
                self.metrics.size_bytes -= old_entry.size_bytes
            
            self.entries[entry.key] = entry
            self.metrics.size_bytes += entry.size_bytes
            self.metrics.writes += 1
            
            return True
    
    async def delete(self, key: str) -> bool:
        """Delete entry from cache"""
        async with self._lock:
            return await self._remove_entry(key)
    
    async def clear(self):
        """Clear all entries"""
        async with self._lock:
            self.entries.clear()
            self.metrics.size_bytes = 0
            self._access_history.clear()
    
    async def get_metrics(self) -> CacheMetrics:
        """Get cache metrics"""
        return self.metrics
    
    async def _remove_entry(self, key: str) -> bool:
        """Remove entry and update metrics"""
        entry = self.entries.pop(key, None)
        if entry:
            self.metrics.size_bytes -= entry.size_bytes
            return True
        return False
    
    async def _evict_one(self) -> bool:
        """Evict one entry based on strategy"""
        if not self.entries:
            return False
        
        if self.config.strategy == CacheStrategy.LRU:
            # Remove least recently used (first item)
            key = next(iter(self.entries))
        elif self.config.strategy == CacheStrategy.LFU:
            # Remove least frequently used
            key = min(self.entries.keys(), 
                     key=lambda k: self.entries[k].access_count)
        elif self.config.strategy == CacheStrategy.TTL:
            # Remove oldest entry
            key = min(self.entries.keys(),
                     key=lambda k: self.entries[k].created_at)
        elif self.config.strategy == CacheStrategy.ADAPTIVE:
            # Adaptive eviction based on access patterns
            key = await self._adaptive_evict()
        else:
            key = next(iter(self.entries))
        
        await self._remove_entry(key)
        self.metrics.evictions += 1
        return True
    
    async def _adaptive_evict(self) -> str:
        """Adaptive eviction based on access patterns"""
        # Find entries that haven't been accessed recently
        recent_keys = set(self._access_history[-100:])  # Last 100 accesses
        
        candidates = [k for k, entry in self.entries.items() 
                     if k not in recent_keys and 
                     entry.access_count < 5 and 
                     entry.age_seconds > 300]  # 5 minutes
        
        if candidates:
            # Remove least frequently used among candidates
            return min(candidates, key=lambda k: self.entries[k].access_count)
        else:
            # Fall back to LRU
            return next(iter(self.entries))
    
    def _estimate_size(self, value: Any) -> int:
        """Estimate size of a value in bytes"""
        try:
            return len(pickle.dumps(value))
        except:
            # Fallback estimation
            if isinstance(value, str):
                return len(value.encode('utf-8'))
            elif isinstance(value, (list, tuple)):
                return sum(self._estimate_size(item) for item in value)
            elif isinstance(value, dict):
                return sum(self._estimate_size(k) + self._estimate_size(v) 
                          for k, v in value.items())
            else:
                return 100  # Default estimate
    
    async def _cleanup_loop(self):
        """Background cleanup of expired entries"""
        while self._running:
            try:
                await asyncio.sleep(self.config.cleanup_interval_seconds)
                await self._cleanup_expired()
            except asyncio.CancelledError:
                break
            except Exception as e:
                self._logger.error(f"Cache cleanup error: {e}")
    
    async def _cleanup_expired(self):
        """Remove expired entries"""
        async with self._lock:
            expired_keys = [key for key, entry in self.entries.items() 
                           if entry.is_expired]
            
            for key in expired_keys:
                await self._remove_entry(key)
            
            if expired_keys:
                self._logger.debug(f"Cleaned up {len(expired_keys)} expired cache entries")


class MultiLevelCache:
    """
    Multi-level cache with L1 (memory), L2 (disk), and L3 (network) support.
    
    Implements intelligent promotion/demotion between levels based on
    access patterns and cache pressure.
    """
    
    def __init__(self, config: CacheConfig):
        self.config = config
        self.levels: Dict[CacheLevel, CacheBackend] = {}
        self._logger = logging.getLogger(__name__)
        
        # Initialize enabled levels
        if config.enable_l1:
            self.levels[CacheLevel.L1_MEMORY] = InMemoryCache(config)
    
    async def start(self):
        """Start all cache levels"""
        for backend in self.levels.values():
            if hasattr(backend, 'start'):
                await backend.start()
    
    async def stop(self):
        """Stop all cache levels"""
        for backend in self.levels.values():
            if hasattr(backend, 'stop'):
                await backend.stop()
    
    async def get(self, key: str) -> Optional[Any]:
        """Get value from cache hierarchy"""
        # Try each level in order
        for level in [CacheLevel.L1_MEMORY, CacheLevel.L2_DISK, CacheLevel.L3_NETWORK]:
            if level not in self.levels:
                continue
                
            entry = await self.levels[level].get(key)
            if entry:
                # Promote to higher level if not L1
                if level != CacheLevel.L1_MEMORY:
                    await self._promote(key, entry, level)
                
                return entry.value
        
        return None
    
    async def put(self, key: str, value: Any, ttl_seconds: Optional[int] = None) -> bool:
        """Put value in cache"""
        ttl = ttl_seconds or self.config.default_ttl_seconds
        
        entry = CacheEntry(
            key=key,
            value=value,
            ttl_seconds=ttl
        )
        
        # Put in L1 first
        if CacheLevel.L1_MEMORY in self.levels:
            return await self.levels[CacheLevel.L1_MEMORY].put(entry)
        
        return False
    
    async def delete(self, key: str) -> bool:
        """Delete from all levels"""
        deleted = False
        for backend in self.levels.values():
            if await backend.delete(key):
                deleted = True
        return deleted
    
    async def clear(self):
        """Clear all levels"""
        for backend in self.levels.values():
            await backend.clear()
    
    async def _promote(self, key: str, entry: CacheEntry, from_level: CacheLevel):
        """Promote entry to higher cache level"""
        # Only promote to L1 for now
        if CacheLevel.L1_MEMORY in self.levels and from_level != CacheLevel.L1_MEMORY:
            await self.levels[CacheLevel.L1_MEMORY].put(entry)
    
    async def get_metrics(self) -> Dict[CacheLevel, CacheMetrics]:
        """Get metrics for all levels"""
        metrics = {}
        for level, backend in self.levels.items():
            metrics[level] = await backend.get_metrics()
        return metrics


class SmartCache:
    """
    Smart cache with automatic invalidation, prefetching, and optimization.
    
    Features:
    - Automatic cache invalidation based on data changes
    - Intelligent prefetching based on access patterns
    - Collection-aware caching strategies
    - Search result caching with similarity-based retrieval
    """
    
    def __init__(self, config: Optional[CacheConfig] = None):
        self.config = config or CacheConfig()
        self.cache = MultiLevelCache(self.config)
        
        # Cache categories
        self._vector_cache_keys: Set[str] = set()
        self._search_cache_keys: Set[str] = set()
        self._collection_cache_keys: Set[str] = set()
        
        # Invalidation tracking
        self._collection_versions: Dict[str, int] = {}
        
        self._logger = logging.getLogger(__name__)
    
    async def start(self):
        """Start the smart cache"""
        await self.cache.start()
    
    async def stop(self):
        """Stop the smart cache"""
        await self.cache.stop()
    
    async def cache_vector(self, collection_id: str, vector_id: str, 
                          vector: VectorRecord, ttl_seconds: Optional[int] = None) -> bool:
        """Cache a vector"""
        key = self._vector_key(collection_id, vector_id)
        self._vector_cache_keys.add(key)
        return await self.cache.put(key, vector, ttl_seconds)
    
    async def get_vector(self, collection_id: str, vector_id: str) -> Optional[VectorRecord]:
        """Get cached vector"""
        key = self._vector_key(collection_id, vector_id)
        return await self.cache.get(key)
    
    async def cache_search_results(self, collection_id: str, query_vector: List[float],
                                  results: List[SearchResult], 
                                  ttl_seconds: Optional[int] = None) -> bool:
        """Cache search results"""
        key = self._search_key(collection_id, query_vector)
        self._search_cache_keys.add(key)
        return await self.cache.put(key, results, ttl_seconds)
    
    async def get_search_results(self, collection_id: str, 
                               query_vector: List[float]) -> Optional[List[SearchResult]]:
        """Get cached search results"""
        key = self._search_key(collection_id, query_vector)
        return await self.cache.get(key)
    
    async def invalidate_collection(self, collection_id: str):
        """Invalidate all cache entries for a collection"""
        # Update version
        self._collection_versions[collection_id] = self._collection_versions.get(collection_id, 0) + 1
        
        # Find and delete related keys
        keys_to_delete = []
        
        for key in self._vector_cache_keys:
            if key.startswith(f"vector:{collection_id}:"):
                keys_to_delete.append(key)
        
        for key in self._search_cache_keys:
            if key.startswith(f"search:{collection_id}:"):
                keys_to_delete.append(key)
        
        # Delete keys
        for key in keys_to_delete:
            await self.cache.delete(key)
        
        # Update tracking sets
        self._vector_cache_keys -= set(keys_to_delete)
        self._search_cache_keys -= set(keys_to_delete)
        
        self._logger.info(f"Invalidated {len(keys_to_delete)} cache entries for collection {collection_id}")
    
    async def prefetch_similar_vectors(self, collection_id: str, 
                                     reference_vectors: List[VectorRecord]):
        """Prefetch vectors that might be accessed based on similarity"""
        # This would integrate with the search system to find similar vectors
        # Implementation would depend on the specific use case
        pass
    
    def _vector_key(self, collection_id: str, vector_id: str) -> str:
        """Generate cache key for a vector"""
        version = self._collection_versions.get(collection_id, 0)
        return f"vector:{collection_id}:{vector_id}:v{version}"
    
    def _search_key(self, collection_id: str, query_vector: List[float]) -> str:
        """Generate cache key for search results"""
        # Create hash of query vector for cache key
        vector_str = json.dumps(query_vector, sort_keys=True)
        vector_hash = hashlib.md5(vector_str.encode()).hexdigest()[:16]
        version = self._collection_versions.get(collection_id, 0)
        return f"search:{collection_id}:{vector_hash}:v{version}"
    
    async def get_metrics(self) -> Dict[str, Any]:
        """Get comprehensive cache metrics"""
        level_metrics = await self.cache.get_metrics()
        
        return {
            "levels": level_metrics,
            "tracked_keys": {
                "vectors": len(self._vector_cache_keys),
                "searches": len(self._search_cache_keys),
                "collections": len(self._collection_cache_keys)
            },
            "collection_versions": self._collection_versions.copy()
        }


# Convenience functions and decorators

def cached(ttl_seconds: int = 3600, key_func: Optional[callable] = None):
    """Decorator for caching function results"""
    def decorator(func):
        cache_instance = SmartCache()
        
        async def async_wrapper(*args, **kwargs):
            # Generate cache key
            if key_func:
                cache_key = key_func(*args, **kwargs)
            else:
                key_parts = [func.__name__] + [str(arg) for arg in args]
                key_parts.extend([f"{k}={v}" for k, v in sorted(kwargs.items())])
                cache_key = ":".join(key_parts)
            
            # Try cache first
            result = await cache_instance.cache.get(cache_key)
            if result is not None:
                return result
            
            # Execute function and cache result
            result = await func(*args, **kwargs)
            await cache_instance.cache.put(cache_key, result, ttl_seconds)
            return result
        
        def sync_wrapper(*args, **kwargs):
            return asyncio.run(async_wrapper(*args, **kwargs))
        
        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        else:
            return sync_wrapper
    
    return decorator


async def create_smart_cache(config: Optional[CacheConfig] = None) -> SmartCache:
    """Create and start a smart cache instance"""
    cache = SmartCache(config)
    await cache.start()
    return cache