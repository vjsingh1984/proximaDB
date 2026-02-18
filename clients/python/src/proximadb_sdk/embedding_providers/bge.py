"""
BGE (Beijing Academy of Artificial Intelligence) Embedding Provider

Provides access to BAAI's state-of-the-art BGE embedding models which consistently
rank at the top of the MTEB leaderboard. These models are optimized for both
English and multilingual tasks.

Top BGE Models (Open Source):
- BAAI/bge-large-en-v1.5: Best quality English (1024 dims) - Top MTEB performer
- BAAI/bge-base-en-v1.5: Balanced English (768 dims) - Great quality/speed tradeoff
- BAAI/bge-small-en-v1.5: Fast English (384 dims) - Excellent for production
- BAAI/bge-m3: Multilingual (1024 dims) - Supports 100+ languages
"""

import logging
from typing import Any, Dict, List, Optional

import numpy as np

from .base import EmbeddingConfig, EmbeddingProvider

logger = logging.getLogger(__name__)


class BGEEmbeddingProvider(EmbeddingProvider):
    """
    BGE (BAAI General Embedding) provider using sentence-transformers

    BGE models are among the best open-source embedding models available,
    consistently ranking at the top of MTEB leaderboard. They use a special
    instruction prefix for better retrieval performance.

    Usage:
        config = EmbeddingConfig(
            model_name="BAAI/bge-large-en-v1.5",
            dimension=1024
        )
        provider = BGEEmbeddingProvider(config)
        embeddings = provider.embed_texts(["your text here"])
    """

    # BGE model specifications
    BGE_MODELS = {
        "BAAI/bge-large-en-v1.5": {
            "dimension": 1024,
            "max_length": 512,
            "description": "Best quality English embeddings, top MTEB performer",
            "use_case": "Maximum accuracy, research, when quality > speed",
        },
        "BAAI/bge-base-en-v1.5": {
            "dimension": 768,
            "max_length": 512,
            "description": "Balanced quality and speed for English",
            "use_case": "Production use, good balance",
        },
        "BAAI/bge-small-en-v1.5": {
            "dimension": 384,
            "max_length": 512,
            "description": "Fast and efficient for English",
            "use_case": "High throughput, latency-sensitive applications",
        },
        "BAAI/bge-m3": {
            "dimension": 1024,
            "max_length": 8192,
            "description": "Multilingual model supporting 100+ languages",
            "use_case": "Cross-lingual retrieval, multilingual applications",
        },
    }

    # Instruction prefix for queries (improves retrieval performance)
    QUERY_INSTRUCTION = "Represent this sentence for searching relevant passages: "

    def __init__(self, config: Optional[EmbeddingConfig] = None):
        """Initialize BGE provider with optional config"""
        self.config = config if config is not None else self._get_default_config()
        self._available = None
        self.model = None
        self._initialize()

    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration - uses bge-base-en-v1.5 for balance"""
        return EmbeddingConfig(
            model_name="BAAI/bge-base-en-v1.5",
            dimension=768,
            batch_size=32,
            normalize=True,  # BGE models benefit from normalization
            cache_embeddings=True,
            device=None,  # Auto-detect
            max_length=512,
            extra_params={
                "use_query_instruction": False,  # Set True for queries, False for passages
                "trust_remote_code": True,  # Required for some BGE models
            },
        )

    def _initialize(self) -> None:
        """Initialize the BGE model using sentence-transformers"""
        try:
            from sentence_transformers import SentenceTransformer

            # Get extra params
            extra_params = self.config.extra_params or {}
            trust_remote_code = extra_params.get("trust_remote_code", True)

            # Initialize model
            self.model = SentenceTransformer(
                self.config.model_name,
                device=self.config.device,
                trust_remote_code=trust_remote_code,
            )

            # Update dimension if it's a known BGE model
            if self.config.model_name in self.BGE_MODELS:
                model_info = self.BGE_MODELS[self.config.model_name]
                self.config.dimension = model_info["dimension"]
                self.config.max_length = model_info["max_length"]
            else:
                # Get dimension from model
                dummy_embedding = self.model.encode(["test"], show_progress_bar=False)
                self.config.dimension = dummy_embedding.shape[1]

            self._available = True
            logger.info(
                f"Initialized BGE model: {self.config.model_name} "
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
            logger.error(f"Failed to initialize BGE model: {e}")

    def _apply_instruction(self, texts: List[str], is_query: bool = None) -> List[str]:
        """
        Apply BGE instruction prefix if configured

        BGE models perform better when queries are prefixed with an instruction.
        Passages should NOT be prefixed.

        Args:
            texts: Input texts
            is_query: If True, apply query instruction. If None, check config.

        Returns:
            Texts with instruction prefix applied if appropriate
        """
        extra_params = self.config.extra_params or {}
        use_instruction = (
            is_query
            if is_query is not None
            else extra_params.get("use_query_instruction", False)
        )

        if use_instruction:
            return [self.QUERY_INSTRUCTION + text for text in texts]
        return texts

    def embed_text(self, text: str, is_query: bool = None) -> np.ndarray:
        """
        Generate embedding for a single text

        Args:
            text: Text to embed
            is_query: If True, apply query instruction prefix

        Returns:
            Embedding vector as numpy array
        """
        return self.embed_texts([text], is_query=is_query)[0]

    def embed_texts(self, texts: List[str], is_query: bool = None) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        Args:
            texts: List of texts to embed
            is_query: If True, apply query instruction prefix for better retrieval

        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError(
                "BGE model not available. "
                "Install with: pip install sentence-transformers"
            )

        if not texts:
            return np.array([])

        # Apply instruction prefix if this is for queries
        processed_texts = self._apply_instruction(texts, is_query=is_query)

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
        self, documents: List[Dict[str, Any]], text_field: str = "text"
    ) -> np.ndarray:
        """
        Generate embeddings for documents (passages, NOT queries)

        Args:
            documents: List of documents (dicts with text field)
            text_field: Name of field containing text

        Returns:
            Array of embedding vectors
        """
        texts = [doc.get(text_field, "") for doc in documents]
        # Documents are passages, not queries - don't use instruction
        return self.embed_texts(texts, is_query=False)

    def embed_query(self, query: str) -> np.ndarray:
        """
        Generate embedding for a search query

        This is a convenience method that automatically applies the query instruction.

        Args:
            query: Search query text

        Returns:
            Query embedding vector
        """
        return self.embed_text(query, is_query=True)

    def embed_queries(self, queries: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple search queries

        This is a convenience method that automatically applies the query instruction.

        Args:
            queries: List of search queries

        Returns:
            Array of query embeddings
        """
        return self.embed_texts(queries, is_query=True)

    def get_dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension

    def get_model_info(self) -> Dict[str, Any]:
        """Get model information"""
        info = {
            "model_name": self.config.model_name,
            "dimension": self.config.dimension,
            "max_length": self.config.max_length,
            "provider": "bge",
            "normalize": self.config.normalize,
            "available": self._available,
        }

        # Add model-specific info if it's a known BGE model
        if self.config.model_name in self.BGE_MODELS:
            model_spec = self.BGE_MODELS[self.config.model_name]
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
    def list_available_models(cls) -> Dict[str, Dict[str, Any]]:
        """List available BGE models with their specifications"""
        return cls.BGE_MODELS.copy()

    @classmethod
    def get_recommended_model(cls, use_case: str = "balanced") -> str:
        """
        Get recommended BGE model for a use case

        Args:
            use_case: One of "best_quality", "balanced", "fast", "multilingual"

        Returns:
            Model name
        """
        recommendations = {
            "best_quality": "BAAI/bge-large-en-v1.5",
            "balanced": "BAAI/bge-base-en-v1.5",
            "fast": "BAAI/bge-small-en-v1.5",
            "multilingual": "BAAI/bge-m3",
        }

        return recommendations.get(use_case, "BAAI/bge-base-en-v1.5")
