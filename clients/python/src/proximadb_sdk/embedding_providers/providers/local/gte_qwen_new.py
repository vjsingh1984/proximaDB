"""
gte-Qwen Provider (Optimized v2)

Alibaba's state-of-the-art multilingual embedding models.
Ranks #1 on MTEB for both English and Chinese.

This is the refactored version using the new architecture:
- 90% less code than original (30 lines vs 336 lines)
- Automatic model caching
- Cleaner configuration
- Better extensibility
"""

from ...core.base import BaseEmbeddingProvider
from ...core.config import ModelMetadata, ProviderConfig
from ...core.registry import ProviderRegistry
from ...mixins.instruction import InstructionMixin
from ...mixins.sentence_transformer import SentenceTransformerMixin

# Model metadata catalog
GTE_QWEN_MODELS = {
    "Alibaba-NLP/gte-Qwen2-7B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen2-7B-instruct",
        dimension=3584,
        max_length=32768,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=71.0,
        languages="100+",
        description="Top MTEB performer, #1 English & Chinese, 7B parameters",
        use_case="Maximum accuracy, multilingual, enterprise applications",
    ),
    "Alibaba-NLP/gte-Qwen2-1.5B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen2-1.5B-instruct",
        dimension=1536,
        max_length=32768,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=68.0,
        languages="100+",
        description="Smaller variant, excellent quality, 1.5B parameters",
        use_case="Balanced accuracy and speed, multilingual",
    ),
    "Alibaba-NLP/gte-Qwen1.5-7B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen1.5-7B-instruct",
        dimension=3584,
        max_length=8192,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=70.0,
        languages="100+",
        description="Earlier 7B model, still excellent performance",
        use_case="Alternative to Qwen2-7B",
    ),
}


@ProviderRegistry.register(
    name="gte-qwen-v2",
    models=GTE_QWEN_MODELS,
    aliases=["alibaba-v2", "qwen-v2", "gte-v2"],
    description="Alibaba's state-of-the-art multilingual embeddings (#1 MTEB)",
)
class GTEQwenProvider(
    InstructionMixin,  # Provides embed_query(), embed_passages()
    SentenceTransformerMixin,  # Provides _load_model(), embed()
    BaseEmbeddingProvider,  # Provides lifecycle, config
):
    """
    Optimized gte-Qwen Provider (v2)

    **Key improvements over v1:**
    - 90% code reduction (30 lines vs 336 lines)
    - Automatic model caching via ModelCache
    - Cleaner configuration via ProviderConfig
    - Instruction handling via InstructionMixin
    - Better testing via composition

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers.core import ProviderRegistry

    # Get provider class
    GTEQwen = ProviderRegistry.get_provider("gte-qwen-v2")

    # Create with defaults
    provider = GTEQwen()

    # Query embeddings (with automatic instruction)
    query_emb = provider.embed_query("What is machine learning?")

    # Passage embeddings (no instruction)
    passage_embs = provider.embed_passages([
        "ML is a subset of AI",
        "AI enables machines to learn"
    ])

    # Batch embeddings
    all_embs = provider.embed(["text1", "text2", "text3"])
    ```

    **Advanced usage:**

    ```python
    # Use larger model
    config = ProviderConfig(
        model=GTE_QWEN_MODELS["Alibaba-NLP/gte-Qwen2-7B-instruct"],
        batch_size=16,
        device="cuda"
    )
    provider = GTEQwen(config)

    # Custom instruction
    config = config.merge(
        extra={"custom_instruction": "Find relevant documents for: {query}"}
    )
    ```
    """

    def default_config(self) -> ProviderConfig:
        """
        Return default configuration

        Uses the 1.5B model for best balance of speed and accuracy.
        """
        return ProviderConfig(
            model=GTE_QWEN_MODELS["Alibaba-NLP/gte-Qwen2-1.5B-instruct"],
            batch_size=16,  # Smaller batch for large model
            normalize=True,
            trust_remote_code=False,  # Use standard implementation for compatibility
            extra={"use_query_instruction": True},
        )


# Convenience: Register v2 as the default "gte-qwen" provider
# (Once migration complete, remove this and v1 implementation)
@ProviderRegistry.register(
    name="gte-qwen",
    models=GTE_QWEN_MODELS,
    aliases=["alibaba", "qwen", "gte"],
    description="Alibaba's state-of-the-art multilingual embeddings (#1 MTEB)",
)
class GTEQwenProviderV2(GTEQwenProvider):
    """V2 alias of GTEQwenProvider (kept for backward compatibility).

    Was previously named GTEQwenProvider, which self-shadowed the class above
    and left the GTEQwenProviderV2 name (imported by __init__new) undefined.
    """

    pass
