"""
Model caching system

Provides thread-safe caching of loaded models to reduce memory usage
and improve initialization performance.
"""

import logging
import threading
from typing import Any, Callable, Dict, Optional

logger = logging.getLogger(__name__)


class ModelCache:
    """
    Thread-safe singleton model cache

    This cache allows sharing model instances across multiple provider instances,
    reducing memory usage and initialization time.

    Example:
        # Provider 1
        cache = ModelCache()
        model1 = cache.get_or_load("model-key", lambda: load_heavy_model())

        # Provider 2 (reuses same model instance)
        cache = ModelCache()
        model2 = cache.get_or_load("model-key", lambda: load_heavy_model())

        # model1 is model2 -> True (same instance)
    """

    _instance: Optional["ModelCache"] = None
    _lock = threading.Lock()
    _models: Dict[str, Any] = {}
    _stats: Dict[str, int] = {"hits": 0, "misses": 0, "loads": 0}

    def __new__(cls):
        """Singleton pattern"""
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:
                    cls._instance = super().__new__(cls)
        return cls._instance

    def get_or_load(
        self, key: str, loader: Callable[[], Any], force_reload: bool = False
    ) -> Any:
        """
        Get cached model or load it

        Args:
            key: Unique cache key for the model
            loader: Function that loads and returns the model (called only if not cached)
            force_reload: If True, reload model even if cached

        Returns:
            Loaded model instance

        Example:
            >>> from sentence_transformers import SentenceTransformer
            >>> cache = ModelCache()
            >>> model = cache.get_or_load(
            ...     "bge-small",
            ...     lambda: SentenceTransformer("BAAI/bge-small-en-v1.5")
            ... )
        """
        if force_reload:
            with self._lock:
                logger.info(f"Force reloading model: {key}")
                self._models.pop(key, None)

        # Fast path: model already cached
        if key in self._models:
            self._stats["hits"] += 1
            logger.debug(f"Cache hit: {key}")
            return self._models[key]

        # Slow path: load model
        with self._lock:
            # Double-check locking pattern
            if key in self._models:
                self._stats["hits"] += 1
                logger.debug(f"Cache hit (after lock): {key}")
                return self._models[key]

            # Load model
            self._stats["misses"] += 1
            self._stats["loads"] += 1
            logger.info(f"Cache miss, loading model: {key}")

            try:
                model = loader()
                self._models[key] = model
                logger.info(f"Model loaded and cached: {key}")
                return model
            except Exception as e:
                logger.error(f"Failed to load model {key}: {e}")
                raise

    def get(self, key: str) -> Optional[Any]:
        """
        Get model from cache without loading

        Args:
            key: Cache key

        Returns:
            Cached model or None if not found

        Example:
            >>> cache = ModelCache()
            >>> model = cache.get("bge-small")
            >>> if model is None:
            ...     print("Model not cached")
        """
        with self._lock:
            return self._models.get(key)

    def clear(self, key: Optional[str] = None):
        """
        Clear cache entry or entire cache

        Args:
            key: Optional cache key. If None, clear entire cache

        Example:
            >>> cache = ModelCache()
            >>> cache.clear("bge-small")  # Clear specific model
            >>> cache.clear()  # Clear all models
        """
        with self._lock:
            if key:
                if key in self._models:
                    logger.info(f"Clearing cached model: {key}")
                    del self._models[key]
                else:
                    logger.warning(f"Model not in cache: {key}")
            else:
                count = len(self._models)
                logger.info(f"Clearing entire model cache ({count} models)")
                self._models.clear()

    def keys(self) -> list[str]:
        """
        Get list of cached model keys

        Returns:
            List of cache keys

        Example:
            >>> cache = ModelCache()
            >>> print(cache.keys())
            ['bge-small', 'gte-qwen-1.5b', 'sfr-embedding']
        """
        with self._lock:
            return list(self._models.keys())

    def size(self) -> int:
        """
        Get number of cached models

        Returns:
            Number of models in cache

        Example:
            >>> cache = ModelCache()
            >>> print(f"Cached models: {cache.size()}")
            Cached models: 3
        """
        with self._lock:
            return len(self._models)

    def stats(self) -> Dict[str, int]:
        """
        Get cache statistics

        Returns:
            Dictionary with hit/miss/load counts

        Example:
            >>> cache = ModelCache()
            >>> stats = cache.stats()
            >>> print(f"Hit rate: {stats['hits'] / (stats['hits'] + stats['misses']):.1%}")
            Hit rate: 85.7%
        """
        with self._lock:
            return self._stats.copy()

    def reset_stats(self):
        """
        Reset cache statistics

        Example:
            >>> cache = ModelCache()
            >>> cache.reset_stats()
        """
        with self._lock:
            self._stats = {"hits": 0, "misses": 0, "loads": 0}
            logger.debug("Cache statistics reset")

    def __repr__(self) -> str:
        """String representation"""
        with self._lock:
            return (
                f"ModelCache(size={len(self._models)}, "
                f"hits={self._stats['hits']}, "
                f"misses={self._stats['misses']})"
            )


# Global convenience function
def get_model_cache() -> ModelCache:
    """
    Get the global model cache instance

    Returns:
        Singleton ModelCache instance

    Example:
        >>> cache = get_model_cache()
        >>> model = cache.get_or_load("my-model", loader_func)
    """
    return ModelCache()
