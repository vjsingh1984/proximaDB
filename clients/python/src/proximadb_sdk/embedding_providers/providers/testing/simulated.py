"""
Simulated Embedding Provider

Fast deterministic embeddings for testing without model downloads.
"""

from typing import List
import numpy as np
import hashlib

from ...core.base import BaseEmbeddingProvider
from ...core.config import ProviderConfig, ModelMetadata
from ...core.registry import ProviderRegistry


SIMULATED_MODELS = {
    "simulated-embeddings": ModelMetadata(
        name="simulated-embeddings",
        dimension=384,
        max_length=512,
        provider_type="simulated",
        description="Fast hash-based embeddings for testing",
        use_case="Testing, development, CI/CD pipelines"
    )
}


@ProviderRegistry.register(
    name="simulated",
    models=SIMULATED_MODELS,
    aliases=["test", "mock"],
    description="Fast deterministic embeddings for testing (no model download)"
)
class SimulatedEmbeddingProvider(BaseEmbeddingProvider):
    """
    Simulated embedding provider for testing

    Generates deterministic embeddings using text hashing.
    Perfect for testing - no model download required.

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers import get_provider

    # Default configuration
    provider = get_provider("simulated")

    # Custom dimension
    provider = get_provider("simulated", dimension=768)

    # Generate embeddings
    embeddings = provider.embed(["text1", "text2"])
    ```
    """

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=SIMULATED_MODELS["simulated-embeddings"],
            batch_size=1000,
            normalize=True,
            extra={"seed": 42, "method": "hash"}
        )

    def _load_model(self):
        """No model to load - return True to indicate provider is ready"""
        return True

    def embed(self, texts: List[str]) -> np.ndarray:
        """Generate simulated embeddings"""
        if not texts:
            return np.array([])

        self.ensure_initialized()

        dimension = self.config.model.dimension
        seed = self.config.extra.get("seed", 42)

        embeddings = []
        for text in texts:
            emb = self._hash_embedding(text, dimension, seed)
            embeddings.append(emb)

        embeddings = np.array(embeddings, dtype=np.float32)

        if self.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1.0
            embeddings = embeddings / norms

        return embeddings

    def _hash_embedding(self, text: str, dimension: int, seed: int) -> np.ndarray:
        """Generate embedding using deterministic hash"""
        hash_input = f"{text}_{seed}".encode('utf-8')
        hash_obj = hashlib.sha256(hash_input)

        embedding = []
        hash_bytes = hash_obj.digest()

        for i in range(dimension):
            if i * 4 >= len(hash_bytes):
                hash_input = f"{text}_{seed}_{i}".encode('utf-8')
                hash_obj = hashlib.sha256(hash_input)
                hash_bytes = hash_obj.digest()

            byte_idx = (i * 4) % len(hash_bytes)
            val = int.from_bytes(hash_bytes[byte_idx:byte_idx+4], 'big', signed=False)
            normalized_val = (val / (2**32 - 1)) * 2 - 1
            embedding.append(normalized_val)

        return np.array(embedding, dtype=np.float32)
