"""
OpenAI-compatible embedding provider

Supports any OpenAI-compatible API including local models
served by vLLM, Ollama, LocalAI, etc.
"""

import logging
import os
from urllib.parse import urljoin

import numpy as np
import requests

from .base import EmbeddingConfig, EmbeddingProvider

logger = logging.getLogger(__name__)


class OpenAICompatibleProvider(EmbeddingProvider):
    """
    Embedding provider for OpenAI-compatible APIs

    This provider works with:
    - Local models served by vLLM, Ollama, LocalAI
    - OpenAI API (requires API key)
    - Any OpenAI-compatible endpoint

    For truly free usage, use with local models like:
    - Ollama with nomic-embed-text, all-minilm, etc.
    - vLLM with any HuggingFace embedding model
    - LocalAI with BERT, sentence-transformers models
    """

    # Known model dimensions for common models
    MODEL_DIMENSIONS = {
        # OpenAI models (require API key)
        "text-embedding-ada-002": 1536,
        "text-embedding-3-small": 1536,
        "text-embedding-3-large": 3072,
        # Common Ollama models (free, local)
        "nomic-embed-text": 768,
        "all-minilm": 384,
        "mxbai-embed-large": 1024,
        # Common vLLM/LocalAI models (free, local)
        "BAAI/bge-base-en-v1.5": 768,
        "BAAI/bge-small-en-v1.5": 384,
        "sentence-transformers/all-MiniLM-L6-v2": 384,
    }

    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration"""
        return EmbeddingConfig(
            model_name="nomic-embed-text",  # Free Ollama model
            dimension=768,
            batch_size=100,
            normalize=True,
            cache_embeddings=True,
            device=None,
            extra_params={
                "api_base": "http://localhost:11434/v1",  # Ollama default
                "api_key": None,  # Not needed for local models
                "timeout": 30.0,
                "max_retries": 3,
            },
        )

    def _initialize(self) -> None:
        """Initialize the OpenAI-compatible client"""
        try:
            # Get API configuration
            self.api_base = self.config.extra_params.get("api_base") or os.getenv(
                "OPENAI_API_BASE", "http://localhost:11434/v1"
            )
            self.api_key = self.config.extra_params.get("api_key") or os.getenv(
                "OPENAI_API_KEY", ""
            )

            # Update dimension if known
            if self.config.model_name in self.MODEL_DIMENSIONS:
                self.config.dimension = self.MODEL_DIMENSIONS[self.config.model_name]

            # Test connection with a simple embedding
            self._test_connection()

            self._available = True
            logger.info(
                f"Initialized OpenAI-compatible provider with model: {self.config.model_name} "
                f"at {self.api_base}"
            )

        except Exception as e:
            self._available = False
            logger.error(f"Failed to initialize OpenAI-compatible provider: {e}")

    def _test_connection(self):
        """Test the connection with a simple embedding request"""
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        data = {
            "model": self.config.model_name,
            "input": ["test"],
        }

        response = requests.post(
            urljoin(self.api_base, "/embeddings"),
            json=data,
            headers=headers,
            timeout=self.config.extra_params.get("timeout", 30.0),
        )

        if response.status_code != 200:
            raise RuntimeError(
                f"API test failed: {response.status_code} - {response.text}"
            )

        # Update dimension from response
        result = response.json()
        if "data" in result and len(result["data"]) > 0:
            embedding = result["data"][0]["embedding"]
            self.config.dimension = len(embedding)

    def embed_texts(self, texts: list[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        Args:
            texts: List of texts to embed

        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError("OpenAI-compatible provider not available")

        if not texts:
            return np.array([])

        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        all_embeddings = []

        # Process in batches
        for i in range(0, len(texts), self.config.batch_size):
            batch = texts[i : i + self.config.batch_size]

            data = {
                "model": self.config.model_name,
                "input": batch,
            }

            # Add encoding format if normalizing
            if self.config.normalize:
                data["encoding_format"] = "float"

            response = requests.post(
                urljoin(self.api_base, "/embeddings"),
                json=data,
                headers=headers,
                timeout=self.config.extra_params.get("timeout", 30.0),
            )

            if response.status_code != 200:
                raise RuntimeError(
                    f"Embedding request failed: {response.status_code} - {response.text}"
                )

            result = response.json()

            # Extract embeddings
            batch_embeddings = [item["embedding"] for item in result["data"]]
            all_embeddings.extend(batch_embeddings)

        embeddings = np.array(all_embeddings)

        # Normalize if requested and not already done by API
        if self.config.normalize and not data.get("encoding_format"):
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            embeddings = embeddings / norms

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
    def create_ollama_provider(
        cls,
        model_name: str = "nomic-embed-text",
        host: str = "localhost",
        port: int = 11434,
        **kwargs,
    ) -> "OpenAICompatibleProvider":
        """
        Create provider configured for Ollama

        Args:
            model_name: Ollama model name
            host: Ollama host
            port: Ollama port
            **kwargs: Additional config parameters

        Returns:
            Configured provider for Ollama
        """
        config = EmbeddingConfig(
            model_name=model_name,
            dimension=cls.MODEL_DIMENSIONS.get(model_name, 768),
            extra_params={
                "api_base": f"http://{host}:{port}/v1",
                "api_key": None,
                **kwargs,
            },
        )
        return cls(config)

    @classmethod
    def create_vllm_provider(
        cls,
        model_name: str = "BAAI/bge-base-en-v1.5",
        host: str = "localhost",
        port: int = 8000,
        **kwargs,
    ) -> "OpenAICompatibleProvider":
        """
        Create provider configured for vLLM

        Args:
            model_name: HuggingFace model name
            host: vLLM host
            port: vLLM port
            **kwargs: Additional config parameters

        Returns:
            Configured provider for vLLM
        """
        config = EmbeddingConfig(
            model_name=model_name,
            dimension=cls.MODEL_DIMENSIONS.get(model_name, 768),
            extra_params={
                "api_base": f"http://{host}:{port}/v1",
                "api_key": None,
                **kwargs,
            },
        )
        return cls(config)
