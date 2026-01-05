"""
SFR (Salesforce Research) Embedding Provider

Provides access to Salesforce Research's SFR-Embedding models which are among
the top-performing open-weights models on the MTEB leaderboard, consistently
ranking #1 or #2 for retrieval tasks.

Top SFR Models (Open Source):
- Salesforce/SFR-Embedding-2_R: Top MTEB performer (4096 dims) - Best accuracy
- Salesforce/SFR-Embedding-Mistral: Excellent quality (4096 dims) - Mistral-based
"""

import numpy as np
from typing import List, Optional, Dict, Any
import logging

from .base import EmbeddingProvider, EmbeddingConfig

logger = logging.getLogger(__name__)


class SFREmbeddingProvider(EmbeddingProvider):
    """
    SFR (Salesforce Research) Embedding Provider

    SFR-Embedding models from Salesforce Research are among the best open-weights
    models available, consistently ranking at the top of MTEB leaderboard. They
    produce high-dimensional embeddings (4096) for maximum accuracy.

    Key features:
    - Top-tier accuracy on retrieval tasks
    - 4096-dimensional embeddings for fine-grained representations
    - Uses special query instruction for optimal retrieval
    - Based on Mistral architecture (efficient and fast)

    Usage:
        config = EmbeddingConfig(
            model_name="Salesforce/SFR-Embedding-2_R",
            dimension=4096
        )
        provider = SFREmbeddingProvider(config)

        # For queries
        query_emb = provider.embed_query("What is machine learning?")

        # For passages/documents
        doc_embs = provider.embed_documents([{"text": "ML is..."}])
    """

    # SFR model specifications
    SFR_MODELS = {
        "Salesforce/SFR-Embedding-2_R": {
            "dimension": 4096,
            "max_length": 4096,
            "description": "Top MTEB performer, best accuracy, retrieval-optimized",
            "use_case": "Maximum accuracy, research, when quality is paramount",
            "mteb_score": 66.4,  # Average MTEB score as of 2024
            "architecture": "Mistral-based",
        },
        "Salesforce/SFR-Embedding-Mistral": {
            "dimension": 4096,
            "max_length": 4096,
            "description": "Excellent quality, Mistral-based architecture",
            "use_case": "High accuracy, production when quality > speed",
            "mteb_score": 64.8,
            "architecture": "Mistral-7B",
        },
    }

    # Instruction prefix for queries (critical for performance)
    QUERY_INSTRUCTION = "Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: "

    def __init__(self, config: Optional[EmbeddingConfig] = None):
        """Initialize SFR provider with optional config"""
        self.config = config if config is not None else self._get_default_config()
        self._available = None
        self.model = None
        self._initialize()

    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration - uses SFR-Embedding-2_R for best accuracy"""
        return EmbeddingConfig(
            model_name="Salesforce/SFR-Embedding-2_R",
            dimension=4096,
            batch_size=16,  # Smaller batch due to large dimensions
            normalize=True,  # Required for cosine similarity
            cache_embeddings=True,
            device=None,  # Auto-detect
            max_length=4096,
            extra_params={
                "use_query_instruction": False,  # Set True for queries, False for passages
                "trust_remote_code": True,  # Required for SFR models
            },
        )

    def _initialize(self) -> None:
        """Initialize the SFR model using sentence-transformers"""
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

            # Update dimension if it's a known SFR model
            if self.config.model_name in self.SFR_MODELS:
                model_info = self.SFR_MODELS[self.config.model_name]
                self.config.dimension = model_info["dimension"]
                self.config.max_length = model_info["max_length"]
            else:
                # Get dimension from model
                dummy_embedding = self.model.encode(["test"], show_progress_bar=False)
                self.config.dimension = dummy_embedding.shape[1]

            self._available = True
            logger.info(
                f"Initialized SFR model: {self.config.model_name} "
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
            logger.error(f"Failed to initialize SFR model: {e}")

    def _apply_instruction(self, texts: List[str], is_query: bool = None) -> List[str]:
        """
        Apply SFR instruction prefix if configured

        SFR models require special instruction for queries to achieve top performance.
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
            Embedding vector as numpy array (4096 dimensions)
        """
        return self.embed_texts([text], is_query=is_query)[0]

    def embed_texts(self, texts: List[str], is_query: bool = None) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        Args:
            texts: List of texts to embed
            is_query: If True, apply query instruction prefix for better retrieval

        Returns:
            Array of embeddings with shape (len(texts), 4096)
        """
        if not self._available:
            raise RuntimeError(
                "SFR model not available. "
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
            Query embedding vector (4096 dimensions)
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
        """Get embedding dimension (4096 for SFR models)"""
        return self.config.dimension

    def get_model_info(self) -> Dict[str, Any]:
        """Get model information"""
        info = {
            "model_name": self.config.model_name,
            "dimension": self.config.dimension,
            "max_length": self.config.max_length,
            "provider": "sfr",
            "normalize": self.config.normalize,
            "available": self._available,
        }

        # Add model-specific info if it's a known SFR model
        if self.config.model_name in self.SFR_MODELS:
            model_spec = self.SFR_MODELS[self.config.model_name]
            info.update(
                {
                    "description": model_spec["description"],
                    "use_case": model_spec["use_case"],
                    "mteb_score": model_spec.get("mteb_score"),
                    "architecture": model_spec.get("architecture"),
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
        """List available SFR models with their specifications"""
        return cls.SFR_MODELS.copy()

    @classmethod
    def get_recommended_model(cls, use_case: str = "best") -> str:
        """
        Get recommended SFR model for a use case

        Args:
            use_case: One of "best", "mistral_based"

        Returns:
            Model name
        """
        recommendations = {
            "best": "Salesforce/SFR-Embedding-2_R",
            "mistral_based": "Salesforce/SFR-Embedding-Mistral",
            "top_accuracy": "Salesforce/SFR-Embedding-2_R",
        }

        return recommendations.get(use_case, "Salesforce/SFR-Embedding-2_R")
