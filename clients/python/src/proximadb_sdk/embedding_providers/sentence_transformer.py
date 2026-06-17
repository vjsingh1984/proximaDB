"""
Sentence Transformer embedding provider

Uses the sentence-transformers library (Apache 2.0 license) to provide
access to hundreds of free embedding models including BERT variants.
"""

import logging
from typing import Any

import numpy as np

from .base import EmbeddingConfig, EmbeddingProvider

logger = logging.getLogger(__name__)


class SentenceTransformerProvider(EmbeddingProvider):
    """
    Embedding provider using sentence-transformers library

    This provider supports hundreds of free models from HuggingFace,
    including various BERT models, MiniLM, MPNet, and more.

    Popular models:
    - all-MiniLM-L6-v2: Fast and good quality (384 dims)
    - all-mpnet-base-v2: Best quality (768 dims)
    - paraphrase-MiniLM-L6-v2: Good for paraphrase (384 dims)
    - multi-qa-MiniLM-L6-cos-v1: Optimized for Q&A (384 dims)
    """

    # Model dimension mapping for popular models
    MODEL_DIMENSIONS = {
        "all-MiniLM-L6-v2": 384,
        "all-MiniLM-L12-v2": 384,
        "all-mpnet-base-v2": 768,
        "paraphrase-MiniLM-L6-v2": 384,
        "paraphrase-mpnet-base-v2": 768,
        "multi-qa-MiniLM-L6-cos-v1": 384,
        "multi-qa-mpnet-base-v1": 768,
        "distilbert-base-nli-mean-tokens": 768,
        "bert-base-nli-mean-tokens": 768,
    }

    def __init__(self, config: EmbeddingConfig | None = None):
        """Initialize the provider with optional config"""
        self.config = config if config is not None else self._get_default_config()
        self._available = None
        self.model = None
        self._initialize()

    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration"""
        return EmbeddingConfig(
            model_name="all-MiniLM-L6-v2",
            dimension=384,
            batch_size=32,
            normalize=True,
            cache_embeddings=True,
            device=None,  # Auto-detect
        )

    def _initialize(self) -> None:
        """Initialize the sentence transformer model"""
        try:
            from sentence_transformers import SentenceTransformer

            # Initialize model
            self.model = SentenceTransformer(
                self.config.model_name, device=self.config.device
            )

            # Update dimension if it's a known model
            if self.config.model_name in self.MODEL_DIMENSIONS:
                self.config.dimension = self.MODEL_DIMENSIONS[self.config.model_name]
            else:
                # Get dimension from model
                dummy_embedding = self.model.encode(["test"], show_progress_bar=False)
                self.config.dimension = dummy_embedding.shape[1]

            self._available = True
            logger.info(
                f"Initialized SentenceTransformer with model: {self.config.model_name} "
                f"(dimension: {self.config.dimension})"
            )

        except ImportError:
            self._available = False
            logger.warning(
                "sentence-transformers not installed. "
                "Install with: pip install sentence-transformers"
            )
        except Exception as e:
            self._available = False
            logger.error(f"Failed to initialize SentenceTransformer: {e}")

    def embed_text(self, text: str) -> np.ndarray:
        """
        Generate embedding for a single text

        Args:
            text: Text to embed

        Returns:
            Embedding vector as numpy array
        """
        return self.embed_texts([text])[0]

    def embed_texts(self, texts: list[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        Args:
            texts: List of texts to embed

        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError(
                "SentenceTransformer not available. "
                "Install with: pip install sentence-transformers"
            )

        if not texts:
            return np.array([])

        # Generate embeddings
        embeddings = self.model.encode(
            texts,
            batch_size=self.config.batch_size,
            show_progress_bar=False,
            normalize_embeddings=self.config.normalize,
            convert_to_numpy=True,
        )

        return embeddings

    def embed_documents(
        self, documents: list[dict[str, Any]], text_field: str = "text"
    ) -> np.ndarray:
        """
        Generate embeddings for documents

        Args:
            documents: List of documents (dicts with text field)
            text_field: Name of field containing text

        Returns:
            Array of embedding vectors
        """
        texts = [doc.get(text_field, "") for doc in documents]
        return self.embed_texts(texts)

    def get_dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension

    def get_model_info(self) -> dict[str, Any]:
        """Get model information"""
        return {
            "model_name": self.config.model_name,
            "dimension": self.config.dimension,
            "provider": "sentence-transformers",
            "normalize": self.config.normalize,
            "available": self._available,
        }

    def is_available(self) -> bool:
        """Check if provider is available"""
        if self._available is None:
            self._initialize()
        return self._available

    @classmethod
    def list_recommended_models(cls) -> dict[str, dict[str, Any]]:
        """List recommended models with their properties"""
        return {
            "all-MiniLM-L6-v2": {
                "dimension": 384,
                "description": "Fast and good quality, recommended for most use cases",
                "speed": "fast",
                "quality": "good",
            },
            "all-mpnet-base-v2": {
                "dimension": 768,
                "description": "Best quality, slower than MiniLM",
                "speed": "medium",
                "quality": "excellent",
            },
            "paraphrase-MiniLM-L6-v2": {
                "dimension": 384,
                "description": "Optimized for paraphrase detection",
                "speed": "fast",
                "quality": "good",
            },
            "multi-qa-MiniLM-L6-cos-v1": {
                "dimension": 384,
                "description": "Optimized for question-answering tasks",
                "speed": "fast",
                "quality": "good",
            },
        }
