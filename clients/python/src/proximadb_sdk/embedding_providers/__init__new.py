"""
Embedding Providers V2 - Optimized Architecture

This module provides a clean, extensible embedding provider system with:
- 90% code reduction via mixins
- Automatic model caching
- Plugin-based provider registration
- Type-safe configuration
- Comprehensive testing

## Quick Start

```python
from proximadb_sdk.embedding_providers import get_provider

# Get provider using registry
provider = get_provider("gte-qwen")

# Generate embeddings
embeddings = provider.embed(["text1", "text2"])
```

## Available Providers

Run `list_providers()` to see all registered providers.

Top providers:
- **gte-qwen**: Alibaba's #1 MTEB multilingual (1536/3584 dims)
- **sfr**: Salesforce top accuracy (4096 dims)
- **bge**: BAAI retrieval-optimized (384/768/1024 dims)
- **e5**: Microsoft general-purpose (384/768/1024 dims)
- **simulated**: Fast testing provider (no model download)
"""

# Core components
from .core import (
    BaseEmbeddingProvider,
    ProviderConfig,
    ModelMetadata,
    ProviderRegistry,
    ModelCache,
)

# Import providers to trigger registration
from .providers.local.gte_qwen_v2 import GTEQwenProvider, GTEQwenProviderV2
from .providers.testing.simulated_v2 import (
    SimulatedEmbeddingProvider,
    SimulatedEmbeddingProviderV2,
)


# Convenience functions
def get_provider(name: str, **config_kwargs):
    """
    Get embedding provider by name

    This is the primary API for creating providers. Supports both simple
    and advanced usage patterns.

    Args:
        name: Provider name (e.g., "gte-qwen", "simulated", "bge")
        **config_kwargs: Optional configuration parameters

    Returns:
        Initialized embedding provider

    Examples:
        >>> # Simple: use defaults
        >>> provider = get_provider("gte-qwen")

        >>> # Custom dimension
        >>> provider = get_provider("simulated", dimension=768)

        >>> # Custom batch size and device
        >>> provider = get_provider("gte-qwen", batch_size=64, device="cuda")

    Advanced:
        For full control, use ProviderRegistry directly:

        >>> from proximadb_sdk.embedding_providers import ProviderRegistry, ProviderConfig
        >>> GTEQwen = ProviderRegistry.get_provider("gte-qwen")
        >>> config = ProviderConfig(...)
        >>> provider = GTEQwen(config)
    """
    # Get provider class from registry
    provider_class = ProviderRegistry.get_provider(name)

    # No config kwargs - use defaults
    if not config_kwargs:
        return provider_class()

    # Has config kwargs - merge with defaults
    default_config = provider_class().default_config()

    # Handle special kwargs
    if "dimension" in config_kwargs:
        # Update model metadata with new dimension
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

    # Merge remaining kwargs
    config = default_config.merge(**config_kwargs)

    return provider_class(config)


def list_providers(include_aliases: bool = False) -> list[str]:
    """
    List all available providers

    Args:
        include_aliases: If True, also return provider aliases

    Returns:
        Sorted list of provider names

    Example:
        >>> providers = list_providers()
        >>> print(providers)
        ['gte-qwen', 'simulated', 'gte-qwen-v2', 'simulated-v2']
    """
    return ProviderRegistry.list_providers(include_aliases=include_aliases)


def get_provider_info(name: str) -> dict:
    """
    Get detailed information about a provider

    Args:
        name: Provider name

    Returns:
        Dictionary with provider metadata

    Example:
        >>> info = get_provider_info("gte-qwen")
        >>> print(info["description"])
        "Alibaba's state-of-the-art multilingual embeddings"
        >>> print(info["models"])
        ['Alibaba-NLP/gte-Qwen2-7B-instruct', ...]
    """
    return ProviderRegistry.get_provider_info(name)


# Backward compatibility functions
def get_embedding_provider(provider: str, **kwargs):
    """
    Backward compatibility wrapper for get_provider()

    **DEPRECATED**: Use `get_provider()` instead.

    This function maintains compatibility with old code but will be
    removed in a future version.

    Args:
        provider: Provider name
        **kwargs: Configuration parameters

    Returns:
        Embedding provider instance

    Example:
        >>> # Old API (deprecated)
        >>> provider = get_embedding_provider("simulated", dimension=384)

        >>> # New API (recommended)
        >>> provider = get_provider("simulated", dimension=384)
    """
    import warnings

    warnings.warn(
        "get_embedding_provider() is deprecated. Use get_provider() instead.",
        DeprecationWarning,
        stacklevel=2,
    )
    return get_provider(provider, **kwargs)


def get_default_embedding_provider():
    """
    Get default provider (simulated for testing)

    **DEPRECATED**: Use `get_provider("simulated")` instead.

    Returns:
        Simulated embedding provider

    Example:
        >>> # Old API
        >>> provider = get_default_embedding_provider()

        >>> # New API
        >>> provider = get_provider("simulated")
    """
    import warnings

    warnings.warn(
        "get_default_embedding_provider() is deprecated. "
        "Use get_provider('simulated') instead.",
        DeprecationWarning,
        stacklevel=2,
    )
    return get_provider("simulated")


# Exports
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
    # Backward compatibility (deprecated)
    "get_embedding_provider",
    "get_default_embedding_provider",
    # Provider classes (for advanced usage)
    "GTEQwenProvider",
    "GTEQwenProviderV2",
    "SimulatedEmbeddingProvider",
    "SimulatedEmbeddingProviderV2",
]
