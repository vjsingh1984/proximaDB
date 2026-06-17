"""
E5 (EmbEddings from bidirEctional Encoder rEpresentations) Provider

Provides access to Microsoft's E5 embedding models which are among the best
open-source models available. E5 models use special prefixes for queries and
passages to improve retrieval performance.

Top E5 Models (Open Source):
- intfloat/e5-large-v2: Best quality (1024 dims) - Top MTEB performer
- intfloat/e5-base-v2: Balanced (768 dims) - Great quality/speed
- intfloat/e5-small-v2: Fast (384 dims) - Excellent for production
- intfloat/multilingual-e5-large: Multilingual (1024 dims) - 100+ languages
"""

import logging
from typing import Any

import numpy as np

from .base import EmbeddingConfig, EmbeddingProvider

logger = logging.getLogger(__name__)


class E5EmbeddingProvider(EmbeddingProvider):
    """
    E5 (Text Embeddings by Weakly-Supervised Contrastive Pre-training) provider

    E5 models from Microsoft are state-of-the-art open-source embeddings that
    require special prefixes for optimal performance:
    - Queries: "query: " prefix
    - Passages: "passage: " prefix

    These prefixes are crucial for achieving top performance on retrieval tasks.

    Usage:
        config = EmbeddingConfig(
            model_name="intfloat/e5-large-v2",
            dimension=1024
        )
        provider = E5EmbeddingProvider(config)

        # For queries
        query_emb = provider.embed_query("what is machine learning?")

        # For passages/documents
        doc_embs = provider.embed_documents([{"text": "ML is..."}])
    """

    # E5 model specifications
    E5_MODELS = {
        "intfloat/e5-large-v2": {
            "dimension": 1024,
            "max_length": 512,
            "description": "Best quality English embeddings, top MTEB performer",
            "use_case": "Maximum accuracy, research, production when quality critical",
        },
        "intfloat/e5-base-v2": {
            "dimension": 768,
            "max_length": 512,
            "description": "Balanced quality and speed for English",
            "use_case": "Production use, good balance of quality and speed",
        },
        "intfloat/e5-small-v2": {
            "dimension": 384,
            "max_length": 512,
            "description": "Fast and efficient for English",
            "use_case": "High throughput, latency-sensitive applications",
        },
        "intfloat/multilingual-e5-large": {
            "dimension": 1024,
            "max_length": 512,
            "description": "Multilingual model supporting 100+ languages",
            "use_case": "Cross-lingual retrieval, multilingual applications",
        },
        "intfloat/multilingual-e5-base": {
            "dimension": 768,
            "max_length": 512,
            "description": "Balanced multilingual model",
            "use_case": "Multilingual production use",
        },
        "intfloat/multilingual-e5-small": {
            "dimension": 384,
            "max_length": 512,
            "description": "Fast multilingual model",
            "use_case": "Multilingual high-throughput applications",
        },
    }

    # E5 instruction prefixes (critical for performance)
    QUERY_PREFIX = "query: "
    PASSAGE_PREFIX = "passage: "

    def __init__(self, config: EmbeddingConfig | None = None):
        """Initialize E5 provider with optional config"""
        self.config = config if config is not None else self._get_default_config()
        self._available = None
        self.model = None
        self._initialize()

    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration - uses e5-base-v2 for balance"""
        return EmbeddingConfig(
            model_name="intfloat/e5-base-v2",
            dimension=768,
            batch_size=32,
            normalize=True,  # E5 models require normalization for cosine similarity
            cache_embeddings=True,
            device=None,  # Auto-detect
            max_length=512,
            extra_params={
                "auto_prefix": True,  # Automatically add query/passage prefixes
                "trust_remote_code": False,  # E5 models don't need this
            },
        )

    def _initialize(self) -> None:
        """Initialize the E5 model using sentence-transformers"""
        try:
            from sentence_transformers import SentenceTransformer

            # Initialize model
            self.model = SentenceTransformer(
                self.config.model_name, device=self.config.device
            )

            # Update dimension if it's a known E5 model
            if self.config.model_name in self.E5_MODELS:
                model_info = self.E5_MODELS[self.config.model_name]
                self.config.dimension = model_info["dimension"]
                self.config.max_length = model_info["max_length"]
            else:
                # Get dimension from model
                dummy_embedding = self.model.encode(["test"], show_progress_bar=False)
                self.config.dimension = dummy_embedding.shape[1]

            self._available = True
            logger.info(
                f"Initialized E5 model: {self.config.model_name} "
                f"(dimension: {self.config.dimension}, max_length: {self.config.max_length})"
            )

        except ImportError:
            self._available = False
            logger.warning(
                "sentence-transformers not installed. "
                "Install with: pip install sentence-transformers"
            )
        except Exception as e:
            self._available = False
            logger.error(f"Failed to initialize E5 model: {e}")

    def _apply_prefix(self, texts: list[str], prefix: str) -> list[str]:
        """
        Apply E5 prefix to texts

        Args:
            texts: Input texts
            prefix: Prefix to apply ("query: " or "passage: ")

        Returns:
            Texts with prefix applied
        """
        return [prefix + text for text in texts]

    def embed_text(self, text: str, is_query: bool = None) -> np.ndarray:
        """
        Generate embedding for a single text

        Args:
            text: Text to embed
            is_query: If True, apply query prefix. If False, apply passage prefix.
                     If None, no prefix applied.

        Returns:
            Embedding vector as numpy array
        """
        return self.embed_texts([text], is_query=is_query)[0]

    def embed_texts(self, texts: list[str], is_query: bool = None) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        IMPORTANT: E5 models require prefixes for optimal performance:
        - Queries should have "query: " prefix
        - Passages should have "passage: " prefix

        Args:
            texts: List of texts to embed
            is_query: If True, apply query prefix. If False, apply passage prefix.
                     If None, check config for auto_prefix setting.

        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError(
                "E5 model not available. "
                "Install with: pip install sentence-transformers"
            )

        if not texts:
            return np.array([])

        # Apply appropriate prefix based on is_query parameter
        processed_texts = texts
        if is_query is not None:
            if is_query:
                processed_texts = self._apply_prefix(texts, self.QUERY_PREFIX)
            else:
                processed_texts = self._apply_prefix(texts, self.PASSAGE_PREFIX)

        # Generate embeddings
        embeddings = self.model.encode(
            processed_texts,
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
        Generate embeddings for documents (passages)

        Automatically applies "passage: " prefix for optimal retrieval.

        Args:
            documents: List of documents (dicts with text field)
            text_field: Name of field containing text

        Returns:
            Array of embedding vectors
        """
        texts = [doc.get(text_field, "") for doc in documents]
        # Documents are passages - use passage prefix
        return self.embed_texts(texts, is_query=False)

    def embed_query(self, query: str) -> np.ndarray:
        """
        Generate embedding for a search query

        Automatically applies "query: " prefix for optimal retrieval.

        Args:
            query: Search query text

        Returns:
            Query embedding vector
        """
        return self.embed_text(query, is_query=True)

    def embed_queries(self, queries: list[str]) -> np.ndarray:
        """
        Generate embeddings for multiple search queries

        Automatically applies "query: " prefix for optimal retrieval.

        Args:
            queries: List of search queries

        Returns:
            Array of query embeddings
        """
        return self.embed_texts(queries, is_query=True)

    def embed_passages(self, passages: list[str]) -> np.ndarray:
        """
        Generate embeddings for passages/documents

        Automatically applies "passage: " prefix for optimal retrieval.

        Args:
            passages: List of passage texts

        Returns:
            Array of passage embeddings
        """
        return self.embed_texts(passages, is_query=False)

    def get_dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension

    def get_model_info(self) -> dict[str, Any]:
        """Get model information"""
        info = {
            "model_name": self.config.model_name,
            "dimension": self.config.dimension,
            "max_length": self.config.max_length,
            "provider": "e5",
            "normalize": self.config.normalize,
            "available": self._available,
            "query_prefix": self.QUERY_PREFIX,
            "passage_prefix": self.PASSAGE_PREFIX,
        }

        # Add model-specific info if it's a known E5 model
        if self.config.model_name in self.E5_MODELS:
            model_spec = self.E5_MODELS[self.config.model_name]
            info.update(
                {
                    "description": model_spec["description"],
                    "use_case": model_spec["use_case"],
                }
            )

        return info

    def is_available(self) -> bool:
        """Check if provider is available"""
        if self._available is None:
            self._initialize()
        return self._available

    @classmethod
    def list_available_models(cls) -> dict[str, dict[str, Any]]:
        """List available E5 models with their specifications"""
        return cls.E5_MODELS.copy()

    @classmethod
    def get_recommended_model(cls, use_case: str = "balanced") -> str:
        """
        Get recommended E5 model for a use case

        Args:
            use_case: One of "best_quality", "balanced", "fast", "multilingual", "multilingual_fast"

        Returns:
            Model name
        """
        recommendations = {
            "best_quality": "intfloat/e5-large-v2",
            "balanced": "intfloat/e5-base-v2",
            "fast": "intfloat/e5-small-v2",
            "multilingual": "intfloat/multilingual-e5-large",
            "multilingual_balanced": "intfloat/multilingual-e5-base",
            "multilingual_fast": "intfloat/multilingual-e5-small",
        }

        return recommendations.get(use_case, "intfloat/e5-base-v2")
