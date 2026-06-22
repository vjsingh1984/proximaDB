"""
FastEmbed embedding provider

Uses the fastembed library (Apache 2.0) for fast, lightweight ONNX embedding
models with minimal dependencies.
"""

import logging
from typing import Any

import numpy as np

from .core.base import BaseEmbeddingProvider
from .core.config import ModelMetadata, ProviderConfig
from .core.registry import ProviderRegistry

logger = logging.getLogger(__name__)

FASTEMBED_MODELS = {
    "BAAI/bge-small-en-v1.5": ModelMetadata(
        name="BAAI/bge-small-en-v1.5",
        dimension=384,
        max_length=512,
        provider_type="onnx",
        languages="en",
        description="Fast and efficient, great for most use cases",
        use_case="High-throughput CPU inference",
    ),
    "BAAI/bge-base-en-v1.5": ModelMetadata(
        name="BAAI/bge-base-en-v1.5",
        dimension=768,
        max_length=512,
        provider_type="onnx",
        languages="en",
        description="Better quality, still fast",
        use_case="Balanced CPU inference",
    ),
    "sentence-transformers/all-MiniLM-L6-v2": ModelMetadata(
        name="sentence-transformers/all-MiniLM-L6-v2",
        dimension=384,
        max_length=512,
        provider_type="onnx",
        languages="en",
        description="Classic lightweight model",
        use_case="Edge / low-resource inference",
    ),
}

# Known dimensions for models not in the curated metadata table above.
_MODEL_DIMENSIONS = {
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


@ProviderRegistry.register(
    name="fastembed",
    models=FASTEMBED_MODELS,
    aliases=["qdrant-fastembed"],
    description="FastEmbed ONNX embeddings (fast, lightweight, Apache 2.0)",
)
class FastEmbedProvider(BaseEmbeddingProvider):
    """
    Embedding provider using FastEmbed models.

    FastEmbed provides optimized ONNX models for fast CPU inference with minimal
    dependencies. Install with ``pip install fastembed``.
    """

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=FASTEMBED_MODELS["BAAI/bge-small-en-v1.5"],
            batch_size=256,  # FastEmbed handles large batches well
            normalize=True,
            extra={"max_length": 512},
        )

    def _load_model(self) -> Any:
        try:
            from fastembed import TextEmbedding
        except ImportError as exc:  # pragma: no cover - exercised via stub
            raise ImportError(
                "fastembed is required for FastEmbedProvider. "
                "Install with: pip install fastembed"
            ) from exc

        model = TextEmbedding(
            model_name=self.config.model.name,
            max_length=self.config.extra.get("max_length", 512),
            cache_dir=self.config.cache_dir,
        )
        logger.info(
            "Initialized FastEmbed with model %s (dimension %s)",
            self.config.model.name,
            self.get_dimension(),
        )
        return model

    def embed(self, texts: list[str]) -> np.ndarray:
        if not texts:
            return np.array([])

        self.ensure_initialized()

        # FastEmbed returns a generator of (already L2-normalized) vectors.
        embeddings_list = list(
            self._model.embed(texts, batch_size=self.config.batch_size)
        )
        embeddings = np.array(embeddings_list, dtype=np.float32)

        if self.config.normalize and embeddings.size:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1.0
            embeddings = embeddings / norms

        return embeddings

    def get_dimension(self) -> int:
        return _MODEL_DIMENSIONS.get(
            self.config.model.name, self.config.model.dimension
        )
