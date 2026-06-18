"""
Test embedding utilities.

Prefers real sentence-transformers ("all-MiniLM-L6-v2", 384D) for realistic
vectors, but loads the model **lazily** and falls back to deterministic
pseudo-embeddings when it is unavailable (offline, HuggingFace rate-limited /
429, or the dependency is missing).

Why: importing this module must never trigger a network download — otherwise a
single rate-limited HF request aborts pytest *collection* and takes the whole
unit suite down (3 collection errors observed in CI). The non-semantic unit
tests (WAL, batching, quantization, ...) only need stable, correctly-shaped
vectors, which the fallback provides deterministically. Tests that genuinely
need semantic vectors can gate on ``using_real_embeddings()``.

Set ``PROXIMADB_TEST_FORCE_FALLBACK_EMBEDDINGS=1`` to force the deterministic
path (used to verify the offline behavior).
"""

from __future__ import annotations

import hashlib
import math
import os

# all-MiniLM-L6-v2 output dimension. Kept as a constant so importing this module
# never needs to instantiate the model.
_BASE_DIM = 384

_model = None
_model_load_attempted = False
_model_available = False

# Simple in-memory cache: {(text, dim): vector}
_CACHE: dict[tuple[str, int], list[float]] = {}

_SEED_TEXTS = [
    "machine learning for data analysis",
    "vector database similarity search",
    "natural language understanding systems",
    "computer vision and image processing",
    "deep learning neural networks",
    "recommendation systems and personalization",
    "dimensionality reduction techniques",
    "knowledge graphs and entity relations",
    "semantic search and embeddings",
    "efficient retrieval with typed filters",
]


def _get_model():
    """Lazily load the real model once. Returns the model, or None if unavailable.

    Any failure (missing dependency, offline, HF 429) is swallowed and recorded
    so callers transparently fall back to deterministic embeddings.
    """
    global _model, _model_load_attempted, _model_available
    if _model_load_attempted:
        return _model
    _model_load_attempted = True
    if os.environ.get("PROXIMADB_TEST_FORCE_FALLBACK_EMBEDDINGS") == "1":
        return None
    try:
        from sentence_transformers import SentenceTransformer

        _model = SentenceTransformer("all-MiniLM-L6-v2")  # 384D
        _model_available = True
    except Exception:
        _model = None
        _model_available = False
    return _model


def using_real_embeddings() -> bool:
    """True if the real sentence-transformers model is loaded (not the fallback)."""
    _get_model()
    return _model_available


def _fallback_embedding(text: str, dim: int) -> list[float]:
    """Deterministic, L2-normalized pseudo-embedding seeded by the text.

    Not semantically meaningful — only stable and correctly shaped, which is all
    the non-semantic tests require.
    """
    vec: list[float] = []
    counter = 0
    while len(vec) < dim:
        digest = hashlib.sha256(f"{text}:{counter}".encode()).digest()
        for i in range(0, len(digest), 4):
            if len(vec) >= dim:
                break
            n = int.from_bytes(digest[i : i + 4], "big")
            vec.append((n / 0xFFFFFFFF) * 2.0 - 1.0)
        counter += 1
    norm = math.sqrt(sum(x * x for x in vec)) or 1.0
    return [x / norm for x in vec]


def _base_encode(text: str) -> list[float]:
    """Encode at the model's base dimension, real model or deterministic fallback."""
    model = _get_model()
    if model is not None:
        return model.encode([text], convert_to_tensor=False)[0].tolist()
    return _fallback_embedding(text, _BASE_DIM)


def _adjust_dim(vec: list[float], dim: int) -> list[float]:
    if len(vec) == dim:
        return vec
    if len(vec) > dim:
        return vec[:dim]
    # pad with zeros
    return vec + [0.0] * (dim - len(vec))


def embed_text(text: str, dim: int) -> list[float]:
    """Encode text and adjust to requested dimension by truncate/pad with caching."""
    key = (text, dim)
    if key in _CACHE:
        return _CACHE[key]
    # Cache base encoding too
    base_key = (text, _BASE_DIM)
    if base_key in _CACHE:
        base = _CACHE[base_key]
    else:
        base = _base_encode(text)
        _CACHE[base_key] = base
    vec = _adjust_dim(base, dim)
    _CACHE[key] = vec
    return vec


def embed_seed(index: int, dim: int) -> list[float]:
    """Use a deterministic seed text based on index for stable tests."""
    text = _SEED_TEXTS[index % len(_SEED_TEXTS)]
    return embed_text(text, dim)


def embed_many(count: int, dim: int) -> list[list[float]]:
    return [embed_seed(i, dim) for i in range(count)]


def warm_cache(dims: list[int] = None) -> None:
    """Precompute and cache embeddings for seed texts at common dimensions."""
    if dims is None:
        dims = [32, 64, 128, 256, 384, 512, 1536]
    for text in _SEED_TEXTS:
        # Warm base dim
        _ = embed_text(text, _BASE_DIM)
        for d in dims:
            _ = embed_text(text, d)
