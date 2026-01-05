"""
Simulated Embedding Provider (Optimized v2)

Fast deterministic embeddings for testing without requiring model downloads.
Uses hash-based generation for consistency.
"""

from typing import List, Optional
import numpy as np
import hashlib

from ...core.base import BaseEmbeddingProvider
from ...core.config import ProviderConfig, ModelMetadata
from ...core.registry import ProviderRegistry


# Model metadata
SIMULATED_MODELS = {
    "simulated-embeddings": ModelMetadata(
        name="simulated-embeddings",
        dimension=384,
        max_length=512,
        provider_type="simulated",
        description="Fast hash-based embeddings for testing",
        use_case="Testing, development, CI/CD pipelines",
    )
}


@ProviderRegistry.register(
    name="simulated",
    models=SIMULATED_MODELS,
    aliases=["test", "mock"],
    description="Fast deterministic embeddings for testing (no model download required)",
)
class SimulatedEmbeddingProvider(BaseEmbeddingProvider):
    """
    Simulated embedding provider for testing

    Generates deterministic embeddings based on text hashing.
    No model download required - perfect for testing and CI/CD.

    **Key features:**
    - Deterministic: Same text always produces same embedding
    - Fast: No model loading or GPU required
    - Configurable: Adjust dimension, seed, normalization
    - Zero dependencies: Pure NumPy implementation

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers.core import ProviderRegistry

    # Get provider
    SimulatedProvider = ProviderRegistry.get_provider("simulated")

    # Create with defaults (384 dims)
    provider = SimulatedProvider()

    # Custom dimension
    from proximadb_sdk.embedding_providers.core import ProviderConfig, ModelMetadata

    config = ProviderConfig(
        model=ModelMetadata(name="simulated-embeddings", dimension=768),
        extra={"seed": 12345, "method": "hash"}
    )
    provider = SimulatedProvider(config)

    # Generate embeddings
    embeddings = provider.embed(["text1", "text2", "text3"])
    ```
    """

    def default_config(self) -> ProviderConfig:
        """Return default configuration"""
        return ProviderConfig(
            model=SIMULATED_MODELS["simulated-embeddings"],
            batch_size=1000,  # Fast, so large batches OK
            normalize=True,
            extra={
                "seed": 42,
                "method": "hash",  # Options: "hash", "random", "gaussian"
            },
        )

    def _load_model(self):
        """No model to load for simulated provider"""
        return None

    def embed(self, texts: List[str]) -> np.ndarray:
        """
        Generate simulated embeddings

        Args:
            texts: List of text strings

        Returns:
            NumPy array of shape (len(texts), dimension)
        """
        if not texts:
            return np.array([])

        self.ensure_initialized()

        dimension = self.config.model.dimension
        seed = self.config.extra.get("seed", 42)
        method = self.config.extra.get("method", "hash")

        embeddings = []
        for text in texts:
            if method == "hash":
                emb = self._hash_based_embedding(text, dimension, seed)
            elif method == "random":
                emb = self._random_embedding(text, dimension, seed)
            elif method == "gaussian":
                emb = self._gaussian_embedding(text, dimension, seed)
            else:
                raise ValueError(f"Unknown method: {method}")

            embeddings.append(emb)

        embeddings = np.array(embeddings, dtype=np.float32)

        if self.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1.0
            embeddings = embeddings / norms

        return embeddings

    def _hash_based_embedding(self, text: str, dimension: int, seed: int) -> np.ndarray:
        """Generate embedding using hash function"""
        # Create deterministic hash from text + seed
        hash_input = f"{text}_{seed}".encode("utf-8")
        hash_obj = hashlib.sha256(hash_input)

        # Generate dimension values from hash
        embedding = []
        hash_bytes = hash_obj.digest()

        for i in range(dimension):
            # Use different parts of hash + rehash as needed
            if i * 4 >= len(hash_bytes):
                hash_input = f"{text}_{seed}_{i}".encode("utf-8")
                hash_obj = hashlib.sha256(hash_input)
                hash_bytes = hash_obj.digest()

            # Convert bytes to float in [-1, 1]
            byte_idx = (i * 4) % len(hash_bytes)
            val = int.from_bytes(
                hash_bytes[byte_idx : byte_idx + 4], "big", signed=False
            )
            normalized_val = (val / (2**32 - 1)) * 2 - 1
            embedding.append(normalized_val)

        return np.array(embedding, dtype=np.float32)

    def _random_embedding(self, text: str, dimension: int, seed: int) -> np.ndarray:
        """Generate embedding using random values (deterministic via seed)"""
        # Hash text to get deterministic seed
        hash_obj = hashlib.sha256(f"{text}_{seed}".encode("utf-8"))
        text_seed = int.from_bytes(hash_obj.digest()[:4], "big")

        # Generate random embedding
        rng = np.random.RandomState(text_seed)
        return rng.randn(dimension).astype(np.float32)

    def _gaussian_embedding(self, text: str, dimension: int, seed: int) -> np.ndarray:
        """Generate embedding from Gaussian distribution"""
        # Similar to random but with specific distribution
        hash_obj = hashlib.sha256(f"{text}_{seed}".encode("utf-8"))
        text_seed = int.from_bytes(hash_obj.digest()[:4], "big")

        rng = np.random.RandomState(text_seed)
        return rng.normal(loc=0.0, scale=0.5, size=dimension).astype(np.float32)


# Backward compatibility alias
@ProviderRegistry.register(
    name="simulated-v2",
    models=SIMULATED_MODELS,
    description="Simulated embeddings (v2 architecture)",
)
class SimulatedEmbeddingProviderV2(SimulatedEmbeddingProvider):
    """Alias for versioning"""

    pass
