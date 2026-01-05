"""
FastEmbed embedding provider

Uses the fastembed library (Apache 2.0 license) for fast,
lightweight embedding models with ONNX runtime.
"""

import numpy as np
from typing import List, Optional, Dict, Any
import logging

from .base import EmbeddingProvider, EmbeddingConfig

logger = logging.getLogger(__name__)


class FastEmbedProvider(EmbeddingProvider):
    """
    Embedding provider using FastEmbed models

    FastEmbed provides optimized ONNX models for fast inference
    with minimal dependencies. All models are quantized for speed
    and small model size.

    Popular models:
    - BAAI/bge-small-en-v1.5: 384 dims, fast and efficient
    - BAAI/bge-base-en-v1.5: 768 dims, better quality
    - sentence-transformers/all-MiniLM-L6-v2: 384 dims, classic choice
    - jinaai/jina-embeddings-v2-small-en: 512 dims, optimized for search

    All models use Apache 2.0 or similar permissive licenses.
    """

    # Model dimension mapping
    MODEL_DIMENSIONS = {
        "BAAI/bge-small-en-v1.5": 384,
        "BAAI/bge-base-en-v1.5": 768,
        "BAAI/bge-large-en-v1.5": 1024,
        "sentence-transformers/all-MiniLM-L6-v2": 384,
        "sentence-transformers/all-MiniLM-L12-v2": 384,
        "jinaai/jina-embeddings-v2-small-en": 512,
        "jinaai/jina-embeddings-v2-base-en": 768,
        "snowflake/snowflake-arctic-embed-xs": 384,
        "snowflake/snowflake-arctic-embed-s": 384,
        "snowflake/snowflake-arctic-embed-m": 768,
        "snowflake/snowflake-arctic-embed-l": 1024,
    }

    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration"""
        return EmbeddingConfig(
            model_name="BAAI/bge-small-en-v1.5",
            dimension=384,
            batch_size=256,  # FastEmbed handles large batches well
            normalize=True,
            cache_embeddings=True,
            device=None,  # CPU optimized
        )

    def _initialize(self) -> None:
        """Initialize the FastEmbed model"""
        try:
            from fastembed import TextEmbedding

            # Initialize model
            self.model = TextEmbedding(
                model_name=self.config.model_name,
                max_length=512,  # Standard max length
                normalize=self.config.normalize,
                cache_dir=None,  # Use default cache
            )

            # Update dimension
            if self.config.model_name in self.MODEL_DIMENSIONS:
                self.config.dimension = self.MODEL_DIMENSIONS[self.config.model_name]
            else:
                # Get dimension from model
                dummy_embedding = list(self.model.embed(["test"]))[0]
                self.config.dimension = len(dummy_embedding)

            self._available = True
            logger.info(
                f"Initialized FastEmbed with model: {self.config.model_name} "
                f"(dimension: {self.config.dimension})"
            )

        except ImportError:
            self._available = False
            logger.warning(
                "fastembed not installed. " "Install with: pip install fastembed"
            )
        except Exception as e:
            self._available = False
            logger.error(f"Failed to initialize FastEmbed: {e}")

    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        Args:
            texts: List of texts to embed

        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError(
                "FastEmbed not available. " "Install with: pip install fastembed"
            )

        if not texts:
            return np.array([])

        # Generate embeddings
        # FastEmbed returns a generator, so we need to convert to list
        embeddings_list = list(
            self.model.embed(texts, batch_size=self.config.batch_size)
        )

        # Convert to numpy array
        embeddings = np.array(embeddings_list)

        return embeddings

    @property
    def dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension

    @property
    def model_name(self) -> str:
        """Get model name"""
        return self.config.model_name

    def is_available(self) -> bool:
        """Check if provider is available"""
        if self._available is None:
            self._initialize()
        return self._available

    @classmethod
    def list_recommended_models(cls) -> Dict[str, Dict[str, Any]]:
        """List recommended models with their properties"""
        return {
            "BAAI/bge-small-en-v1.5": {
                "dimension": 384,
                "description": "Fast and efficient, great for most use cases",
                "speed": "very fast",
                "quality": "good",
                "size_mb": 33,
            },
            "BAAI/bge-base-en-v1.5": {
                "dimension": 768,
                "description": "Better quality, still fast",
                "speed": "fast",
                "quality": "very good",
                "size_mb": 109,
            },
            "jinaai/jina-embeddings-v2-small-en": {
                "dimension": 512,
                "description": "Optimized for search, supports long contexts",
                "speed": "fast",
                "quality": "very good",
                "size_mb": 33,
            },
            "snowflake/snowflake-arctic-embed-s": {
                "dimension": 384,
                "description": "Optimized for retrieval tasks",
                "speed": "very fast",
                "quality": "good",
                "size_mb": 33,
            },
        }

    @classmethod
    def list_all_models(cls) -> List[str]:
        """List all available models"""
        try:
            from fastembed import TextEmbedding

            return TextEmbedding.list_supported_models()
        except:
            return list(cls.MODEL_DIMENSIONS.keys())
