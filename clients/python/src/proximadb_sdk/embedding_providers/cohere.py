"""
Cohere embedding provider

Uses Cohere's embedding API via the modern ``cohere.ClientV2``.

WARNING: Requires an API key and incurs costs per token.
"""

import logging
import os
import warnings
from typing import Any

import numpy as np

from .core.base import BaseEmbeddingProvider
from .core.config import ModelMetadata, ProviderConfig
from .core.registry import ProviderRegistry

logger = logging.getLogger(__name__)

COHERE_MODELS = {
    "embed-english-light-v3.0": ModelMetadata(
        name="embed-english-light-v3.0",
        dimension=384,
        max_length=512,
        provider_type="api",
        languages="en",
        description="Lightweight English model, very cost-effective",
        use_case="Cost-effective English retrieval",
    ),
    "embed-english-v3.0": ModelMetadata(
        name="embed-english-v3.0",
        dimension=1024,
        max_length=512,
        provider_type="api",
        languages="en",
        description="High-quality English embeddings",
        use_case="High-accuracy English retrieval",
    ),
    "embed-multilingual-v3.0": ModelMetadata(
        name="embed-multilingual-v3.0",
        dimension=1024,
        max_length=512,
        provider_type="api",
        languages="100+",
        description="Supports 100+ languages",
        use_case="Multilingual retrieval",
    ),
}

# Cohere maps embedding intent to an `input_type` argument (v3 models).
_INPUT_TYPES = (
    "search_document",
    "search_query",
    "classification",
    "clustering",
)

# Pricing (USD per 1M tokens) for rough cost estimates.
_COST_PER_1M = {
    "embed-english-light-v3.0": 0.02,
    "embed-english-v3.0": 0.10,
    "embed-multilingual-v3.0": 0.10,
}


@ProviderRegistry.register(
    name="cohere",
    models=COHERE_MODELS,
    aliases=["cohere-embeddings"],
    description="Cohere hosted embeddings (requires API key, incurs cost)",
)
class CohereProvider(BaseEmbeddingProvider):
    """
    Embedding provider using Cohere's API (``cohere.ClientV2``).

    WARNING: This provider requires a Cohere API key and will incur costs.

    Set the API key via:
    - Environment variable ``COHERE_API_KEY``
    - ``extra={"api_key": "..."}`` on the :class:`ProviderConfig`

    Optional ``extra`` parameters:
    - ``api_key``: API key (falls back to ``COHERE_API_KEY``)
    - ``input_type``: one of ``search_document`` (default), ``search_query``,
      ``classification``, ``clustering``
    - ``truncate``: ``NONE`` / ``START`` / ``END`` (default ``END``)
    - ``show_cost_warnings``: emit a UserWarning about per-token cost (default True)
    """

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=COHERE_MODELS["embed-english-light-v3.0"],
            batch_size=96,  # Cohere max batch size
            normalize=True,
            extra={
                "api_key": None,
                "input_type": "search_document",
                "truncate": "END",
                "show_cost_warnings": True,
            },
        )

    def _load_model(self) -> Any:
        try:
            import cohere
        except ImportError as exc:  # pragma: no cover - exercised via stub
            raise ImportError(
                "cohere is required for CohereProvider. "
                "Install with: pip install 'cohere>=5'"
            ) from exc

        extra = self.config.extra
        api_key = extra.get("api_key") or os.getenv("COHERE_API_KEY")
        if not api_key:
            raise RuntimeError(
                "Cohere API key not found. Set COHERE_API_KEY or pass "
                "extra={'api_key': ...} in the provider config."
            )

        if extra.get("show_cost_warnings", True):
            warnings.warn(
                f"Cohere embeddings incur cost. Model '{self.config.model.name}' "
                "charges per token. Consider a local provider for development.",
                UserWarning,
                stacklevel=2,
            )

        self._token_count = 0
        client = cohere.ClientV2(api_key=api_key)
        logger.info(
            "Initialized Cohere provider with model %s (dimension %s)",
            self.config.model.name,
            self.config.model.dimension,
        )
        return client

    def embed(self, texts: list[str], input_type: str | None = None) -> np.ndarray:
        if not texts:
            return np.array([])

        self.ensure_initialized()

        resolved_type = input_type or self.config.extra.get(
            "input_type", "search_document"
        )
        if resolved_type not in _INPUT_TYPES:
            raise ValueError(
                f"Invalid input_type '{resolved_type}'. Expected one of {_INPUT_TYPES}."
            )

        all_embeddings: list[list[float]] = []
        for i in range(0, len(texts), self.config.batch_size):
            batch = texts[i : i + self.config.batch_size]
            try:
                response = self._model.embed(
                    texts=batch,
                    model=self.config.model.name,
                    input_type=resolved_type,
                    embedding_types=["float"],
                    truncate=self.config.extra.get("truncate", "END"),
                )
            except Exception as exc:
                logger.error("Cohere API error: %s", exc)
                raise RuntimeError(f"Failed to generate embeddings: {exc}") from exc

            # ClientV2 returns embeddings keyed by type: response.embeddings.float
            batch_embeddings = response.embeddings.float
            all_embeddings.extend(batch_embeddings)

            meta = getattr(response, "meta", None)
            billed = getattr(meta, "billed_units", None) if meta else None
            tokens = getattr(billed, "input_tokens", None) if billed else None
            if tokens is not None:
                self._token_count = getattr(self, "_token_count", 0) + tokens

        embeddings = np.array(all_embeddings, dtype=np.float32)

        if self.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1.0
            embeddings = embeddings / norms

        return embeddings

    def embed_query(self, query: str) -> np.ndarray:
        """Embed a single query with the ``search_query`` input type."""
        return self.embed([query], input_type="search_query")[0]

    def embed_passages(self, passages: list[str]) -> np.ndarray:
        """Embed documents with the ``search_document`` input type."""
        return self.embed(passages, input_type="search_document")

    def _estimate_cost(self, tokens: int) -> float:
        rate = _COST_PER_1M.get(self.config.model.name, 0.10)
        return (tokens / 1_000_000) * rate

    def get_token_usage(self) -> dict[str, Any]:
        tokens = getattr(self, "_token_count", 0)
        return {
            "estimated_tokens": tokens,
            "estimated_cost": self._estimate_cost(tokens),
            "model": self.config.model.name,
        }
