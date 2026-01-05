"""
Provider registry system

Provides dynamic provider registration and discovery via decorators.
"""

from typing import Dict, Type, List, Optional
import logging

from .base import BaseEmbeddingProvider
from .config import ModelMetadata

logger = logging.getLogger(__name__)


class ProviderRegistry:
    """
    Global provider registry with auto-discovery

    This singleton registry allows providers to self-register using decorators.
    No need to modify factory code when adding new providers.

    Example:
        @ProviderRegistry.register(
            name="my-provider",
            models={"model-1": ModelMetadata(...)},
            aliases=["alias1", "alias2"]
        )
        class MyProvider(BaseEmbeddingProvider):
            ...
    """

    _providers: Dict[str, Type[BaseEmbeddingProvider]] = {}
    _metadata: Dict[str, Dict[str, ModelMetadata]] = {}
    _aliases: Dict[str, str] = {}
    _descriptions: Dict[str, str] = {}

    @classmethod
    def register(
        cls,
        name: str,
        models: Dict[str, ModelMetadata],
        aliases: Optional[List[str]] = None,
        description: str = "",
    ):
        """
        Decorator for provider registration

        Args:
            name: Primary provider name (e.g., "gte-qwen")
            models: Dictionary mapping model names to ModelMetadata
            aliases: Optional list of alternative names (e.g., ["alibaba", "qwen"])
            description: Human-readable provider description

        Returns:
            Decorator function

        Example:
            @ProviderRegistry.register(
                name="gte-qwen",
                models=GTE_QWEN_MODELS,
                aliases=["alibaba", "qwen"],
                description="Alibaba's state-of-the-art multilingual embeddings"
            )
            class GTEQwenProvider(BaseEmbeddingProvider):
                ...
        """

        def decorator(provider_class: Type[BaseEmbeddingProvider]):
            # Validate provider class
            if not issubclass(provider_class, BaseEmbeddingProvider):
                raise TypeError(
                    f"{provider_class.__name__} must inherit from BaseEmbeddingProvider"
                )

            # Register provider
            cls._providers[name] = provider_class
            cls._metadata[name] = models
            cls._descriptions[name] = description

            # Register aliases
            for alias in aliases or []:
                if alias in cls._aliases:
                    logger.warning(
                        f"Alias '{alias}' already registered for '{cls._aliases[alias]}', "
                        f"overriding with '{name}'"
                    )
                cls._aliases[alias] = name

            logger.debug(
                f"Registered provider: {name} "
                f"({len(models)} models, {len(aliases or [])} aliases)"
            )

            return provider_class

        return decorator

    @classmethod
    def get_provider(cls, name: str) -> Type[BaseEmbeddingProvider]:
        """
        Get provider class by name or alias

        Args:
            name: Provider name or alias

        Returns:
            Provider class

        Raises:
            ValueError: If provider not found

        Example:
            >>> provider_class = ProviderRegistry.get_provider("gte-qwen")
            >>> provider_class = ProviderRegistry.get_provider("alibaba")  # alias
        """
        # Resolve alias
        original_name = name
        name = name.lower()
        name = cls._aliases.get(name, name)

        if name not in cls._providers:
            available = sorted(
                set(list(cls._providers.keys()) + list(cls._aliases.keys()))
            )
            raise ValueError(
                f"Unknown embedding provider: '{original_name}'. "
                f"Available providers: {available}"
            )

        return cls._providers[name]

    @classmethod
    def get_models(cls, provider_name: str) -> Dict[str, ModelMetadata]:
        """
        Get available models for a provider

        Args:
            provider_name: Provider name or alias

        Returns:
            Dictionary mapping model names to ModelMetadata

        Example:
            >>> models = ProviderRegistry.get_models("gte-qwen")
            >>> print(list(models.keys()))
            ['Alibaba-NLP/gte-Qwen2-7B-instruct', 'Alibaba-NLP/gte-Qwen2-1.5B-instruct']
        """
        provider_name = provider_name.lower()
        provider_name = cls._aliases.get(provider_name, provider_name)
        return cls._metadata.get(provider_name, {})

    @classmethod
    def get_default_model(cls, provider_name: str) -> Optional[ModelMetadata]:
        """
        Get the default model for a provider

        The default is the first model in the models dictionary.

        Args:
            provider_name: Provider name or alias

        Returns:
            Default ModelMetadata, or None if no models

        Example:
            >>> model = ProviderRegistry.get_default_model("gte-qwen")
            >>> print(model.name)
            'Alibaba-NLP/gte-Qwen2-7B-instruct'
        """
        models = cls.get_models(provider_name)
        if not models:
            return None
        return next(iter(models.values()))

    @classmethod
    def list_providers(cls, include_aliases: bool = False) -> List[str]:
        """
        List all registered providers

        Args:
            include_aliases: If True, also return aliases

        Returns:
            Sorted list of provider names

        Example:
            >>> providers = ProviderRegistry.list_providers()
            >>> print(providers)
            ['bge', 'e5', 'gte-qwen', 'sfr', 'simulated']
        """
        if include_aliases:
            return sorted(set(list(cls._providers.keys()) + list(cls._aliases.keys())))
        return sorted(cls._providers.keys())

    @classmethod
    def get_provider_info(cls, provider_name: str) -> Dict[str, any]:
        """
        Get comprehensive provider information

        Args:
            provider_name: Provider name or alias

        Returns:
            Dictionary with provider metadata

        Example:
            >>> info = ProviderRegistry.get_provider_info("gte-qwen")
            >>> print(info['description'])
            "Alibaba's state-of-the-art multilingual embeddings"
        """
        provider_name = provider_name.lower()
        provider_name = cls._aliases.get(provider_name, provider_name)

        if provider_name not in cls._providers:
            raise ValueError(f"Unknown provider: {provider_name}")

        models = cls._metadata.get(provider_name, {})
        return {
            "name": provider_name,
            "class": cls._providers[provider_name].__name__,
            "description": cls._descriptions.get(provider_name, ""),
            "num_models": len(models),
            "models": list(models.keys()),
            "default_model": next(iter(models.keys())) if models else None,
        }

    @classmethod
    def clear(cls):
        """
        Clear the registry (mainly for testing)

        Warning: This removes all registered providers!
        """
        cls._providers.clear()
        cls._metadata.clear()
        cls._aliases.clear()
        cls._descriptions.clear()
        logger.warning("Provider registry cleared")
