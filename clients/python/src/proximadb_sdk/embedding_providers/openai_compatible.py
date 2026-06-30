"""
OpenAI-compatible embedding provider

Supports any OpenAI-compatible ``/embeddings`` endpoint, including local models
served by vLLM, Ollama, LocalAI, etc.
"""

import logging
import os
from typing import Any

import numpy as np
import requests

from .core.base import BaseEmbeddingProvider
from .core.config import ModelMetadata, ProviderConfig
from .core.registry import ProviderRegistry

logger = logging.getLogger(__name__)

OPENAI_COMPATIBLE_MODELS = {
    "nomic-embed-text": ModelMetadata(
        name="nomic-embed-text",
        dimension=768,
        max_length=8192,
        provider_type="api",
        languages="en",
        description="Free local Ollama embedding model",
        use_case="Local OpenAI-compatible inference (Ollama)",
    ),
    "all-minilm": ModelMetadata(
        name="all-minilm",
        dimension=384,
        max_length=512,
        provider_type="api",
        languages="en",
        description="Lightweight local model",
        use_case="Local OpenAI-compatible inference",
    ),
    "mxbai-embed-large": ModelMetadata(
        name="mxbai-embed-large",
        dimension=1024,
        max_length=512,
        provider_type="api",
        languages="en",
        description="Higher-quality local model",
        use_case="Local OpenAI-compatible inference",
    ),
}

_MODEL_DIMENSIONS = {
    "text-embedding-ada-002": 1536,
    "text-embedding-3-small": 1536,
    "text-embedding-3-large": 3072,
    "nomic-embed-text": 768,
    "all-minilm": 384,
    "mxbai-embed-large": 1024,
    "BAAI/bge-base-en-v1.5": 768,
    "BAAI/bge-small-en-v1.5": 384,
    "sentence-transformers/all-MiniLM-L6-v2": 384,
}


def _embeddings_url(api_base: str) -> str:
    """Join the ``/embeddings`` path onto an API base WITHOUT dropping a path
    segment such as ``/v1``.

    ``urljoin("http://x/v1", "/embeddings")`` returns ``http://x/embeddings`` —
    the leading slash makes it absolute and discards ``/v1``. We instead append
    to the (slash-normalized) base.
    """
    return api_base.rstrip("/") + "/embeddings"


@ProviderRegistry.register(
    name="openai-compatible",
    models=OPENAI_COMPATIBLE_MODELS,
    aliases=["ollama", "vllm", "localai"],
    description="Any OpenAI-compatible /embeddings endpoint (Ollama, vLLM, LocalAI)",
)
class OpenAICompatibleProvider(BaseEmbeddingProvider):
    """
    Embedding provider for OpenAI-compatible REST endpoints.

    Works with local models served by vLLM, Ollama, LocalAI, or any
    OpenAI-compatible ``/embeddings`` endpoint.

    Optional ``extra`` parameters:
    - ``api_base``: base URL (default ``http://localhost:11434/v1`` for Ollama;
      also honours the ``OPENAI_API_BASE`` env var)
    - ``api_key``: bearer token (falls back to ``OPENAI_API_KEY``; not needed
      for local models)
    - ``timeout``: per-request timeout in seconds (default 30.0)
    """

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=OPENAI_COMPATIBLE_MODELS["nomic-embed-text"],
            batch_size=100,
            normalize=True,
            extra={
                "api_base": "http://localhost:11434/v1",  # Ollama default
                "api_key": None,
                "timeout": 30.0,
            },
        )

    def _load_model(self) -> Any:
        """Resolve connection settings; there is no in-process model to load."""
        extra = self.config.extra
        self.api_base = extra.get("api_base") or os.getenv(
            "OPENAI_API_BASE", "http://localhost:11434/v1"
        )
        self.api_key = extra.get("api_key") or os.getenv("OPENAI_API_KEY", "")
        logger.info(
            "Initialized OpenAI-compatible provider with model %s at %s",
            self.config.model.name,
            self.api_base,
        )
        return self.api_base

    def _headers(self) -> dict[str, str]:
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers

    def embed(self, texts: list[str]) -> np.ndarray:
        if not texts:
            return np.array([])

        self.ensure_initialized()

        url = _embeddings_url(self.api_base)
        timeout = self.config.extra.get("timeout", 30.0)
        all_embeddings: list[list[float]] = []

        for i in range(0, len(texts), self.config.batch_size):
            batch = texts[i : i + self.config.batch_size]
            data = {"model": self.config.model.name, "input": batch}

            response = requests.post(
                url, json=data, headers=self._headers(), timeout=timeout
            )
            if response.status_code != 200:
                raise RuntimeError(
                    f"Embedding request failed: {response.status_code} - "
                    f"{response.text}"
                )

            result = response.json()
            ordered = sorted(result["data"], key=lambda item: item.get("index", 0))
            all_embeddings.extend(item["embedding"] for item in ordered)

        embeddings = np.array(all_embeddings, dtype=np.float32)

        if self.config.normalize and embeddings.size:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1.0
            embeddings = embeddings / norms

        return embeddings

    def get_dimension(self) -> int:
        return _MODEL_DIMENSIONS.get(
            self.config.model.name, self.config.model.dimension
        )

    @classmethod
    def create_ollama_provider(
        cls,
        model_name: str = "nomic-embed-text",
        host: str = "localhost",
        port: int = 11434,
        **extra: Any,
    ) -> "OpenAICompatibleProvider":
        """Create a provider configured for an Ollama endpoint."""
        model = OPENAI_COMPATIBLE_MODELS.get(model_name) or ModelMetadata(
            name=model_name,
            dimension=_MODEL_DIMENSIONS.get(model_name, 768),
            provider_type="api",
        )
        config = ProviderConfig(
            model=model,
            extra={
                "api_base": f"http://{host}:{port}/v1",
                "api_key": None,
                **extra,
            },
        )
        return cls(config)

    @classmethod
    def create_vllm_provider(
        cls,
        model_name: str = "BAAI/bge-base-en-v1.5",
        host: str = "localhost",
        port: int = 8000,
        **extra: Any,
    ) -> "OpenAICompatibleProvider":
        """Create a provider configured for a vLLM endpoint."""
        model = OPENAI_COMPATIBLE_MODELS.get(model_name) or ModelMetadata(
            name=model_name,
            dimension=_MODEL_DIMENSIONS.get(model_name, 768),
            provider_type="api",
        )
        config = ProviderConfig(
            model=model,
            extra={
                "api_base": f"http://{host}:{port}/v1",
                "api_key": None,
                **extra,
            },
        )
        return cls(config)
