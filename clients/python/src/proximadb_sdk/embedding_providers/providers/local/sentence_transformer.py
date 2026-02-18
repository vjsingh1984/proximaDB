"""
Generic SentenceTransformer Provider

Wrapper for any sentence-transformers model.
Use this for models not covered by specialized providers.
"""

from ...core.base import BaseEmbeddingProvider
from ...core.config import ModelMetadata, ProviderConfig
from ...core.registry import ProviderRegistry
from ...mixins.sentence_transformer import SentenceTransformerMixin

# Popular models
SENTENCE_TRANSFORMER_MODELS = {
    "all-mpnet-base-v2": ModelMetadata(
        name="all-mpnet-base-v2",
        dimension=768,
        max_length=512,
        provider_type="sentence-transformer",
        mteb_score=63.3,
        languages="en",
        description="Excellent general-purpose model",
        use_case="General semantic similarity and search",
    ),
    "all-MiniLM-L6-v2": ModelMetadata(
        name="all-MiniLM-L6-v2",
        dimension=384,
        max_length=512,
        provider_type="sentence-transformer",
        mteb_score=58.8,
        languages="en",
        description="Fast and compact",
        use_case="High-throughput, resource-constrained environments",
    ),
    "all-MiniLM-L12-v2": ModelMetadata(
        name="all-MiniLM-L12-v2",
        dimension=384,
        max_length=512,
        provider_type="sentence-transformer",
        mteb_score=59.8,
        languages="en",
        description="Better than L6, still fast",
        use_case="Balance of speed and quality",
    ),
    "paraphrase-MiniLM-L6-v2": ModelMetadata(
        name="paraphrase-MiniLM-L6-v2",
        dimension=384,
        max_length=512,
        provider_type="sentence-transformer",
        languages="en",
        description="Optimized for paraphrase detection",
        use_case="Paraphrase identification, duplicate detection",
    ),
}


@ProviderRegistry.register(
    name="sentence-transformer",
    models=SENTENCE_TRANSFORMER_MODELS,
    aliases=["st", "sbert"],
    description="Generic wrapper for any sentence-transformers model",
)
class SentenceTransformerProvider(SentenceTransformerMixin, BaseEmbeddingProvider):
    """
    Generic SentenceTransformer provider

    **Use this provider for:**
    - Any sentence-transformers model from HuggingFace
    - Models not covered by specialized providers (BGE, E5, SFR, etc.)
    - Custom fine-tuned models

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers import get_provider
    from proximadb_sdk.embedding_providers.core import ProviderConfig, ModelMetadata

    # Use a pre-configured model
    provider = get_provider("sentence-transformer")  # Defaults to all-mpnet-base-v2

    # Use any HuggingFace model
    from proximadb_sdk.embedding_providers.core import ProviderRegistry
    ST = ProviderRegistry.get_provider("sentence-transformer")

    config = ProviderConfig(
        model=ModelMetadata(
            name="sentence-transformers/all-roberta-large-v1",
            dimension=1024,
            max_length=512
        ),
        batch_size=32
    )
    provider = ST(config)

    # Generate embeddings
    embeddings = provider.embed(["text1", "text2", "text3"])
    ```
    """

    def default_config(self) -> ProviderConfig:
        """Default to all-mpnet-base-v2 (best general-purpose model)"""
        return ProviderConfig(
            model=SENTENCE_TRANSFORMER_MODELS["all-mpnet-base-v2"],
            batch_size=32,
            normalize=True,
        )
