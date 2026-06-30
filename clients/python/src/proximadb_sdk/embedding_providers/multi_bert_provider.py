"""
Multi-BERT Embedding Providers for ProximaDB

Various BERT model sizes for different latency/accuracy/memory budgets:
- MiniLM: fast, lightweight (384 dims)
- BERT/RoBERTa/MPNet base: balanced (768 dims)
- BERT/RoBERTa/E5 large: higher accuracy (1024 dims)
- DeBERTa xlarge: maximum accuracy (1536 dims)

Ported onto :class:`core.BaseEmbeddingProvider` (TD-126 System-B collapse).
Self-registers under ``multi-bert``; :class:`AdaptiveBERTProvider` registers
under ``adaptive-bert``. Heavy ``torch`` / ``transformers`` /
``sentence_transformers`` imports are performed lazily inside the model-loading
and encoding paths so importing this module pulls no model dependencies.

The provider keeps its ergonomic constructor
(``MultiBERTProvider(model_name=..., size=..., device=...)``) while also
accepting a standard :class:`ProviderConfig` for parity with the registry.
"""

import logging
import os
from enum import Enum
from pathlib import Path
from typing import Any

import numpy as np

from .core.base import BaseEmbeddingProvider
from .core.config import ModelMetadata, ProviderConfig
from .core.device import resolve_device
from .core.registry import ProviderRegistry

logger = logging.getLogger(__name__)


class ModelSize(Enum):
    """Model size options"""

    MINI = "mini"  # ~20MB, 384 dims, fastest
    SMALL = "small"  # ~100MB, 512 dims
    BASE = "base"  # ~400MB, 768 dims, balanced
    LARGE = "large"  # ~1.3GB, 1024 dims
    XLARGE = "xlarge"  # ~3GB, 1536 dims, most accurate


# Build registry ModelMetadata from the rich internal MODELS spec below.
def _models_to_metadata(models: dict[str, dict]) -> dict[str, ModelMetadata]:
    return {
        cfg["name"]: ModelMetadata(
            name=cfg["name"],
            dimension=cfg["dimension"],
            max_length=512,
            provider_type=(
                "sentence-transformer"
                if cfg.get("is_sentence_transformer")
                else "transformers"
            ),
            languages="en",
            description=cfg.get("description", ""),
            use_case=f"{cfg['size'].value} model ({cfg.get('memory', '?')})",
        )
        for cfg in models.values()
    }


@ProviderRegistry.register(
    name="multi-bert",
    models=_models_to_metadata(
        {
            "mpnet-base": {
                "name": "sentence-transformers/all-mpnet-base-v2",
                "dimension": 768,
                "size": ModelSize.BASE,
                "memory": "~420MB",
                "description": "High quality general purpose",
                "is_sentence_transformer": True,
            },
        }
    ),
    aliases=["bert", "multibert"],
    description="Multi-size BERT embeddings (MiniLM/BERT/RoBERTa/MPNet/DeBERTa)",
)
class MultiBERTProvider(BaseEmbeddingProvider):
    """
    Multi-size BERT embedding provider with various model options.

    Features:
    - Multiple model sizes from Mini to XLarge
    - Automatic model selection based on available resources
    - Configurable token pooling for the transformers backend
    - Per-text caching keyed on model + content
    """

    # Available models by size and capability.
    MODELS = {
        # Mini models (fastest, least memory)
        "minilm-l6": {
            "name": "sentence-transformers/all-MiniLM-L6-v2",
            "dimension": 384,
            "size": ModelSize.MINI,
            "speed": "fastest",
            "memory": "~22MB",
            "description": "Lightweight, fast inference",
            "is_sentence_transformer": True,
        },
        "minilm-l12": {
            "name": "sentence-transformers/all-MiniLM-L12-v2",
            "dimension": 384,
            "size": ModelSize.MINI,
            "speed": "very fast",
            "memory": "~40MB",
            "description": "Better quality mini model",
            "is_sentence_transformer": True,
        },
        # Small models
        "distilbert": {
            "name": "sentence-transformers/all-distilroberta-v1",
            "dimension": 768,
            "size": ModelSize.SMALL,
            "speed": "fast",
            "memory": "~100MB",
            "description": "Distilled model, good balance",
            "is_sentence_transformer": True,
        },
        # Base models (balanced)
        "bert-base": {
            "name": "bert-base-uncased",
            "dimension": 768,
            "size": ModelSize.BASE,
            "speed": "moderate",
            "memory": "~420MB",
            "description": "Original BERT base",
            "is_sentence_transformer": False,
        },
        "mpnet-base": {
            "name": "sentence-transformers/all-mpnet-base-v2",
            "dimension": 768,
            "size": ModelSize.BASE,
            "speed": "moderate",
            "memory": "~420MB",
            "description": "High quality general purpose",
            "is_sentence_transformer": True,
        },
        "roberta-base": {
            "name": "roberta-base",
            "dimension": 768,
            "size": ModelSize.BASE,
            "speed": "moderate",
            "memory": "~480MB",
            "description": "Robust BERT variant",
            "is_sentence_transformer": False,
        },
        # Large models (higher accuracy)
        "bert-large": {
            "name": "bert-large-uncased",
            "dimension": 1024,
            "size": ModelSize.LARGE,
            "speed": "slow",
            "memory": "~1.3GB",
            "description": "Original BERT large",
            "is_sentence_transformer": False,
        },
        "roberta-large": {
            "name": "roberta-large",
            "dimension": 1024,
            "size": ModelSize.LARGE,
            "speed": "slow",
            "memory": "~1.4GB",
            "description": "Large robust model",
            "is_sentence_transformer": False,
        },
        "e5-large": {
            "name": "intfloat/e5-large-v2",
            "dimension": 1024,
            "size": ModelSize.LARGE,
            "speed": "slow",
            "memory": "~1.3GB",
            "description": "State-of-the-art embeddings",
            "is_sentence_transformer": True,
        },
        # XLarge models (maximum accuracy)
        "deberta-large": {
            "name": "microsoft/deberta-v3-large",
            "dimension": 1024,
            "size": ModelSize.XLARGE,
            "speed": "very slow",
            "memory": "~1.5GB",
            "description": "DeBERTa v3 large",
            "is_sentence_transformer": False,
        },
        "deberta-xlarge": {
            "name": "microsoft/deberta-v2-xlarge",
            "dimension": 1536,
            "size": ModelSize.XLARGE,
            "speed": "slowest",
            "memory": "~3GB",
            "description": "Maximum quality DeBERTa",
            "is_sentence_transformer": False,
        },
    }

    DEFAULT_MODEL = "mpnet-base"

    def __init__(
        self,
        config: ProviderConfig | None = None,
        *,
        model_name: str | None = None,
        size: ModelSize | None = None,
        device: str | None = None,
        cache_dir: str | None = None,
        batch_size: int = 32,
        normalize: bool = True,
        pooling_strategy: str = "mean",
        max_memory_gb: float | None = None,
    ):
        """
        Initialize the provider.

        Accepts either a standard :class:`ProviderConfig` (registry / get_provider
        path) or the ergonomic keyword form (``model_name``/``size``/...). When a
        ``config`` is supplied the keyword model selectors are ignored.
        """
        if config is not None:
            self._init_from_config(config)
            return

        # Ergonomic / legacy keyword constructor.
        if size and not model_name:
            model_name = self._select_model_by_size(size, max_memory_gb)
        elif not model_name:
            model_name = self._auto_select_model(max_memory_gb)

        if model_name not in self.MODELS:
            raise ValueError(
                f"Model {model_name} not found. Available: {list(self.MODELS.keys())}"
            )

        spec = self.MODELS[model_name]
        resolved_device = device
        if resolved_device is None:
            resolved_device = self._auto_device(model_name)

        built = ProviderConfig(
            model=self._spec_to_metadata(model_name),
            batch_size=batch_size,
            normalize=normalize,
            device=resolved_device,
            cache_dir=cache_dir,
            extra={"pooling_strategy": pooling_strategy, "model_key": model_name},
        )
        self._init_from_config(built)
        # Eager-load to preserve the legacy contract (``.model`` available after
        # construction, used by tests + auto-selection diagnostics).
        self.ensure_initialized()
        logger.info(
            "MultiBERT initialized: %s (%s, %dd) on %s",
            model_name,
            spec["memory"],
            spec["dimension"],
            self.device,
        )

    def default_config(self) -> ProviderConfig:
        """Default configuration (the balanced MPNet base model)."""
        return ProviderConfig(
            model=self._spec_to_metadata(self.DEFAULT_MODEL),
            batch_size=32,
            normalize=True,
            extra={"pooling_strategy": "mean", "model_key": self.DEFAULT_MODEL},
        )

    @property
    def model_name(self) -> str:
        """Short model key (e.g. ``"mpnet-base"``), not the HF path."""
        return self._model_key

    @model_name.setter
    def model_name(self, value: str) -> None:
        self._model_key = value

    def _init_from_config(self, config: ProviderConfig) -> None:
        super().__init__(config)
        self._model_key = config.extra.get("model_key") or self._key_for_model_name(
            config.model.name
        )
        self.model_config = self.MODELS.get(self.model_name, {})
        self.batch_size = config.batch_size
        self.normalize = config.normalize
        self.pooling_strategy = config.extra.get("pooling_strategy", "mean")
        self.device = resolve_device(config.device) or "cpu"
        self._cache: dict[str, np.ndarray] = {}
        if config.cache_dir:
            self.cache_dir = Path(config.cache_dir)
        else:
            self.cache_dir = Path.home() / ".cache" / "proximadb" / "models"
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        os.environ["TRANSFORMERS_CACHE"] = str(self.cache_dir)

    # -- model selection -----------------------------------------------------
    @classmethod
    def _key_for_model_name(cls, hf_name: str) -> str:
        for key, cfg in cls.MODELS.items():
            if cfg["name"] == hf_name:
                return key
        return cls.DEFAULT_MODEL

    @classmethod
    def _spec_to_metadata(cls, model_key: str) -> ModelMetadata:
        cfg = cls.MODELS[model_key]
        return ModelMetadata(
            name=cfg["name"],
            dimension=cfg["dimension"],
            max_length=512,
            provider_type=(
                "sentence-transformer"
                if cfg.get("is_sentence_transformer")
                else "transformers"
            ),
            languages="en",
            description=cfg.get("description", ""),
            use_case=f"{cfg['size'].value} model ({cfg.get('memory', '?')})",
        )

    def _auto_device(self, model_name: str) -> str:
        """Pick cuda/cpu, downgrading XLARGE models off small GPUs."""
        import torch

        if torch.cuda.is_available():
            device = "cuda"
            if self.MODELS[model_name]["size"] == ModelSize.XLARGE:
                gpu_memory = torch.cuda.get_device_properties(0).total_memory / 1e9
                if gpu_memory < 8:
                    logger.warning(
                        "GPU memory (%.1fGB) may be insufficient for %s",
                        gpu_memory,
                        model_name,
                    )
                    device = "cpu"
            return device
        return "cpu"

    def _auto_select_model(self, max_memory_gb: float | None = None) -> str:
        """Auto-select best model based on available resources."""
        import torch

        if torch.cuda.is_available():
            gpu_memory = torch.cuda.get_device_properties(0).total_memory / 1e9
            if gpu_memory >= 8:
                return "e5-large"
            elif gpu_memory >= 4:
                return "mpnet-base"
            else:
                return "minilm-l12"

        import psutil

        ram_gb = psutil.virtual_memory().total / 1e9
        if max_memory_gb:
            ram_gb = min(ram_gb, max_memory_gb)

        if ram_gb >= 16:
            return "mpnet-base"
        elif ram_gb >= 8:
            return "distilbert"
        else:
            return "minilm-l6"

    def _select_model_by_size(
        self, size: ModelSize, max_memory_gb: float | None = None
    ) -> str:
        """Select a model by size preference (prefers sentence-transformers)."""
        candidates = [
            name for name, config in self.MODELS.items() if config["size"] == size
        ]
        if not candidates:
            logger.warning("No models found for size %s, using default", size)
            return self.DEFAULT_MODEL

        st_models = [
            name
            for name in candidates
            if self.MODELS[name].get("is_sentence_transformer", False)
        ]
        return st_models[0] if st_models else candidates[0]

    # -- lifecycle / encoding ------------------------------------------------
    def _is_sentence_transformer(self) -> bool:
        return bool(self.model_config.get("is_sentence_transformer"))

    def _load_model(self) -> Any:
        """Load the selected model (sentence-transformer or transformers tuple)."""
        model_path = self.model_config["name"]
        logger.info("Loading model: %s", model_path)

        if self._is_sentence_transformer():
            from sentence_transformers import SentenceTransformer

            self.tokenizer = None
            model = SentenceTransformer(
                model_path, device=self.device, cache_folder=str(self.cache_dir)
            )
            self.model = model
            logger.info("Model loaded on %s", self.device)
            return model

        from transformers import AutoModel, AutoTokenizer

        self.tokenizer = AutoTokenizer.from_pretrained(
            model_path, cache_dir=self.cache_dir
        )
        model = AutoModel.from_pretrained(model_path, cache_dir=self.cache_dir).to(
            self.device
        )
        model.eval()
        self.model = model
        logger.info("Model loaded on %s", self.device)
        return model

    def embed(self, texts: list[str]) -> np.ndarray:
        """Core embedding entry point (delegates to the cached implementation)."""
        if not texts:
            return np.array([])
        return self.embed_texts(texts)

    def embed_texts(self, texts: list[str]) -> np.ndarray:
        """Generate embeddings for multiple texts, with per-text caching."""
        self.ensure_initialized()

        uncached_texts: list[str] = []
        uncached_indices: list[int] = []
        cached_embeddings: dict[int, np.ndarray] = {}

        for i, text in enumerate(texts):
            cache_key = f"{self.model_name}:{hash(text)}"
            if cache_key in self._cache:
                cached_embeddings[i] = self._cache[cache_key]
            else:
                uncached_texts.append(text)
                uncached_indices.append(i)

        if uncached_texts:
            if self._is_sentence_transformer():
                new_embeddings = self.model.encode(
                    uncached_texts,
                    batch_size=self.batch_size,
                    normalize_embeddings=self.normalize,
                    show_progress_bar=len(uncached_texts) > 100,
                    device=self.device,
                )
            else:
                new_embeddings = self._encode_with_transformers(uncached_texts)

            for text, embedding, idx in zip(
                uncached_texts, new_embeddings, uncached_indices
            ):
                cache_key = f"{self.model_name}:{hash(text)}"
                self._cache[cache_key] = embedding
                cached_embeddings[idx] = embedding

        result = np.zeros((len(texts), self.model_config["dimension"]))
        for i, embedding in cached_embeddings.items():
            result[i] = embedding

        return result

    def _encode_with_transformers(self, texts: list[str]) -> np.ndarray:
        """Encode using the raw transformers backend with configured pooling."""
        import torch
        import torch.nn.functional as F

        embeddings = []
        for i in range(0, len(texts), self.batch_size):
            batch = texts[i : i + self.batch_size]
            inputs = self.tokenizer(
                batch,
                padding=True,
                truncation=True,
                max_length=512,
                return_tensors="pt",
            ).to(self.device)

            with torch.no_grad():
                outputs = self.model(**inputs)

                if self.pooling_strategy == "cls":
                    batch_embeddings = outputs.last_hidden_state[:, 0, :]
                elif self.pooling_strategy == "max":
                    batch_embeddings = outputs.last_hidden_state.max(dim=1)[0]
                else:  # mean
                    attention_mask = inputs["attention_mask"].unsqueeze(-1)
                    masked = outputs.last_hidden_state * attention_mask
                    summed = masked.sum(dim=1)
                    counts = attention_mask.sum(dim=1)
                    batch_embeddings = summed / counts

                if self.normalize:
                    batch_embeddings = F.normalize(batch_embeddings, p=2, dim=1)

                embeddings.append(batch_embeddings.cpu().numpy())

        return np.vstack(embeddings)

    def embed_documents(
        self, documents: list[dict[str, Any]], text_field: str = "text"
    ) -> np.ndarray:
        """Generate embeddings for documents, extracting ``text_field``."""
        texts = [doc.get(text_field, "") for doc in documents]
        return self.embed_texts(texts)

    def get_dimension(self) -> int:
        """Get embedding dimension."""
        return self.model_config["dimension"]

    def get_model_info(self) -> dict[str, Any]:
        """Get detailed model information."""
        return {
            "provider": "MultiBERT",
            "model": self.model_name,
            "model_path": self.model_config["name"],
            "dimension": self.model_config["dimension"],
            "size": self.model_config["size"].value,
            "speed": self.model_config["speed"],
            "memory": self.model_config["memory"],
            "device": self.device,
            "description": self.model_config["description"],
        }

    def benchmark(self, test_texts: list[str] = None) -> dict[str, float]:
        """Benchmark model throughput on a small text set."""
        import time

        if test_texts is None:
            test_texts = [
                "This is a test sentence for benchmarking.",
                "The quick brown fox jumps over the lazy dog.",
                "Machine learning models require evaluation.",
            ] * 10

        _ = self.embed_texts(test_texts[:3])  # warmup

        start = time.time()
        embeddings = self.embed_texts(test_texts)
        elapsed = time.time() - start

        return {
            "total_time": elapsed,
            "texts_per_second": len(test_texts) / elapsed,
            "ms_per_text": (elapsed * 1000) / len(test_texts),
            "batch_size": self.batch_size,
            "dimension": embeddings.shape[1],
            "device": self.device,
        }

    @classmethod
    def compare_models(cls, texts: list[str], models: list[str] = None):
        """Compare different models on the same texts (returns a DataFrame)."""
        import time

        import pandas as pd
        import torch

        if models is None:
            models = ["minilm-l6", "distilbert", "mpnet-base", "e5-large"]

        results = []
        for model_name in models:
            try:
                logger.info("Testing %s...", model_name)
                provider = cls(model_name=model_name)

                start = time.time()
                _ = provider.embed_texts(texts)
                elapsed = time.time() - start

                info = provider.get_model_info()
                results.append(
                    {
                        "model": model_name,
                        "dimension": info["dimension"],
                        "size": info["size"],
                        "memory": info["memory"],
                        "speed": info["speed"],
                        "time_seconds": elapsed,
                        "texts_per_sec": len(texts) / elapsed,
                        "device": info["device"],
                    }
                )

                if hasattr(provider, "model"):
                    del provider.model
                if torch.cuda.is_available():
                    torch.cuda.empty_cache()
            except Exception as e:
                logger.error("Failed to test %s: %s", model_name, e)

        return pd.DataFrame(results)


@ProviderRegistry.register(
    name="adaptive-bert",
    models=_models_to_metadata(
        {
            "mpnet-base": {
                "name": "sentence-transformers/all-mpnet-base-v2",
                "dimension": 768,
                "size": ModelSize.BASE,
                "memory": "~420MB",
                "description": "Adaptive default (auto-selected at runtime)",
                "is_sentence_transformer": True,
            },
        }
    ),
    aliases=["adaptive"],
    description="Adaptive BERT provider with runtime model selection",
)
class AdaptiveBERTProvider(MultiBERTProvider):
    """
    Adaptive BERT provider that selects a model based on input characteristics.

    Features:
    - Automatic model selection based on text length
    - Speed/accuracy preference knobs
    - Fallback to a smaller model on GPU OOM
    """

    def __init__(
        self,
        config: ProviderConfig | None = None,
        *,
        prefer_speed: bool = False,
        prefer_accuracy: bool = False,
        max_memory_gb: float | None = None,
        **kwargs,
    ):
        self.prefer_speed = prefer_speed
        self.prefer_accuracy = prefer_accuracy

        if config is not None:
            super().__init__(config=config)
            self.performance_stats = {
                "total_texts": 0,
                "total_time": 0,
                "model_switches": 0,
            }
            return

        if prefer_speed:
            model_name = "minilm-l12"
        elif prefer_accuracy:
            model_name = "e5-large"
        else:
            model_name = None

        super().__init__(model_name=model_name, max_memory_gb=max_memory_gb, **kwargs)
        self.performance_stats = {
            "total_texts": 0,
            "total_time": 0,
            "model_switches": 0,
        }

    def embed_texts(self, texts: list[str]) -> np.ndarray:
        """Adaptively embed texts with optional model switching + OOM fallback."""
        import torch

        avg_length = np.mean([len(t) for t in texts]) if texts else 0

        if self.prefer_speed and avg_length > 1000 and self.model_name != "minilm-l6":
            logger.info("Switching to mini model for long texts")
            self._switch_model("minilm-l6")
        elif (
            self.prefer_accuracy and avg_length < 200 and self.model_name != "e5-large"
        ):
            logger.info("Switching to large model for short texts")
            self._switch_model("e5-large")

        try:
            embeddings = super().embed_texts(texts)
        except torch.cuda.OutOfMemoryError:
            logger.warning("GPU OOM, falling back to smaller model")
            self._switch_model("minilm-l6")
            if torch.cuda.is_available():
                torch.cuda.empty_cache()
            embeddings = super().embed_texts(texts)

        self.performance_stats["total_texts"] += len(texts)
        return embeddings

    def _switch_model(self, new_model: str) -> None:
        """Switch to a different model in-place."""
        import torch

        if new_model == self.model_name:
            return

        logger.info("Switching from %s to %s", self.model_name, new_model)
        if hasattr(self, "model"):
            del self.model
        if getattr(self, "tokenizer", None) is not None:
            del self.tokenizer
        if torch.cuda.is_available():
            torch.cuda.empty_cache()

        self.model_name = new_model
        self.model_config = self.MODELS[new_model]
        # Re-point config.model so get_dimension() / metadata reflect the switch.
        self.config = self.config.merge(model=self._spec_to_metadata(new_model))
        self._initialized = False
        self._model = None
        self._cache = {}
        self.ensure_initialized()
        self.performance_stats["model_switches"] += 1
