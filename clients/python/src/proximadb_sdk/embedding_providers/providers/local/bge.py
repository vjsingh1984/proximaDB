"""
BGE (BAAI General Embedding) Provider

BAAI's state-of-the-art retrieval-optimized embeddings.
Excellent performance on semantic search tasks.
"""

from ...core.base import BaseEmbeddingProvider
from ...core.config import ModelMetadata, ProviderConfig
from ...core.registry import ProviderRegistry
from ...mixins.instruction import InstructionMixin
from ...mixins.sentence_transformer import SentenceTransformerMixin

BGE_MODELS = {
    "BAAI/bge-large-en-v1.5": ModelMetadata(
        name="BAAI/bge-large-en-v1.5",
        dimension=1024,
        max_length=512,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Represent this sentence for searching relevant passages: {query}",
        mteb_score=64.2,
        languages="en",
        description="Best quality English embeddings, top MTEB performer",
        use_case="Maximum accuracy, research, when quality > speed",
    ),
    "BAAI/bge-base-en-v1.5": ModelMetadata(
        name="BAAI/bge-base-en-v1.5",
        dimension=768,
        max_length=512,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Represent this sentence for searching relevant passages: {query}",
        mteb_score=63.5,
        languages="en",
        description="Balanced quality and speed",
        use_case="Production systems requiring good performance",
    ),
    "BAAI/bge-small-en-v1.5": ModelMetadata(
        name="BAAI/bge-small-en-v1.5",
        dimension=384,
        max_length=512,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Represent this sentence for searching relevant passages: {query}",
        mteb_score=62.2,
        languages="en",
        description="Fastest variant, still excellent quality",
        use_case="High-throughput applications, edge devices",
    ),
    "BAAI/bge-m3": ModelMetadata(
        name="BAAI/bge-m3",
        dimension=1024,
        max_length=8192,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Represent this sentence for searching relevant passages: {query}",
        mteb_score=66.0,
        languages="100+",
        description="Multilingual model with long context support",
        use_case="Multilingual applications, long documents",
    ),
}


@ProviderRegistry.register(
    name="bge",
    models=BGE_MODELS,
    aliases=["baai"],
    description="BAAI state-of-the-art retrieval-optimized embeddings",
)
class BGEProvider(InstructionMixin, SentenceTransformerMixin, BaseEmbeddingProvider):
    """
    BGE embedding provider

    **Key features:**
    - Optimized for semantic search and retrieval
    - Query instruction support for better accuracy
    - Multiple size options (small/base/large)
    - Multilingual variant available (bge-m3)

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers import get_provider

    # Default (large model)
    provider = get_provider("bge")

    # Small model for speed
    from proximadb_sdk.embedding_providers.core import ProviderRegistry, ProviderConfig
    BGE = ProviderRegistry.get_provider("bge")
    config = ProviderConfig(model=BGE_MODELS["BAAI/bge-small-en-v1.5"])
    provider = BGE(config)

    # Query embeddings (with automatic instruction)
    query_emb = provider.embed_query("machine learning tutorial")

    # Document embeddings (no instruction)
    doc_embs = provider.embed_passages(["ML is great", "AI is the future"])
    ```
    """

    def default_config(self) -> ProviderConfig:
        """Default to large model for best accuracy"""
        return ProviderConfig(
            model=BGE_MODELS["BAAI/bge-large-en-v1.5"],
            batch_size=32,
            normalize=True,
            extra={"use_query_instruction": True},
        )
