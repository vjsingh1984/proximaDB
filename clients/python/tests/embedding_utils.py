"""
Test embedding utilities using sentence-transformers for realistic vectors.

Falls back to a clear error if the dependency is missing. To install:
    pip install sentence-transformers
"""

try:
    from sentence_transformers import SentenceTransformer
except Exception as e:
    raise SystemExit(
        "sentence-transformers is required for test embeddings.\n"
        "Install: pip install sentence-transformers\n"
        f"Error: {e}"
    )

_MODEL = SentenceTransformer("all-MiniLM-L6-v2")  # 384D
_BASE_DIM = _MODEL.get_sentence_embedding_dimension()

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
        base = _MODEL.encode([text], convert_to_tensor=False)[0].tolist()
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
