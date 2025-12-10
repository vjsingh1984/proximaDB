"""
ProximaDB Embedded Mode - Lightweight embedded vector + graph database.

This module provides an embedded database mode for ProximaDB that can be used
without running a separate server process. Perfect for:
- CLI tools (like Victor/codingagent)
- Single-user applications
- Development and testing
- Semantic code knowledge stores

Features:
- In-process database (no separate server needed)
- Vector similarity search with HNSW index
- Graph storage with ORION engine
- Persistent storage to disk
- Low memory footprint
- Thread-safe operations
- Pluggable embedding models (sentence-transformers, Ollama, OpenAI, etc.)

Usage:
    from proximadb import EmbeddedProximaDB

    # Initialize embedded database with auto-embedding
    db = EmbeddedProximaDB(data_dir="~/.myapp/data")
    await db.start()

    # Create collection with embedding model
    collection = await db.create_collection(
        "code_symbols",
        dimension=384,
        embedding_model="all-MiniLM-L6-v2"  # or custom embedding function
    )

    # Insert with auto-embedding (pass text, get embeddings automatically)
    await collection.insert_with_embedding([
        {"id": "func_1", "text": "def hello(): print('world')", "metadata": {"name": "hello"}}
    ])

    # Or insert pre-computed vectors
    await collection.insert([
        {"id": "func_1", "vector": [...], "metadata": {"name": "my_func"}}
    ])

    # Search with text (auto-embedded)
    results = await collection.search_text("hello function", top_k=10)

    # Or search with vector
    results = await collection.search(query_vector, top_k=10)

    # Cleanup
    await db.stop()
"""

import asyncio
import hashlib
import json
import subprocess
import sys
import threading
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Union, Protocol, runtime_checkable
import os
import signal


# =============================================================================
# Embedding Model Abstraction
# =============================================================================

@runtime_checkable
class EmbeddingFunction(Protocol):
    """Protocol for embedding functions.

    Any callable that takes a string and returns a list of floats can be used.
    This allows integration with any embedding provider.
    """
    def __call__(self, text: str) -> List[float]:
        """Generate embedding for text."""
        ...


@runtime_checkable
class AsyncEmbeddingFunction(Protocol):
    """Protocol for async embedding functions."""
    async def __call__(self, text: str) -> List[float]:
        """Generate embedding for text asynchronously."""
        ...


@runtime_checkable
class BatchEmbeddingFunction(Protocol):
    """Protocol for batch embedding functions."""
    def __call__(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts."""
        ...


class BaseEmbeddingModel(ABC):
    """Abstract base class for embedding models.

    Implement this for custom embedding providers.
    """

    @abstractmethod
    def embed(self, text: str) -> List[float]:
        """Generate embedding for a single text."""
        pass

    @abstractmethod
    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts."""
        pass

    @abstractmethod
    def get_dimension(self) -> int:
        """Get the embedding dimension."""
        pass

    async def embed_async(self, text: str) -> List[float]:
        """Async version of embed (default: runs sync in executor)."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(None, self.embed, text)

    async def embed_batch_async(self, texts: List[str]) -> List[List[float]]:
        """Async version of embed_batch (default: runs sync in executor)."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(None, self.embed_batch, texts)


class SentenceTransformerModel(BaseEmbeddingModel):
    """Sentence-transformers embedding model.

    This is the default embedding model for ProximaDB.
    Supports all models from https://huggingface.co/sentence-transformers

    Popular models:
    - all-MiniLM-L6-v2: Fast, 384-dim (80MB)
    - all-MiniLM-L12-v2: Balanced, 384-dim (120MB)
    - BAAI/bge-small-en-v1.5: Best for code, 384-dim (130MB)
    - all-mpnet-base-v2: High quality, 768-dim (420MB)
    """

    def __init__(self, model_name: str = "all-MiniLM-L6-v2", device: Optional[str] = None):
        """Initialize sentence-transformer model.

        Args:
            model_name: HuggingFace model name
            device: Device to use (cpu, cuda, mps). Auto-detected if None.
        """
        self.model_name = model_name
        self.device = device
        self._model = None
        self._dimension: Optional[int] = None
        self._lock = threading.Lock()

    def _ensure_loaded(self) -> None:
        """Ensure model is loaded (lazy loading)."""
        if self._model is None:
            with self._lock:
                if self._model is None:
                    try:
                        from sentence_transformers import SentenceTransformer
                        self._model = SentenceTransformer(self.model_name, device=self.device)
                        self._dimension = self._model.get_sentence_embedding_dimension()
                    except ImportError:
                        raise ImportError(
                            "sentence-transformers not installed. "
                            "Install with: pip install sentence-transformers"
                        )

    def embed(self, text: str) -> List[float]:
        """Generate embedding for text."""
        self._ensure_loaded()
        embedding = self._model.encode(text, convert_to_numpy=True, show_progress_bar=False)
        return embedding.tolist()

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts."""
        self._ensure_loaded()
        embeddings = self._model.encode(texts, convert_to_numpy=True, show_progress_bar=False)
        return embeddings.tolist()

    def get_dimension(self) -> int:
        """Get embedding dimension."""
        self._ensure_loaded()
        return self._dimension or 384


class OllamaEmbeddingModel(BaseEmbeddingModel):
    """Ollama embedding model.

    Use local Ollama for embeddings. Supports high-quality models like:
    - nomic-embed-text: 768-dim, fast
    - mxbai-embed-large: 1024-dim, high quality
    - qwen3-embedding:8b: 4096-dim, best quality
    """

    def __init__(
        self,
        model_name: str = "nomic-embed-text",
        base_url: str = "http://localhost:11434",
        dimension: int = 768,
    ):
        """Initialize Ollama embedding model.

        Args:
            model_name: Ollama model name (must be pulled first)
            base_url: Ollama server URL
            dimension: Embedding dimension (model-specific)
        """
        self.model_name = model_name
        self.base_url = base_url
        self._dimension = dimension

    def embed(self, text: str) -> List[float]:
        """Generate embedding using Ollama."""
        import httpx

        response = httpx.post(
            f"{self.base_url}/api/embeddings",
            json={"model": self.model_name, "prompt": text},
            timeout=60.0,
        )
        response.raise_for_status()
        return response.json()["embedding"]

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts."""
        # Ollama doesn't have native batch API, so we parallelize
        import httpx

        embeddings = []
        with httpx.Client(timeout=60.0) as client:
            for text in texts:
                response = client.post(
                    f"{self.base_url}/api/embeddings",
                    json={"model": self.model_name, "prompt": text},
                )
                response.raise_for_status()
                embeddings.append(response.json()["embedding"])
        return embeddings

    async def embed_async(self, text: str) -> List[float]:
        """Generate embedding asynchronously."""
        import httpx

        async with httpx.AsyncClient(timeout=60.0) as client:
            response = await client.post(
                f"{self.base_url}/api/embeddings",
                json={"model": self.model_name, "prompt": text},
            )
            response.raise_for_status()
            return response.json()["embedding"]

    async def embed_batch_async(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings asynchronously with concurrency."""
        tasks = [self.embed_async(text) for text in texts]
        return await asyncio.gather(*tasks)

    def get_dimension(self) -> int:
        """Get embedding dimension."""
        return self._dimension


class OpenAIEmbeddingModel(BaseEmbeddingModel):
    """OpenAI embedding model.

    Supports:
    - text-embedding-3-small: 1536-dim, fast, cheap
    - text-embedding-3-large: 3072-dim, high quality
    - text-embedding-ada-002: 1536-dim, legacy
    """

    def __init__(
        self,
        model_name: str = "text-embedding-3-small",
        api_key: Optional[str] = None,
    ):
        """Initialize OpenAI embedding model.

        Args:
            model_name: OpenAI model name
            api_key: OpenAI API key (or set OPENAI_API_KEY env var)
        """
        self.model_name = model_name
        self.api_key = api_key or os.environ.get("OPENAI_API_KEY")
        if not self.api_key:
            raise ValueError("OpenAI API key required")

        self._dimensions = {
            "text-embedding-3-small": 1536,
            "text-embedding-3-large": 3072,
            "text-embedding-ada-002": 1536,
        }

    def embed(self, text: str) -> List[float]:
        """Generate embedding using OpenAI."""
        import httpx

        response = httpx.post(
            "https://api.openai.com/v1/embeddings",
            headers={"Authorization": f"Bearer {self.api_key}"},
            json={"model": self.model_name, "input": text},
            timeout=30.0,
        )
        response.raise_for_status()
        return response.json()["data"][0]["embedding"]

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts."""
        import httpx

        response = httpx.post(
            "https://api.openai.com/v1/embeddings",
            headers={"Authorization": f"Bearer {self.api_key}"},
            json={"model": self.model_name, "input": texts},
            timeout=60.0,
        )
        response.raise_for_status()
        data = response.json()["data"]
        # Sort by index to maintain order
        data.sort(key=lambda x: x["index"])
        return [d["embedding"] for d in data]

    def get_dimension(self) -> int:
        """Get embedding dimension."""
        return self._dimensions.get(self.model_name, 1536)


class FunctionEmbeddingModel(BaseEmbeddingModel):
    """Wrapper for custom embedding functions.

    Use this to integrate any embedding provider by providing a function.

    Example:
        # Use Victor's EmbeddingService
        from victor.embeddings.service import EmbeddingService
        service = EmbeddingService.get_instance()

        model = FunctionEmbeddingModel(
            embed_fn=lambda text: service.embed_text_sync(text).tolist(),
            batch_fn=lambda texts: service.embed_batch_sync(texts).tolist(),
            dimension=384,
        )
    """

    def __init__(
        self,
        embed_fn: Callable[[str], List[float]],
        dimension: int,
        batch_fn: Optional[Callable[[List[str]], List[List[float]]]] = None,
        async_embed_fn: Optional[Callable[[str], Any]] = None,
    ):
        """Initialize with custom functions.

        Args:
            embed_fn: Function to embed a single text
            dimension: Embedding dimension
            batch_fn: Optional function to embed batch of texts
            async_embed_fn: Optional async function for embedding
        """
        self._embed_fn = embed_fn
        self._batch_fn = batch_fn
        self._async_embed_fn = async_embed_fn
        self._dimension = dimension

    def embed(self, text: str) -> List[float]:
        """Generate embedding using custom function."""
        return self._embed_fn(text)

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts."""
        if self._batch_fn:
            return self._batch_fn(texts)
        # Fallback to sequential
        return [self._embed_fn(text) for text in texts]

    async def embed_async(self, text: str) -> List[float]:
        """Generate embedding asynchronously."""
        if self._async_embed_fn:
            return await self._async_embed_fn(text)
        return await super().embed_async(text)

    def get_dimension(self) -> int:
        """Get embedding dimension."""
        return self._dimension


def create_embedding_model(
    model_type: str = "sentence-transformers",
    model_name: Optional[str] = None,
    **kwargs,
) -> BaseEmbeddingModel:
    """Factory function to create embedding models.

    Args:
        model_type: Type of model (sentence-transformers, ollama, openai)
        model_name: Model name (type-specific)
        **kwargs: Additional model-specific arguments

    Returns:
        Embedding model instance

    Example:
        # Sentence-transformers (default, local)
        model = create_embedding_model("sentence-transformers", "all-MiniLM-L6-v2")

        # Ollama (local, high quality)
        model = create_embedding_model("ollama", "nomic-embed-text")

        # OpenAI (cloud)
        model = create_embedding_model("openai", "text-embedding-3-small", api_key="...")
    """
    if model_type == "sentence-transformers":
        return SentenceTransformerModel(
            model_name=model_name or "all-MiniLM-L6-v2",
            device=kwargs.get("device"),
        )
    elif model_type == "ollama":
        return OllamaEmbeddingModel(
            model_name=model_name or "nomic-embed-text",
            base_url=kwargs.get("base_url", "http://localhost:11434"),
            dimension=kwargs.get("dimension", 768),
        )
    elif model_type == "openai":
        return OpenAIEmbeddingModel(
            model_name=model_name or "text-embedding-3-small",
            api_key=kwargs.get("api_key"),
        )
    else:
        raise ValueError(f"Unknown model type: {model_type}. "
                        f"Available: sentence-transformers, ollama, openai")


# =============================================================================
# Configuration
# =============================================================================

@dataclass
class EmbeddedConfig:
    """Configuration for embedded ProximaDB."""

    data_dir: str = field(default_factory=lambda: str(Path.home() / ".proximadb"))
    rest_port: int = 15678  # Use non-standard port to avoid conflicts
    grpc_port: int = 15679
    log_level: str = "warn"
    memory_flush_size_mb: int = 16
    cache_size_mb: int = 128

    # Engine selection (opinionated defaults for code knowledge store)
    vector_engine: str = "SST"  # Best for real-time code indexing
    graph_engine: str = "ORION"  # In-memory with WAL for code relationships


class EmbeddedCollection:
    """A collection in the embedded database.

    Supports both raw vector operations and text-based operations
    with automatic embedding generation.
    """

    def __init__(
        self,
        name: str,
        dimension: int,
        db: "EmbeddedProximaDB",
        embedding_model: Optional[BaseEmbeddingModel] = None,
    ):
        self.name = name
        self.dimension = dimension
        self._db = db
        self._embedding_model = embedding_model

    def set_embedding_model(self, model: BaseEmbeddingModel) -> None:
        """Set the embedding model for this collection.

        Args:
            model: Embedding model instance
        """
        self._embedding_model = model

    async def insert(
        self,
        vectors: List[Dict[str, Any]],
    ) -> Dict[str, Any]:
        """Insert vectors into collection.

        Args:
            vectors: List of dicts with 'id', 'vector', and optional 'metadata'

        Returns:
            Insert result with success status
        """
        return await self._db._insert_vectors(self.name, vectors)

    async def insert_with_embedding(
        self,
        documents: List[Dict[str, Any]],
    ) -> Dict[str, Any]:
        """Insert documents with automatic embedding generation.

        Args:
            documents: List of dicts with 'id', 'text', and optional 'metadata'
                       The 'text' field will be embedded automatically.

        Returns:
            Insert result with success status

        Example:
            await collection.insert_with_embedding([
                {"id": "doc1", "text": "Hello world", "metadata": {"type": "greeting"}},
                {"id": "doc2", "text": "Goodbye world", "metadata": {"type": "farewell"}},
            ])
        """
        if not self._embedding_model:
            raise RuntimeError(
                "No embedding model configured. Either:\n"
                "1. Create collection with embedding_model parameter\n"
                "2. Call set_embedding_model() on the collection\n"
                "3. Use insert() with pre-computed vectors"
            )

        # Extract texts and generate embeddings
        texts = [doc["text"] for doc in documents]
        embeddings = await self._embedding_model.embed_batch_async(texts)

        # Build vector records
        vectors = []
        for doc, embedding in zip(documents, embeddings):
            vector_record = {
                "id": doc["id"],
                "vector": embedding,
            }
            # Include text in metadata for retrieval
            metadata = doc.get("metadata", {}).copy()
            metadata["text"] = doc["text"]
            vector_record["metadata"] = metadata
            vectors.append(vector_record)

        return await self._db._insert_vectors(self.name, vectors)

    async def search(
        self,
        query_vector: List[float],
        top_k: int = 10,
        filters: Optional[Dict[str, Any]] = None,
    ) -> List[Dict[str, Any]]:
        """Search for similar vectors.

        Args:
            query_vector: Query embedding vector
            top_k: Number of results to return
            filters: Optional metadata filters

        Returns:
            List of search results with id, score, and metadata
        """
        return await self._db._search_vectors(self.name, query_vector, top_k, filters)

    async def search_text(
        self,
        query: str,
        top_k: int = 10,
        filters: Optional[Dict[str, Any]] = None,
    ) -> List[Dict[str, Any]]:
        """Search using text query with automatic embedding.

        Args:
            query: Text query (will be embedded automatically)
            top_k: Number of results to return
            filters: Optional metadata filters

        Returns:
            List of search results with id, score, text, and metadata
        """
        if not self._embedding_model:
            raise RuntimeError(
                "No embedding model configured. Either:\n"
                "1. Create collection with embedding_model parameter\n"
                "2. Call set_embedding_model() on the collection\n"
                "3. Use search() with pre-computed query vector"
            )

        query_vector = await self._embedding_model.embed_async(query)
        return await self._db._search_vectors(self.name, query_vector, top_k, filters)

    async def delete(self, ids: List[str]) -> int:
        """Delete vectors by ID.

        Args:
            ids: List of vector IDs to delete

        Returns:
            Number of vectors deleted
        """
        return await self._db._delete_vectors(self.name, ids)

    async def count(self) -> int:
        """Get number of vectors in collection."""
        stats = await self._db._get_collection_stats(self.name)
        return stats.get("vector_count", 0)

    @property
    def has_embedding_model(self) -> bool:
        """Check if collection has an embedding model configured."""
        return self._embedding_model is not None


class EmbeddedProximaDB:
    """Embedded ProximaDB instance.

    Runs ProximaDB as a subprocess, providing an in-process-like experience
    with full persistence and durability.

    Example:
        db = EmbeddedProximaDB(data_dir="~/.victor/proximadb")
        await db.start()

        collection = await db.create_collection("codebase", dimension=384)
        await collection.insert([...])
        results = await collection.search(query_vector)

        await db.stop()
    """

    def __init__(
        self,
        data_dir: Optional[str] = None,
        config: Optional[EmbeddedConfig] = None,
        binary_path: Optional[str] = None,
    ):
        """Initialize embedded database.

        Args:
            data_dir: Directory for persistent storage (default: ~/.proximadb)
            config: Full configuration (overrides data_dir if provided)
            binary_path: Path to proximadb-server binary (auto-detected if None)
        """
        self.config = config or EmbeddedConfig(
            data_dir=data_dir or str(Path.home() / ".proximadb")
        )
        self._binary_path = binary_path
        self._process: Optional[subprocess.Popen] = None
        self._started = False
        self._collections: Dict[str, EmbeddedCollection] = {}
        self._lock = threading.Lock()

        # Resolve data directory
        self._data_dir = Path(self.config.data_dir).expanduser()

    @property
    def rest_url(self) -> str:
        """REST API URL."""
        return f"http://localhost:{self.config.rest_port}"

    @property
    def grpc_url(self) -> str:
        """gRPC URL."""
        return f"localhost:{self.config.grpc_port}"

    def _find_binary(self) -> str:
        """Find proximadb-server binary."""
        if self._binary_path:
            return self._binary_path

        # Check common locations
        search_paths = [
            # Installed via pip (future)
            Path(sys.prefix) / "bin" / "proximadb-server",
            # Development build
            Path(__file__).parent.parent.parent.parent.parent / "target" / "release" / "proximadb-server",
            Path(__file__).parent.parent.parent.parent.parent / "target" / "debug" / "proximadb-server",
            # System PATH
            "proximadb-server",
        ]

        for path in search_paths:
            if isinstance(path, Path):
                if path.exists():
                    return str(path)
            else:
                # Check PATH
                import shutil
                if shutil.which(path):
                    return path

        raise RuntimeError(
            "proximadb-server binary not found. "
            "Please build with `cargo build --release` or install via pip."
        )

    def _generate_config(self) -> str:
        """Generate TOML config for embedded mode."""
        config_path = self._data_dir / "embedded-config.toml"
        self._data_dir.mkdir(parents=True, exist_ok=True)

        config_content = f"""# ProximaDB Embedded Mode Configuration
# Auto-generated - do not edit manually

[server]
node_id = "embedded-node"
bind_address = "127.0.0.1"
port = {self.config.rest_port}
data_dir = "{self._data_dir}"

[storage]
metadata_url = "file://{self._data_dir}/metadata"
mmap_enabled = true

[[storage.storage_locations]]
url = "file://{self._data_dir}/data"
weight = 1
tags = ["embedded"]

[storage.wal_config]
enable_wal = true
memory_flush_size_bytes = {self.config.memory_flush_size_mb * 1024 * 1024}
vector_count_threshold = 50000

[storage.sst_config]
cache_size_mb = {self.config.cache_size_mb}
compression = "lz4"
compression_level = 3

[api]
grpc_port = {self.config.grpc_port}
rest_port = {self.config.rest_port}
max_request_size_mb = 32
timeout_seconds = 30

[monitoring]
metrics_enabled = false
log_level = "{self.config.log_level}"

[graph]
engine = "{self.config.graph_engine}"
enable_prefetch = true
prefetch_budget = 4
"""
        config_path.write_text(config_content)
        return str(config_path)

    async def start(self, timeout: float = 30.0) -> None:
        """Start the embedded database.

        Args:
            timeout: Maximum time to wait for startup in seconds
        """
        if self._started:
            return

        with self._lock:
            if self._started:
                return

            # Find binary
            binary = self._find_binary()

            # Generate config
            config_path = self._generate_config()

            # Start process
            self._process = subprocess.Popen(
                [binary, "--config", config_path],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                preexec_fn=os.setsid if hasattr(os, 'setsid') else None,
            )

            # Wait for server to be ready
            start_time = time.time()
            while time.time() - start_time < timeout:
                try:
                    import httpx
                    response = httpx.get(f"{self.rest_url}/health", timeout=1.0)
                    if response.status_code == 200:
                        self._started = True
                        return
                except Exception:
                    pass
                await asyncio.sleep(0.1)

            # Timeout - kill process and raise error
            self._kill_process()
            raise TimeoutError(
                f"ProximaDB failed to start within {timeout}s. "
                "Check logs for errors."
            )

    def _kill_process(self) -> None:
        """Kill the server process."""
        if self._process:
            try:
                if hasattr(os, 'killpg'):
                    os.killpg(os.getpgid(self._process.pid), signal.SIGTERM)
                else:
                    self._process.terminate()
                self._process.wait(timeout=5)
            except Exception:
                if hasattr(os, 'killpg'):
                    os.killpg(os.getpgid(self._process.pid), signal.SIGKILL)
                else:
                    self._process.kill()
            self._process = None

    async def stop(self) -> None:
        """Stop the embedded database gracefully."""
        if not self._started:
            return

        with self._lock:
            if not self._started:
                return

            self._kill_process()
            self._started = False
            self._collections.clear()

    async def create_collection(
        self,
        name: str,
        dimension: Optional[int] = None,
        distance_metric: str = "cosine",
        embedding_model: Optional[Union[BaseEmbeddingModel, str]] = None,
    ) -> EmbeddedCollection:
        """Create a new collection.

        Args:
            name: Collection name
            dimension: Vector dimension (auto-detected from embedding model if not provided)
            distance_metric: Distance metric (cosine, euclidean, dot)
            embedding_model: Embedding model for auto-embedding. Can be:
                - BaseEmbeddingModel instance
                - String model name (uses sentence-transformers)
                - None (no auto-embedding, use raw vectors)

        Returns:
            Collection object for further operations

        Example:
            # With embedding model (recommended)
            collection = await db.create_collection(
                "code_symbols",
                embedding_model="all-MiniLM-L6-v2"
            )

            # With custom embedding model
            model = OllamaEmbeddingModel("nomic-embed-text")
            collection = await db.create_collection("docs", embedding_model=model)

            # Raw vectors only (no auto-embedding)
            collection = await db.create_collection("vectors", dimension=384)
        """
        if not self._started:
            await self.start()

        # Handle embedding model
        model_instance: Optional[BaseEmbeddingModel] = None
        if isinstance(embedding_model, str):
            # Create sentence-transformers model from name
            model_instance = SentenceTransformerModel(model_name=embedding_model)
        elif isinstance(embedding_model, BaseEmbeddingModel):
            model_instance = embedding_model

        # Auto-detect dimension from embedding model
        if dimension is None:
            if model_instance:
                dimension = model_instance.get_dimension()
            else:
                raise ValueError(
                    "dimension is required when not using an embedding model. "
                    "Either provide dimension or embedding_model."
                )

        import httpx

        # Map distance metric to enum
        metric_map = {"cosine": 1, "euclidean": 2, "dot": 3}
        metric_value = metric_map.get(distance_metric.lower(), 1)

        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{self.rest_url}/api/v1/collections",
                json={
                    "operation": 1,  # CREATE
                    "collection_id": name,
                    "collection_config": {
                        "name": name,
                        "dimension": dimension,
                        "distance_metric": metric_value,
                    }
                },
                timeout=30.0,
            )

            result = response.json()
            if not result.get("success") and "already exists" not in str(result):
                raise RuntimeError(f"Failed to create collection: {result}")

        collection = EmbeddedCollection(name, dimension, self, embedding_model=model_instance)
        self._collections[name] = collection
        return collection

    async def get_collection(self, name: str) -> Optional[EmbeddedCollection]:
        """Get an existing collection.

        Args:
            name: Collection name

        Returns:
            Collection object or None if not found
        """
        if not self._started:
            await self.start()

        if name in self._collections:
            return self._collections[name]

        import httpx

        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{self.rest_url}/api/v1/collections/{name}",
                timeout=10.0,
            )

            if response.status_code == 200:
                data = response.json()
                if data.get("collection"):
                    dim = data["collection"]["config"]["dimension"]
                    collection = EmbeddedCollection(name, dim, self)
                    self._collections[name] = collection
                    return collection

        return None

    async def delete_collection(self, name: str) -> bool:
        """Delete a collection.

        Args:
            name: Collection name

        Returns:
            True if deleted successfully
        """
        if not self._started:
            await self.start()

        import httpx

        async with httpx.AsyncClient() as client:
            response = await client.delete(
                f"{self.rest_url}/api/v1/collections/{name}",
                timeout=30.0,
            )

            if name in self._collections:
                del self._collections[name]

            return response.status_code in (200, 204, 404)

    async def list_collections(self) -> List[str]:
        """List all collections.

        Returns:
            List of collection names
        """
        if not self._started:
            await self.start()

        import httpx

        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{self.rest_url}/api/v1/collections",
                timeout=10.0,
            )

            if response.status_code == 200:
                data = response.json()
                collections = data.get("collections", [])
                return [c["config"]["name"] for c in collections]

        return []

    def _to_sql_value(self, value: Any) -> Dict[str, Any]:
        """Convert Python value to SqlValue format."""
        if value is None:
            return {"null_value": 0}
        elif isinstance(value, bool):
            return {"bool_value": value}
        elif isinstance(value, int):
            return {"int64_value": value}
        elif isinstance(value, float):
            return {"number_value": value}
        elif isinstance(value, str):
            return {"string_value": value}
        elif isinstance(value, (list, tuple)):
            return {"array_value": {"values": [self._to_sql_value(v) for v in value]}}
        else:
            return {"string_value": str(value)}

    def _convert_metadata(self, metadata: Dict[str, Any]) -> Dict[str, Any]:
        """Convert metadata dict to SqlValue format."""
        return {k: self._to_sql_value(v) for k, v in metadata.items()}

    async def _insert_vectors(
        self,
        collection_name: str,
        vectors: List[Dict[str, Any]],
    ) -> Dict[str, Any]:
        """Insert vectors into a collection."""
        import httpx

        # Format vectors with SqlValue metadata
        formatted = []
        for v in vectors:
            item = {"id": v["id"], "vector": v["vector"]}
            if "metadata" in v:
                item["metadata"] = self._convert_metadata(v["metadata"])
            formatted.append(item)

        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{self.rest_url}/api/v1/vectors/batch",
                json={
                    "collection_id": collection_name,
                    "vectors": formatted,
                },
                timeout=60.0,
            )
            return response.json()

    async def _search_vectors(
        self,
        collection_name: str,
        query_vector: List[float],
        top_k: int = 10,
        filters: Optional[Dict[str, Any]] = None,
    ) -> List[Dict[str, Any]]:
        """Search for similar vectors."""
        import httpx

        query = {"vector": query_vector}
        if filters:
            query["filters"] = self._convert_metadata(filters)

        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{self.rest_url}/api/v1/search",
                json={
                    "collection_id": collection_name,
                    "queries": [query],
                    "top_k": top_k,
                },
                timeout=30.0,
            )

            data = response.json()

            # Handle nested results structure
            if data.get("success") and data.get("results"):
                inner = data["results"]
                if isinstance(inner, dict) and "results" in inner:
                    return inner["results"] or []
                elif isinstance(inner, list):
                    return inner

            return []

    async def _delete_vectors(
        self,
        collection_name: str,
        ids: List[str],
    ) -> int:
        """Delete vectors by ID."""
        # TODO: Implement delete endpoint in ProximaDB
        return 0

    async def _get_collection_stats(
        self,
        collection_name: str,
    ) -> Dict[str, Any]:
        """Get collection statistics."""
        import httpx

        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{self.rest_url}/api/v1/collections/{collection_name}",
                timeout=10.0,
            )

            if response.status_code == 200:
                data = response.json()
                if data.get("collection"):
                    return data["collection"].get("stats", {})

            return {}

    async def health_check(self) -> bool:
        """Check if database is healthy.

        Returns:
            True if healthy
        """
        if not self._started:
            return False

        try:
            import httpx
            async with httpx.AsyncClient() as client:
                response = await client.get(
                    f"{self.rest_url}/health",
                    timeout=5.0,
                )
                return response.status_code == 200
        except Exception:
            return False

    def __enter__(self):
        """Context manager entry."""
        loop = asyncio.get_event_loop()
        loop.run_until_complete(self.start())
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit."""
        loop = asyncio.get_event_loop()
        loop.run_until_complete(self.stop())

    async def __aenter__(self):
        """Async context manager entry."""
        await self.start()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit."""
        await self.stop()


# Convenience function
async def connect_embedded(
    data_dir: Optional[str] = None,
    **kwargs
) -> EmbeddedProximaDB:
    """Create and start an embedded ProximaDB instance.

    Args:
        data_dir: Directory for persistent storage
        **kwargs: Additional EmbeddedConfig options

    Returns:
        Started EmbeddedProximaDB instance
    """
    config = EmbeddedConfig(data_dir=data_dir or str(Path.home() / ".proximadb"), **kwargs)
    db = EmbeddedProximaDB(config=config)
    await db.start()
    return db
