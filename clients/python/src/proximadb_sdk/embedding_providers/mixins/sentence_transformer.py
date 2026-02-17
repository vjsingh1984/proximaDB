"""
SentenceTransformer mixin

Provides sentence-transformers integration with model caching.
"""

import logging
from typing import List, Optional

import numpy as np

from ..core.cache import ModelCache

logger = logging.getLogger(__name__)


class SentenceTransformerMixin:
    """
    Mixin for sentence-transformers based providers

    This mixin provides:
    - Automatic model loading with caching
    - Standard embedding generation
    - Normalization support
    - Batch processing

    Usage:
        class MyProvider(SentenceTransformerMixin, BaseEmbeddingProvider):
            def default_config(self) -> ProviderConfig:
                return ProviderConfig(model=...)

    Note:
        This mixin assumes the provider has a `config` attribute of type ProviderConfig.
    """

    def _load_model(self):
        """
        Load sentence-transformer model with caching

        Uses ModelCache to share model instances across provider instances.

        Returns:
            Loaded SentenceTransformer model
        """
        try:
            from sentence_transformers import SentenceTransformer
        except ImportError:
            raise ImportError(
                "sentence-transformers is required for this provider. "
                "Install with: pip install sentence-transformers"
            )

        cache = ModelCache()
        cache_key = f"st_{self.config.model.name}_{self.config.trust_remote_code}"

        def loader():
            logger.info(f"Loading sentence-transformer model: {self.config.model.name}")
            model = SentenceTransformer(
                self.config.model.name,
                device=self.config.device,
                trust_remote_code=self.config.trust_remote_code,
                cache_folder=self.config.cache_dir,
            )
            logger.info(f"Model loaded: {self.config.model.name}")
            return model

        return cache.get_or_load(cache_key, loader)

    def embed(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings using sentence-transformers

        Args:
            texts: List of text strings to embed

        Returns:
            NumPy array of shape (len(texts), dimension)

        Example:
            >>> provider = MyProvider()
            >>> embeddings = provider.embed(["Hello", "World"])
            >>> print(embeddings.shape)
            (2, 384)
        """
        if not texts:
            return np.array([])

        self.ensure_initialized()

        logger.debug(
            f"Embedding {len(texts)} texts (batch_size={self.config.batch_size})"
        )

        embeddings = self._model.encode(
            texts,
            batch_size=self.config.batch_size,
            normalize_embeddings=self.config.normalize,
            show_progress_bar=False,
            convert_to_numpy=True,
        )

        return embeddings

    def embed_batch(
        self, texts: List[str], batch_size: Optional[int] = None
    ) -> np.ndarray:
        """
        Embed texts with custom batch size

        Args:
            texts: List of text strings
            batch_size: Custom batch size (overrides config)

        Returns:
            NumPy array of embeddings

        Example:
            >>> provider = MyProvider()
            >>> embeddings = provider.embed_batch(texts, batch_size=64)
        """
        if batch_size is not None:
            original_batch_size = self.config.batch_size
            self.config = self.config.merge(batch_size=batch_size)
            try:
                return self.embed(texts)
            finally:
                self.config = self.config.merge(batch_size=original_batch_size)
        else:
            return self.embed(texts)
