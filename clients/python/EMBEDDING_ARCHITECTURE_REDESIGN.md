# Embedding Providers Architecture Redesign

## Current Issues

### 1. **Code Duplication**
- Each provider (BGE, E5, SFR, gte-Qwen) repeats similar initialization logic
- Model loading code is duplicated across providers
- Normalization, batching, and dimension detection repeated everywhere

### 2. **Clunky Factory Pattern**
- Factory has hardcoded default models and dimensions
- Adding new providers requires modifying factory code
- No clean separation between provider metadata and implementation

### 3. **Poor Extensibility**
- No plugin system for third-party providers
- Hardcoded provider registry in factory.py
- Difficult to add new model variants without code changes

### 4. **Missing Features**
- No provider lifecycle management (init, warmup, cleanup)
- No model caching/pooling across provider instances
- No automatic model discovery from HuggingFace
- No provider-level configuration inheritance

### 5. **Performance Issues**
- Models loaded on every provider instantiation
- No shared model instances across requests
- No async/concurrent embedding support
- No batch optimization strategies

### 6. **Testing Challenges**
- Hard to mock providers for testing
- No clear provider interface contract
- Integration tests require real model downloads

---

## Optimized Architecture

### Design Principles

1. **Composition over Inheritance** - Use mixins for shared functionality
2. **Plugin-Based** - Dynamic provider registration via decorators
3. **Lazy Loading** - Load models only when first used
4. **Singleton Models** - Share model instances across provider instances
5. **Type Safety** - Full type hints and runtime validation
6. **Async-First** - Support both sync and async APIs

---

### New Architecture Components

```
embedding_providers/
├── __init__.py                 # Public API exports
├── core/
│   ├── __init__.py
│   ├── base.py                 # Abstract base classes
│   ├── config.py               # Configuration classes
│   ├── registry.py             # Provider registry system
│   ├── lifecycle.py            # Provider lifecycle management
│   └── cache.py                # Model caching and pooling
├── mixins/
│   ├── __init__.py
│   ├── sentence_transformer.py # SentenceTransformer mixin
│   ├── normalization.py        # Embedding normalization mixin
│   ├── batching.py             # Batch processing mixin
│   └── instruction.py          # Query instruction mixin
├── providers/
│   ├── __init__.py
│   ├── local/                  # Local model providers
│   │   ├── __init__.py
│   │   ├── gte_qwen.py
│   │   ├── sfr.py
│   │   ├── bge.py
│   │   ├── e5.py
│   │   └── sentence_transformer.py
│   ├── api/                    # API-based providers
│   │   ├── __init__.py
│   │   ├── openai.py
│   │   └── cohere.py
│   └── testing/
│       ├── __init__.py
│       └── simulated.py
├── factory.py                  # Smart factory with auto-discovery
└── utils/
    ├── __init__.py
    ├── model_info.py           # HuggingFace model metadata
    └── validators.py           # Input validation utilities
```

---

## Implementation Plan

### Phase 1: Core Infrastructure (This Session)

#### 1.1 New Base Classes (`core/base.py`)

```python
from abc import ABC, abstractmethod
from typing import Optional, List, Dict, Any
import numpy as np

class EmbeddingProviderProtocol(Protocol):
    """Type protocol for embedding providers"""
    def embed(self, texts: List[str]) -> np.ndarray: ...
    def get_dimension(self) -> int: ...

class BaseEmbeddingProvider(ABC):
    """Enhanced base class with lifecycle support"""

    def __init__(self, config: Optional['ProviderConfig'] = None):
        self.config = config or self.default_config()
        self._initialized = False
        self._model = None

    @abstractmethod
    def default_config(self) -> 'ProviderConfig':
        """Return default configuration"""
        pass

    @abstractmethod
    def _load_model(self):
        """Load the embedding model"""
        pass

    def ensure_initialized(self):
        """Lazy initialization"""
        if not self._initialized:
            self._model = self._load_model()
            self._initialized = True

    @abstractmethod
    def embed(self, texts: List[str]) -> np.ndarray:
        """Generate embeddings (concrete implementation)"""
        pass

    def cleanup(self):
        """Cleanup resources"""
        if self._model is not None:
            del self._model
            self._model = None
        self._initialized = False
```

#### 1.2 Provider Configuration (`core/config.py`)

```python
from dataclasses import dataclass, field
from typing import Optional, Dict, Any

@dataclass(frozen=True)
class ModelMetadata:
    """Immutable model metadata"""
    name: str
    dimension: int
    max_length: int = 512
    provider_type: str = "sentence-transformer"
    requires_instruction: bool = False
    instruction_template: Optional[str] = None
    mteb_score: Optional[float] = None
    languages: str = "en"

@dataclass
class ProviderConfig:
    """Unified provider configuration"""
    model: ModelMetadata
    batch_size: int = 32
    normalize: bool = True
    device: Optional[str] = None
    cache_dir: Optional[str] = None
    trust_remote_code: bool = False
    extra: Dict[str, Any] = field(default_factory=dict)

    def merge(self, **kwargs) -> 'ProviderConfig':
        """Return new config with updated values"""
        data = asdict(self)
        data['extra'].update(kwargs.pop('extra', {}))
        data.update(kwargs)
        return ProviderConfig(**data)
```

#### 1.3 Provider Registry (`core/registry.py`)

```python
from typing import Dict, Type, Optional, Callable
from functools import wraps

class ProviderRegistry:
    """Global provider registry with auto-discovery"""

    _providers: Dict[str, Type[BaseEmbeddingProvider]] = {}
    _metadata: Dict[str, Dict[str, ModelMetadata]] = {}
    _aliases: Dict[str, str] = {}

    @classmethod
    def register(
        cls,
        name: str,
        models: Dict[str, ModelMetadata],
        aliases: Optional[List[str]] = None
    ):
        """Decorator for provider registration"""
        def decorator(provider_class: Type[BaseEmbeddingProvider]):
            cls._providers[name] = provider_class
            cls._metadata[name] = models

            # Register aliases
            for alias in (aliases or []):
                cls._aliases[alias] = name

            return provider_class
        return decorator

    @classmethod
    def get_provider(cls, name: str) -> Type[BaseEmbeddingProvider]:
        """Get provider class by name"""
        name = cls._aliases.get(name, name)
        if name not in cls._providers:
            raise ValueError(f"Unknown provider: {name}")
        return cls._providers[name]

    @classmethod
    def get_models(cls, provider_name: str) -> Dict[str, ModelMetadata]:
        """Get available models for provider"""
        provider_name = cls._aliases.get(provider_name, provider_name)
        return cls._metadata.get(provider_name, {})

    @classmethod
    def list_providers(cls) -> List[str]:
        """List all registered providers"""
        return sorted(cls._providers.keys())
```

#### 1.4 Model Cache (`core/cache.py`)

```python
from typing import Optional, Any
import threading

class ModelCache:
    """Thread-safe singleton model cache"""

    _instance = None
    _lock = threading.Lock()
    _models: Dict[str, Any] = {}

    def __new__(cls):
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:
                    cls._instance = super().__new__(cls)
        return cls._instance

    def get_or_load(
        self,
        key: str,
        loader: Callable[[], Any]
    ) -> Any:
        """Get cached model or load it"""
        if key not in self._models:
            with self._lock:
                if key not in self._models:
                    self._models[key] = loader()
        return self._models[key]

    def clear(self, key: Optional[str] = None):
        """Clear cache entry or entire cache"""
        with self._lock:
            if key:
                self._models.pop(key, None)
            else:
                self._models.clear()
```

---

### Phase 2: Mixins for Shared Functionality

#### 2.1 SentenceTransformer Mixin (`mixins/sentence_transformer.py`)

```python
class SentenceTransformerMixin:
    """Mixin for sentence-transformers based providers"""

    def _load_model(self):
        """Load sentence-transformer model with caching"""
        from sentence_transformers import SentenceTransformer

        cache = ModelCache()
        cache_key = f"st_{self.config.model.name}"

        return cache.get_or_load(
            cache_key,
            lambda: SentenceTransformer(
                self.config.model.name,
                device=self.config.device,
                trust_remote_code=self.config.trust_remote_code,
                cache_folder=self.config.cache_dir
            )
        )

    def embed(self, texts: List[str]) -> np.ndarray:
        """Generate embeddings using sentence-transformers"""
        self.ensure_initialized()

        embeddings = self._model.encode(
            texts,
            batch_size=self.config.batch_size,
            normalize_embeddings=self.config.normalize,
            show_progress_bar=False
        )
        return embeddings
```

#### 2.2 Instruction Mixin (`mixins/instruction.py`)

```python
class InstructionMixin:
    """Mixin for providers with query instructions"""

    def apply_instruction(self, text: str, is_query: bool = True) -> str:
        """Apply instruction template to text"""
        if not is_query or not self.config.model.requires_instruction:
            return text

        template = self.config.model.instruction_template
        if not template:
            return text

        return template.format(query=text)

    def embed_query(self, query: str) -> np.ndarray:
        """Embed query with instruction"""
        instructed = self.apply_instruction(query, is_query=True)
        return self.embed([instructed])[0]
```

---

### Phase 3: Refactored Providers

#### 3.1 Example: gte-Qwen Provider

```python
from ..core.base import BaseEmbeddingProvider
from ..core.config import ProviderConfig, ModelMetadata
from ..core.registry import ProviderRegistry
from ..mixins.sentence_transformer import SentenceTransformerMixin
from ..mixins.instruction import InstructionMixin

# Model metadata
GTE_QWEN_MODELS = {
    "Alibaba-NLP/gte-Qwen2-7B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen2-7B-instruct",
        dimension=3584,
        max_length=32768,
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages\\nQuery: {query}",
        mteb_score=71.0,
        languages="100+"
    ),
    "Alibaba-NLP/gte-Qwen2-1.5B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen2-1.5B-instruct",
        dimension=1536,
        max_length=32768,
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages\\nQuery: {query}",
        mteb_score=68.0,
        languages="100+"
    )
}

@ProviderRegistry.register(
    name="gte-qwen",
    models=GTE_QWEN_MODELS,
    aliases=["alibaba", "qwen", "gte"]
)
class GTEQwenProvider(
    InstructionMixin,
    SentenceTransformerMixin,
    BaseEmbeddingProvider
):
    """Optimized gte-Qwen provider"""

    def default_config(self) -> ProviderConfig:
        """Default configuration"""
        return ProviderConfig(
            model=GTE_QWEN_MODELS["Alibaba-NLP/gte-Qwen2-1.5B-instruct"],
            batch_size=16,
            normalize=True,
            trust_remote_code=False  # Use standard implementation
        )
```

**Lines of code: ~30 (vs 336 in current implementation)**

---

### Phase 4: Smart Factory

```python
class EmbeddingProviderFactory:
    """Smart factory with auto-discovery"""

    @staticmethod
    def create(
        provider_name: str,
        model_name: Optional[str] = None,
        **kwargs
    ) -> BaseEmbeddingProvider:
        """
        Create provider with intelligent defaults

        Examples:
            # Use default model
            provider = EmbeddingProviderFactory.create("gte-qwen")

            # Specify model
            provider = EmbeddingProviderFactory.create(
                "gte-qwen",
                model_name="Alibaba-NLP/gte-Qwen2-7B-instruct"
            )

            # Custom config
            provider = EmbeddingProviderFactory.create(
                "bge",
                batch_size=64,
                device="cuda"
            )
        """
        # Get provider class
        provider_class = ProviderRegistry.get_provider(provider_name)

        # Get default config
        config = provider_class().default_config()

        # Override model if specified
        if model_name:
            models = ProviderRegistry.get_models(provider_name)
            if model_name not in models:
                raise ValueError(f"Unknown model: {model_name}")
            config = config.merge(model=models[model_name])

        # Merge kwargs
        if kwargs:
            config = config.merge(**kwargs)

        return provider_class(config)

    @staticmethod
    def list_providers() -> Dict[str, List[str]]:
        """List all providers with their models"""
        result = {}
        for provider_name in ProviderRegistry.list_providers():
            models = ProviderRegistry.get_models(provider_name)
            result[provider_name] = list(models.keys())
        return result
```

---

## Benefits

### 1. **Extensibility**
- Add new providers with ~30 lines of code
- Register providers via decorator
- No factory code changes needed

### 2. **Performance**
- Model caching reduces memory usage
- Lazy loading improves startup time
- Shared models across instances

### 3. **Maintainability**
- DRY principle via mixins
- Clear separation of concerns
- Type-safe with full hints

### 4. **Testing**
- Easy to mock with protocols
- Provider registration is testable
- Clear interface contracts

### 5. **Developer Experience**
- Auto-discovery of providers
- Intelligent defaults
- Comprehensive error messages

---

## Migration Path

### Backward Compatibility

```python
# Old API (still works)
from proximadb.embedding_providers import get_embedding_provider
provider = get_embedding_provider("simulated", dimension=384)

# New API (recommended)
from proximadb.embedding_providers import EmbeddingProviderFactory
provider = EmbeddingProviderFactory.create("gte-qwen")
```

### Migration Steps

1. Implement core infrastructure (core/, mixins/)
2. Refactor one provider as proof-of-concept
3. Add backward compatibility layer
4. Migrate remaining providers
5. Deprecate old API (with warnings)
6. Remove deprecated code in next major version

---

## Implementation Checklist

- [ ] Create core/ module structure
- [ ] Implement BaseEmbeddingProvider
- [ ] Implement ProviderConfig and ModelMetadata
- [ ] Implement ProviderRegistry
- [ ] Implement ModelCache
- [ ] Create mixins/ module structure
- [ ] Implement SentenceTransformerMixin
- [ ] Implement InstructionMixin
- [ ] Refactor gte-Qwen provider as POC
- [ ] Implement new Factory
- [ ] Add backward compatibility layer
- [ ] Migrate all providers
- [ ] Update tests
- [ ] Update documentation

---

## Success Metrics

- **Code Reduction**: 70%+ reduction in provider code
- **Performance**: 50%+ reduction in memory usage (model caching)
- **Extensibility**: Add new provider in <50 lines
- **Test Coverage**: 95%+ coverage on core modules
- **Backward Compatibility**: 100% of existing tests pass
