"""
Configuration classes for embedding providers

Provides immutable model metadata and flexible provider configuration.
"""

from dataclasses import asdict, dataclass, field
from typing import Any


@dataclass(frozen=True)
class ModelMetadata:
    """
    Immutable model metadata

    Contains all static information about an embedding model including
    dimensions, context length, performance scores, and usage requirements.

    Attributes:
        name: Model identifier (HuggingFace model ID or custom name)
        dimension: Embedding vector dimension
        max_length: Maximum input sequence length (in tokens)
        provider_type: Type of provider (e.g., "sentence-transformer", "api")
        requires_instruction: Whether model needs instruction prefix for queries
        instruction_template: Template string for query instructions (if required)
        mteb_score: MTEB benchmark score (if available)
        languages: Supported languages ("en", "multilingual", "100+", etc.)
        description: Human-readable description
        use_case: Recommended use cases
    """

    name: str
    dimension: int
    max_length: int = 512
    provider_type: str = "sentence-transformer"
    requires_instruction: bool = False
    instruction_template: str | None = None
    mteb_score: float | None = None
    languages: str = "en"
    description: str = ""
    use_case: str = ""

    def __str__(self) -> str:
        """Human-readable representation"""
        parts = [f"{self.name} ({self.dimension}D)"]
        if self.mteb_score:
            parts.append(f"MTEB: {self.mteb_score}")
        if self.languages != "en":
            parts.append(f"Lang: {self.languages}")
        return " | ".join(parts)


@dataclass
class ProviderConfig:
    """
    Flexible provider configuration

    Supports configuration inheritance and merging. All providers use this
    standard configuration format.

    Attributes:
        model: Model metadata (immutable)
        batch_size: Number of texts to process in each batch
        normalize: Whether to L2-normalize embeddings
        device: Device for computation ("cpu", "cuda", "mps", None=auto-detect)
        cache_dir: Directory for caching models (None=use default)
        trust_remote_code: Whether to allow custom model code execution
        extra: Provider-specific additional parameters
    """

    model: ModelMetadata
    batch_size: int = 32
    normalize: bool = True
    device: str | None = None
    cache_dir: str | None = None
    trust_remote_code: bool = False
    extra: dict[str, Any] = field(default_factory=dict)

    def merge(self, **kwargs) -> "ProviderConfig":
        """
        Create new configuration with updated values

        Args:
            **kwargs: Configuration parameters to override

        Returns:
            New ProviderConfig instance with merged values

        Example:
            >>> config = ProviderConfig(model=..., batch_size=32)
            >>> new_config = config.merge(batch_size=64, normalize=False)
            >>> assert new_config.batch_size == 64
            >>> assert config.batch_size == 32  # Original unchanged
        """
        data = asdict(self)

        # Handle extra params specially - merge instead of replace
        if "extra" in kwargs:
            data["extra"].update(kwargs.pop("extra"))

        # Update with remaining kwargs
        data.update(kwargs)

        # Reconstruct model metadata if needed
        if "model" in data and isinstance(data["model"], dict):
            data["model"] = ModelMetadata(**data["model"])

        return ProviderConfig(**data)

    def __str__(self) -> str:
        """Human-readable representation"""
        return (
            f"ProviderConfig(\n"
            f"  model={self.model.name},\n"
            f"  dimension={self.model.dimension},\n"
            f"  batch_size={self.batch_size},\n"
            f"  normalize={self.normalize},\n"
            f"  device={self.device or 'auto'}\n"
            f")"
        )
