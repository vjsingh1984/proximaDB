"""
Factory for creating embedding providers

Provides a unified interface for instantiating different embedding providers.
"""

from typing import Optional, Type, Dict, Any, Union
import warnings
import logging

from .base import EmbeddingProvider, EmbeddingConfig
from .sentence_transformer import SentenceTransformerProvider
from .instructor import InstructorProvider
from .fastembed import FastEmbedProvider
from .openai_compatible import OpenAICompatibleProvider
from .openai_provider import OpenAIProvider
from .cohere import CohereProvider
from .simulated import SimulatedEmbeddingProvider

logger = logging.getLogger(__name__)


class EmbeddingProviderFactory:
    """Factory for creating embedding provider instances"""
    
    # Registry of available providers
    _providers: Dict[str, Type[EmbeddingProvider]] = {
        "sentence-transformer": SentenceTransformerProvider,
        "instructor": InstructorProvider,
        "fastembed": FastEmbedProvider,
        "openai-compatible": OpenAICompatibleProvider,
        "openai": OpenAIProvider,
        "cohere": CohereProvider,
        "simulated": SimulatedEmbeddingProvider,
    }
    
    # Aliases for convenience
    _aliases = {
        "bert": "sentence-transformer",
        "minilm": "sentence-transformer",
        "mpnet": "sentence-transformer",
        "bge": "fastembed",
        "jina": "fastembed",
        "ollama": "openai-compatible",
        "vllm": "openai-compatible",
        "localai": "openai-compatible",
        "test": "simulated",
        "mock": "simulated",
    }
    
    @classmethod
    def create_provider(
        cls,
        provider_type: str,
        config: Optional[EmbeddingConfig] = None,
        **kwargs
    ) -> EmbeddingProvider:
        """
        Create an embedding provider instance
        
        Args:
            provider_type: Type of provider to create
            config: Optional configuration
            **kwargs: Additional parameters for provider initialization
            
        Returns:
            Configured embedding provider instance
            
        Raises:
            ValueError: If provider type is not supported
        """
        # Resolve aliases
        provider_type = provider_type.lower()
        if provider_type in cls._aliases:
            provider_type = cls._aliases[provider_type]
        
        if provider_type not in cls._providers:
            available = list(cls._providers.keys()) + list(cls._aliases.keys())
            raise ValueError(
                f"Unknown embedding provider: {provider_type}. "
                f"Available providers: {available}"
            )
        
        # Warn about paid providers
        if provider_type in ["openai", "cohere"]:
            warnings.warn(
                f"⚠️  {provider_type.upper()} is a PAID service that requires an API key! "
                f"Consider using free alternatives like 'sentence-transformer' or 'fastembed' for development.",
                UserWarning,
                stacklevel=2
            )
        
        # Create provider
        provider_class = cls._providers[provider_type]
        
        if config:
            # Merge kwargs into config's extra_params
            if kwargs:
                config.extra_params = config.extra_params or {}
                config.extra_params.update(kwargs)
            return provider_class(config)
        else:
            # Create config from kwargs
            return provider_class(EmbeddingConfig(**kwargs))
    
    @classmethod
    def register_provider(
        cls,
        name: str,
        provider_class: Type[EmbeddingProvider],
        aliases: Optional[list] = None
    ) -> None:
        """
        Register a custom embedding provider
        
        Args:
            name: Name for the provider
            provider_class: Provider implementation class
            aliases: Optional list of aliases
        """
        cls._providers[name] = provider_class
        
        if aliases:
            for alias in aliases:
                cls._aliases[alias] = name
    
    @classmethod
    def list_providers(cls) -> Dict[str, Dict[str, Any]]:
        """List all available providers with details"""
        return {
            "sentence-transformer": {
                "description": "HuggingFace sentence-transformers (100+ models)",
                "free": True,
                "local": True,
                "popular_models": ["all-MiniLM-L6-v2", "all-mpnet-base-v2"],
                "install": "pip install sentence-transformers"
            },
            "instructor": {
                "description": "Instruction-following embeddings",
                "free": True,
                "local": True,
                "models": ["instructor-base", "instructor-large", "instructor-xl"],
                "install": "pip install InstructorEmbedding"
            },
            "fastembed": {
                "description": "Fast ONNX-optimized embeddings",
                "free": True,
                "local": True,
                "popular_models": ["BAAI/bge-small-en-v1.5", "jinaai/jina-embeddings-v2-small-en"],
                "install": "pip install fastembed"
            },
            "openai-compatible": {
                "description": "Any OpenAI-compatible API (Ollama, vLLM, etc.)",
                "free": True,  # When used with local models
                "local": True,
                "examples": ["ollama", "vllm", "localai"],
                "install": "pip install requests"
            },
            "openai": {
                "description": "OpenAI's embedding API",
                "free": False,
                "local": False,
                "warning": "💰 REQUIRES API KEY AND COSTS MONEY!",
                "models": ["text-embedding-3-small", "text-embedding-3-large"],
                "install": "pip install openai"
            },
            "cohere": {
                "description": "Cohere's embedding API",
                "free": False,
                "local": False,
                "warning": "💰 REQUIRES API KEY AND COSTS MONEY!",
                "models": ["embed-english-light-v3.0", "embed-english-v3.0"],
                "install": "pip install cohere"
            },
            "simulated": {
                "description": "Simulated embeddings for testing",
                "free": True,
                "local": True,
                "use_case": "Testing and development only",
                "install": "No dependencies required"
            }
        }


def get_embedding_provider(
    provider: Union[str, EmbeddingProvider] = "sentence-transformer",
    **kwargs
) -> EmbeddingProvider:
    """
    Convenience function to get an embedding provider
    
    Args:
        provider: Provider name or instance
        **kwargs: Configuration parameters
        
    Returns:
        Configured embedding provider
        
    Example:
        # Free, local provider
        provider = get_embedding_provider("fastembed", model_name="BAAI/bge-small-en-v1.5")
        
        # Ollama provider
        provider = get_embedding_provider("ollama", model_name="nomic-embed-text")
        
        # Paid provider (warning will be shown)
        provider = get_embedding_provider("openai", api_key="sk-...")
    """
    if isinstance(provider, EmbeddingProvider):
        return provider
    
    return EmbeddingProviderFactory.create_provider(provider, **kwargs)


def get_default_embedding_provider() -> EmbeddingProvider:
    """
    Get the default embedding provider (free, no dependencies)
    
    Returns SimulatedEmbeddingProvider for immediate use without
    any external dependencies.
    """
    logger.info("Using simulated embeddings. For real embeddings, install sentence-transformers or fastembed.")
    return get_embedding_provider("simulated")


def recommend_free_providers() -> None:
    """Print recommendations for free embedding providers"""
    print("\n🆓 Recommended FREE Embedding Providers:\n")
    
    print("1. FastEmbed (Fastest, lightweight)")
    print("   pip install fastembed")
    print("   provider = get_embedding_provider('fastembed', model_name='BAAI/bge-small-en-v1.5')")
    print()
    
    print("2. Sentence-Transformers (Most models)")
    print("   pip install sentence-transformers")  
    print("   provider = get_embedding_provider('sentence-transformer', model_name='all-MiniLM-L6-v2')")
    print()
    
    print("3. Instructor (Task-specific embeddings)")
    print("   pip install InstructorEmbedding")
    print("   provider = get_embedding_provider('instructor', model_name='hkunlp/instructor-base')")
    print()
    
    print("4. Ollama (Local server)")
    print("   # Install Ollama, then:")
    print("   provider = get_embedding_provider('ollama', model_name='nomic-embed-text')")
    print()
    
    print("⚠️  Paid providers (OpenAI, Cohere) require API keys and incur costs!")
    print("   Only use these for production when you need their specific capabilities.\n")