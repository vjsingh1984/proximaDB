"""
OpenAI embedding provider

Uses OpenAI's embedding API via the modern (>=1.0) ``openai`` client.

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

OPENAI_MODELS = {
    "text-embedding-3-small": ModelMetadata(
        name="text-embedding-3-small",
        dimension=1536,
        max_length=8191,
        provider_type="api",
        mteb_score=62.3,
        languages="multilingual",
        description="Newest small model, very cost-effective (Matryoshka dims)",
        use_case="Cost-effective production embeddings",
    ),
    "text-embedding-3-large": ModelMetadata(
        name="text-embedding-3-large",
        dimension=3072,
        max_length=8191,
        provider_type="api",
        mteb_score=64.6,
        languages="multilingual",
        description="Highest quality OpenAI model (Matryoshka dims)",
        use_case="Maximum accuracy",
    ),
    "text-embedding-ada-002": ModelMetadata(
        name="text-embedding-ada-002",
        dimension=1536,
        max_length=8191,
        provider_type="api",
        languages="multilingual",
        description="Legacy model, being phased out",
        use_case="Legacy compatibility",
    ),
}

# text-embedding-3-* support the Matryoshka `dimensions` parameter.
_MATRYOSHKA_MODELS = {"text-embedding-3-small", "text-embedding-3-large"}

# Rough pricing (USD per 1K tokens) for cost estimates.
_COST_PER_1K = {
    "text-embedding-ada-002": 0.0001,
    "text-embedding-3-small": 0.00002,
    "text-embedding-3-large": 0.00013,
}


@ProviderRegistry.register(
    name="openai",
    models=OPENAI_MODELS,
    aliases=["openai-embeddings"],
    description="OpenAI hosted embeddings (requires API key, incurs cost)",
)
class OpenAIProvider(BaseEmbeddingProvider):
    """
    Embedding provider using OpenAI's API (openai>=1.0 client).

    WARNING: This provider requires an OpenAI API key and will incur costs.

    Set the API key via:
    - Environment variable ``OPENAI_API_KEY``
    - ``extra={"api_key": "sk-..."}`` on the :class:`ProviderConfig`

    Optional ``extra`` parameters:
    - ``api_key``: API key (falls back to ``OPENAI_API_KEY``)
    - ``organization``: OpenAI organization id
    - ``base_url``: override the API base URL (also accepts legacy ``api_base``)
    - ``dimensions``: truncated Matryoshka dimension for text-embedding-3-*
    - ``max_retries`` / ``timeout``: client-level retry/timeout knobs
    - ``show_cost_warnings``: emit a UserWarning about per-token cost (default True)
    """

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=OPENAI_MODELS["text-embedding-3-small"],
            batch_size=100,  # OpenAI supports large batches
            normalize=False,  # OpenAI embeddings are already unit-norm
            extra={
                "api_key": None,
                "organization": None,
                "base_url": None,
                "dimensions": None,
                "max_retries": 3,
                "timeout": 60.0,
                "show_cost_warnings": True,
            },
        )

    def _load_model(self) -> Any:
        """Construct and return the OpenAI client (the 'model' for this provider)."""
        try:
            from openai import OpenAI
        except ImportError as exc:  # pragma: no cover - exercised via stub
            raise ImportError(
                "openai is required for OpenAIProvider. "
                "Install with: pip install 'openai>=2'"
            ) from exc

        extra = self.config.extra
        api_key = extra.get("api_key") or os.getenv("OPENAI_API_KEY")
        if not api_key:
            raise RuntimeError(
                "OpenAI API key not found. Set OPENAI_API_KEY or pass "
                "extra={'api_key': ...} in the provider config."
            )

        if extra.get("show_cost_warnings", True):
            warnings.warn(
                f"OpenAI embeddings incur cost. Model '{self.config.model.name}' "
                "charges per token. Consider a local provider "
                "(sentence-transformer, fastembed) for development.",
                UserWarning,
                stacklevel=2,
            )

        # `api_base` is accepted as a legacy alias for `base_url`.
        base_url = extra.get("base_url") or extra.get("api_base")

        client = OpenAI(
            api_key=api_key,
            organization=extra.get("organization"),
            base_url=base_url,
            max_retries=extra.get("max_retries", 3),
            timeout=extra.get("timeout", 60.0),
        )
        self._token_count = 0
        logger.info(
            "Initialized OpenAI provider with model %s (dimension %s)",
            self.config.model.name,
            self.get_dimension(),
        )
        return client

    def embed(self, texts: list[str]) -> np.ndarray:
        if not texts:
            return np.array([])

        self.ensure_initialized()

        model_name = self.config.model.name
        # `dimensions` is a Matryoshka truncation supported by 3-* models only.
        dimensions = self.config.extra.get("dimensions")
        create_kwargs: dict[str, Any] = {
            "model": model_name,
            "encoding_format": "float",
        }
        if dimensions and model_name in _MATRYOSHKA_MODELS:
            create_kwargs["dimensions"] = dimensions
        elif dimensions:
            logger.warning(
                "Model %s does not support the 'dimensions' parameter; ignoring.",
                model_name,
            )

        all_embeddings: list[list[float]] = []
        for i in range(0, len(texts), self.config.batch_size):
            batch = texts[i : i + self.config.batch_size]
            try:
                response = self._model.embeddings.create(input=batch, **create_kwargs)
            except Exception as exc:
                logger.error("OpenAI API error: %s", exc)
                raise RuntimeError(f"Failed to generate embeddings: {exc}") from exc

            # Preserve request order (the API returns an `index` per item).
            ordered = sorted(response.data, key=lambda item: item.index)
            all_embeddings.extend(item.embedding for item in ordered)

            usage = getattr(response, "usage", None)
            if usage is not None and getattr(usage, "total_tokens", None) is not None:
                self._token_count = (
                    getattr(self, "_token_count", 0) + usage.total_tokens
                )
                logger.info(
                    "OpenAI API used %s tokens for %s texts",
                    usage.total_tokens,
                    len(batch),
                )

        embeddings = np.array(all_embeddings, dtype=np.float32)

        if self.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1.0
            embeddings = embeddings / norms

        return embeddings

    def get_dimension(self) -> int:
        """Effective dimension (honours a Matryoshka ``dimensions`` override)."""
        dimensions = self.config.extra.get("dimensions")
        if dimensions and self.config.model.name in _MATRYOSHKA_MODELS:
            return dimensions
        return self.config.model.dimension

    def _estimate_cost(self, tokens: int) -> float:
        rate = _COST_PER_1K.get(self.config.model.name, 0.0001)
        return (tokens / 1000) * rate

    def get_token_usage(self) -> dict[str, Any]:
        tokens = getattr(self, "_token_count", 0)
        return {
            "estimated_tokens": tokens,
            "estimated_cost": self._estimate_cost(tokens),
            "model": self.config.model.name,
        }
