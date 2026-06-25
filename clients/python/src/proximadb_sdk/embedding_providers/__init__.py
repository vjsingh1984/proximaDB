"""
Embedding Providers - Optimized Architecture

Clean, extensible embedding provider system with:
- 90% code reduction via mixins
- Automatic model caching
- Plugin-based provider registration
"""

# Core components (light — registry, config, base classes; no model deps)
from .core import (
    BaseEmbeddingProvider,
    ModelCache,
    ModelMetadata,
    ProviderConfig,
    ProviderRegistry,
)

# TD-126 Phase 3 (embeddings concern): the concrete providers self-register via
# the `@ProviderRegistry.register(...)` decorator at *module import* time. They
# used to be imported EAGERLY here, so `import proximadb_sdk.embedding_providers`
# paid the cost of importing every provider module (and, on first model use,
# sentence-transformers / onnx) even when only the registry API was wanted. The
# provider imports are now deferred: registration fires LAZILY the first time the
# registry is consulted (`get_provider` / `list_providers` / `get_provider_info`)
# or a provider class name is accessed. Public names stay importable
# (`from proximadb_sdk.embedding_providers import GTEQwenProvider`); only the
# import timing moved. Install the runtime deps with `pip install
# 'proximadb[embeddings]'`.
#
# Public provider-class name -> (submodule, attribute).
_PROVIDER_EXPORTS = {
    "BGEProvider": (".providers.local.bge", "BGEProvider"),
    "E5Provider": (".providers.local.e5", "E5Provider"),
    "GTEQwenProvider": (".providers.local.gte_qwen", "GTEQwenProvider"),
    "SentenceTransformerProvider": (
        ".providers.local.sentence_transformer",
        "SentenceTransformerProvider",
    ),
    "SFRProvider": (".providers.local.sfr", "SFRProvider"),
    "SimulatedEmbeddingProvider": (
        ".providers.testing.simulated",
        "SimulatedEmbeddingProvider",
    ),
    # Cloud / OpenAI-compatible providers (registered onto core.BaseEmbeddingProvider).
    "OpenAIProvider": (".openai_provider", "OpenAIProvider"),
    "CohereProvider": (".cohere", "CohereProvider"),
    "FastEmbedProvider": (".fastembed", "FastEmbedProvider"),
    "OpenAICompatibleProvider": (
        ".openai_compatible",
        "OpenAICompatibleProvider",
    ),
    # Domain providers (ported onto core.BaseEmbeddingProvider in TD-126).
    "InstructorProvider": (".instructor", "InstructorProvider"),
    "FinBERTProvider": (".finbert_provider", "FinBERTProvider"),
    "SECBERTProvider": (".finbert_provider", "SECBERTProvider"),
    "MultiBERTProvider": (".multi_bert_provider", "MultiBERTProvider"),
    "AdaptiveBERTProvider": (".multi_bert_provider", "AdaptiveBERTProvider"),
}

_providers_registered = False


def _ensure_providers_registered() -> None:
    """Import the built-in provider modules so they self-register (idempotent)."""
    global _providers_registered
    if _providers_registered:
        return
    import importlib

    for module_name, _attr in _PROVIDER_EXPORTS.values():
        # The import side effect runs each module's @ProviderRegistry.register.
        importlib.import_module(module_name, __name__)
    _providers_registered = True


def __getattr__(name: str):
    """Lazily import a provider class on first attribute access."""
    if name in _PROVIDER_EXPORTS:
        import importlib

        module_name, attr_name = _PROVIDER_EXPORTS[name]
        module = importlib.import_module(module_name, __name__)
        value = getattr(module, attr_name)
        globals()[name] = value
        return value
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def get_provider(name: str, **config_kwargs):
    """
    Get embedding provider by name

    Args:
        name: Provider name (e.g., "gte-qwen", "simulated")
        **config_kwargs: Optional configuration (dimension, batch_size, device, etc.)

    Returns:
        Initialized embedding provider

    Examples:
        >>> provider = get_provider("gte-qwen")
        >>> provider = get_provider("simulated", dimension=768)
    """
    _ensure_providers_registered()
    provider_class = ProviderRegistry.get_provider(name)

    if not config_kwargs:
        return provider_class()

    default_config = provider_class().default_config()

    # Handle dimension specially
    if "dimension" in config_kwargs:
        new_model = ModelMetadata(
            name=default_config.model.name,
            dimension=config_kwargs.pop("dimension"),
            max_length=default_config.model.max_length,
            provider_type=default_config.model.provider_type,
            requires_instruction=default_config.model.requires_instruction,
            instruction_template=default_config.model.instruction_template,
            mteb_score=default_config.model.mteb_score,
            languages=default_config.model.languages,
            description=default_config.model.description,
            use_case=default_config.model.use_case,
        )
        config_kwargs["model"] = new_model

    config = default_config.merge(**config_kwargs)
    return provider_class(config)


def list_providers(include_aliases: bool = False) -> list[str]:
    """List all available providers"""
    _ensure_providers_registered()
    return ProviderRegistry.list_providers(include_aliases=include_aliases)


def get_provider_info(name: str) -> dict:
    """Get detailed provider information"""
    _ensure_providers_registered()
    return ProviderRegistry.get_provider_info(name)


# Backward compatibility
def get_embedding_provider(provider: str, **kwargs):
    """DEPRECATED: Use get_provider() instead"""
    import warnings

    warnings.warn(
        "get_embedding_provider() is deprecated. Use get_provider()",
        DeprecationWarning,
        stacklevel=2,
    )
    return get_provider(provider, **kwargs)


def get_default_embedding_provider():
    """DEPRECATED: Use get_provider('simulated') instead"""
    import warnings

    warnings.warn(
        "get_default_embedding_provider() is deprecated. Use get_provider('simulated')",
        DeprecationWarning,
        stacklevel=2,
    )
    return get_provider("simulated")


def recommend_free_providers() -> None:
    """Print recommendations for free embedding providers"""
    print("\n🆓 Recommended FREE Embedding Providers:\n")

    print("1. SimulatedProvider (Testing, no dependencies)")
    print("   provider = get_provider('simulated', dimension=768)")
    print()

    print("2. Sentence-Transformers (Most popular)")
    print("   pip install sentence-transformers")
    print("   provider = get_provider('sentence-transformer')")
    print()

    print("3. FastEmbed (Fastest, lightweight)")
    print("   pip install fastembed")
    print("   provider = get_provider('fastembed')")
    print()

    print("4. GTE-Qwen (State-of-the-art multilingual)")
    print("   pip install sentence-transformers")
    print("   provider = get_provider('gte-qwen')")
    print()

    print("5. BGE (Best general-purpose)")
    print("   pip install sentence-transformers")
    print("   provider = get_provider('bge')")
    print()

    print("\n💡 For production, consider OpenAI or Cohere (PAID hosted APIs)")
    print("   These are reachable via get_provider() but require an API key:")
    print("   provider = get_provider('openai')   # OPENAI_API_KEY")
    print("   provider = get_provider('cohere')   # COHERE_API_KEY")
    print("   Or point at a local OpenAI-compatible server (free):")
    print("   provider = get_provider('ollama')   # OpenAI-compatible endpoint")


__all__ = [
    # Core
    "BaseEmbeddingProvider",
    "ProviderConfig",
    "ModelMetadata",
    "ProviderRegistry",
    "ModelCache",
    # Main API
    "get_provider",
    "list_providers",
    "get_provider_info",
    "recommend_free_providers",
    # Backward compatibility (deprecated)
    "get_embedding_provider",
    "get_default_embedding_provider",
    # Providers
    "GTEQwenProvider",
    "SimulatedEmbeddingProvider",
    "OpenAIProvider",
    "CohereProvider",
    "FastEmbedProvider",
    "OpenAICompatibleProvider",
    # Domain providers (ported onto core in TD-126)
    "InstructorProvider",
    "FinBERTProvider",
    "SECBERTProvider",
    "MultiBERTProvider",
    "AdaptiveBERTProvider",
]
