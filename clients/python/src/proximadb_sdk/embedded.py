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

    async def execute_multi_modal_query(
        self,
        query: "MultiModalQuery",
        rerank_config: Optional["RerankConfig"] = None,
        context: Optional["QueryContext"] = None,
    ) -> "MultiModalQueryResult":
        """Execute a multi-modal query combining vector, graph, and document searches.

        This method allows combining different query types (vector similarity,
        graph traversal, document filtering) into a single unified query with
        result fusion and optional reranking.

        Args:
            query: A MultiModalQuery object built with MultiModalQueryBuilder
            rerank_config: Optional configuration for cross-modal reranking
            context: Optional query context for context-aware scoring

        Returns:
            MultiModalQueryResult with fused results from all query components

        Example:
            from proximadb_sdk.multimodal_query import (
                MultiModalQueryBuilder, FusionStrategy, RerankConfig, QueryContext
            )

            # Build a multi-modal query
            query = (MultiModalQueryBuilder()
                .vector("products", query_embedding, top_k=20)
                .graph("knowledge", start_label="Category", edge_types=["CONTAINS"])
                .fuse(FusionStrategy.RRF)
                .limit(10)
                .build())

            # Execute with optional reranking
            results = await db.execute_multi_modal_query(
                query,
                rerank_config=RerankConfig(diversity_optimization=True)
            )

            for record in results:
                print(f"{record['id']}: {record.get('_rrf_score', 0)}")
        """
        if not self._started:
            await self.start()

        # Import here to avoid circular imports
        from .multimodal_query import (
            MultiModalQueryResult,
            CrossModalReranker,
            RerankConfig as RC,
            QueryContext as QC,
        )

        # Create embedded executor
        executor = EmbeddedMultiModalQueryExecutor(self)
        result = await executor.execute(query)

        # Apply reranking if configured
        if rerank_config is not None:
            reranker = CrossModalReranker(config=rerank_config)
            reranked = reranker.rerank(result.records, context)
            result = MultiModalQueryResult(
                records=reranked.records,
                total_count=len(reranked.records),
                query_time_ms=result.query_time_ms,
                component_times=result.component_times,
                fusion_strategy=result.fusion_strategy,
                metadata={
                    **result.metadata,
                    "reranked": True,
                    "quality_score": reranked.quality_score,
                    "diversity_score": reranked.diversity_score,
                },
            )

        return result

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


# =============================================================================
# Embedded Multi-Modal Query Executor
# =============================================================================


class EmbeddedMultiModalQueryExecutor:
    """
    Multi-modal query executor for embedded ProximaDB mode.

    Executes vector, graph, and document queries against an embedded database
    and fuses results using various strategies (RRF, intersection, union, weighted).

    This executor is designed to work with the subprocess-based embedded mode
    that communicates via REST API.
    """

    def __init__(self, db: EmbeddedProximaDB):
        """Initialize with an embedded database instance.

        Args:
            db: EmbeddedProximaDB instance (must be started)
        """
        self._db = db

    async def execute(self, query: "MultiModalQuery") -> "MultiModalQueryResult":
        """Execute a multi-modal query.

        Args:
            query: Compiled MultiModalQuery object

        Returns:
            MultiModalQueryResult with fused results
        """
        from .multimodal_query import MultiModalQueryResult, TimeDecayFunction
        import math

        start_time = time.time()
        component_times: Dict[str, float] = {}
        component_results: List[List[Dict[str, Any]]] = []

        # Execute each component
        for i, component in enumerate(query.components):
            comp_start = time.time()

            comp_type = component.get("type")
            if comp_type == "vector":
                results = await self._execute_vector(component)
            elif comp_type == "graph":
                # Check if this depends on previous results
                if component.get("_from_previous") and component_results:
                    prev_results = component_results[-1]
                    id_field = component.get("_id_field", "id")
                    start_nodes = [r.get(id_field) for r in prev_results if r.get(id_field)]
                    component["start_nodes"] = start_nodes
                results = await self._execute_graph(component)
            elif comp_type == "document":
                results = await self._execute_document(component)
            elif comp_type == "logs":
                results = await self._execute_logs(component)
            elif comp_type == "metrics":
                results = await self._execute_metrics(component)
            else:
                results = []

            component_results.append(results)
            component_times[f"{comp_type}_{i}"] = (time.time() - comp_start) * 1000

        # Apply joins if specified
        if query.joins:
            component_results = self._apply_joins(component_results, query.joins)

        # Fuse results
        fused = self._fuse_results(
            component_results,
            query.fusion_strategy,
            query.fusion_weights,
        )

        # Apply time decay if specified
        if query.time_decay:
            fused = self._apply_time_decay(fused, query.time_decay)

        # Apply custom scorer if specified
        if query.custom_scorer:
            for record in fused:
                record["_custom_score"] = query.custom_scorer(record)
            fused.sort(key=lambda x: x.get("_custom_score", 0), reverse=True)

        # Apply limit and offset
        fused = fused[query.offset:query.offset + query.limit]

        total_time = (time.time() - start_time) * 1000

        return MultiModalQueryResult(
            records=fused,
            total_count=len(fused),
            query_time_ms=total_time,
            component_times=component_times,
            fusion_strategy=query.fusion_strategy,
            metadata={
                "component_count": len(query.components),
                "join_count": len(query.joins),
                "executor": "embedded",
            },
        )

    async def _execute_vector(self, component: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Execute vector search component against embedded database."""
        try:
            collection = component.get("collection", "")
            query_vector = component.get("query_vector", [])
            top_k = component.get("top_k", 10)
            filters = component.get("filter")

            # Use the embedded DB's search method via REST API
            results = await self._db._search_vectors(
                collection,
                query_vector,
                top_k,
                filters,
            )

            # Transform results to standard format
            return [
                {
                    "id": r.get("id", r.get("vector_id", "")),
                    "score": r.get("score", r.get("similarity", 0.0)),
                    "metadata": r.get("metadata", {}),
                    "_source_type": "vector",
                }
                for r in results
            ]
        except Exception as e:
            # Log error but return empty results to allow other components to proceed
            return []

    async def _execute_graph(self, component: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Execute graph traversal component against embedded database.

        Performs graph traversal by:
        1. Getting start nodes (by label or explicit IDs)
        2. Traversing outgoing edges up to max_depth
        3. Filtering by edge types if specified
        """
        try:
            import httpx

            graph_id = component.get("graph_id", "")
            start_nodes = component.get("start_nodes")
            start_label = component.get("start_label")
            edge_types = component.get("edge_types")
            max_depth = component.get("max_depth", 2)
            limit = component.get("limit", 100)

            results: List[Dict[str, Any]] = []

            # Get start nodes
            node_ids: List[str] = []
            if start_nodes:
                node_ids = start_nodes
            elif start_label:
                # Query nodes by label via REST API
                async with httpx.AsyncClient() as client:
                    response = await client.post(
                        f"{self._db.rest_url}/api/v1/graphs/{graph_id}/nodes/query",
                        json={"labels": [start_label]},
                        timeout=30.0,
                    )
                    if response.status_code == 200:
                        data = response.json()
                        nodes = data.get("nodes", [])
                        node_ids = [n.get("id") for n in nodes if n.get("id")]

            if not node_ids:
                return []

            # BFS traversal
            visited = set()
            queue = [(nid, 0) for nid in node_ids[:limit]]  # (node_id, depth)

            async with httpx.AsyncClient() as client:
                while queue and len(results) < limit:
                    node_id, depth = queue.pop(0)

                    if node_id in visited:
                        continue
                    visited.add(node_id)

                    # Get node info
                    response = await client.get(
                        f"{self._db.rest_url}/api/v1/graphs/{graph_id}/nodes/{node_id}",
                        timeout=10.0,
                    )

                    if response.status_code == 200:
                        node_data = response.json().get("node", {})
                        results.append({
                            "id": node_id,
                            "node": node_data,
                            "depth": depth,
                            "labels": node_data.get("labels", []),
                            "properties": node_data.get("properties", {}),
                            "_source_type": "graph",
                        })

                        # Get outgoing edges for further traversal
                        if depth < max_depth:
                            edge_response = await client.get(
                                f"{self._db.rest_url}/api/v1/graphs/{graph_id}/nodes/{node_id}/edges/outgoing",
                                timeout=10.0,
                            )

                            if edge_response.status_code == 200:
                                edges = edge_response.json().get("edges", [])
                                for edge in edges:
                                    edge_type = edge.get("edge_type", "")
                                    if edge_types is None or edge_type in edge_types:
                                        target = edge.get("to_node_id")
                                        if target and target not in visited:
                                            queue.append((target, depth + 1))

            return results[:limit]

        except Exception as e:
            return []

    async def _execute_document(self, component: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Execute document query component against embedded database."""
        try:
            import httpx

            collection = component.get("collection", "")
            filter_expr = component.get("filter")
            text_query = component.get("text_query")
            limit = component.get("limit", 100)

            # Build filter expression
            filter_str = None
            if filter_expr:
                # Convert dict filter to string expression
                if isinstance(filter_expr, dict):
                    parts = []
                    for k, v in filter_expr.items():
                        if isinstance(v, str):
                            parts.append(f"$.{k} = '{v}'")
                        else:
                            parts.append(f"$.{k} = {v}")
                    filter_str = " AND ".join(parts)
                else:
                    filter_str = str(filter_expr)

            # Query documents via REST API
            async with httpx.AsyncClient() as client:
                response = await client.post(
                    f"{self._db.rest_url}/api/v1/documents/{collection}/query",
                    json={
                        "filter": filter_str,
                        "text_query": text_query,
                        "limit": limit,
                    },
                    timeout=30.0,
                )

                if response.status_code == 200:
                    data = response.json()
                    documents = data.get("documents", [])
                    return [
                        {
                            "id": doc.get("id", doc.get("_id", "")),
                            "document": doc,
                            "_source_type": "document",
                        }
                        for doc in documents
                    ]

            return []

        except Exception as e:
            return []

    async def _execute_logs(self, component: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Execute log query component.

        Note: Log queries are not yet fully implemented in embedded mode.
        Returns empty results for now.
        """
        # Log queries would require observability backend integration
        return []

    async def _execute_metrics(self, component: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Execute metric aggregation component.

        Note: Metric queries are not yet fully implemented in embedded mode.
        Returns empty results for now.
        """
        # Metric queries would require observability backend integration
        return []

    def _apply_joins(
        self,
        component_results: List[List[Dict[str, Any]]],
        joins: List[Dict[str, Any]],
    ) -> List[List[Dict[str, Any]]]:
        """Apply joins between component results."""
        if len(component_results) < 2:
            return component_results

        for join in joins:
            join_type = join.get("join_type", "inner")
            left_field = join.get("left_field", "id")
            right_field = join.get("right_field", "id")

            if join_type in ("inner", "semantic"):
                left_results = component_results[0]
                right_results = component_results[1]

                # Build index of right results
                right_index: Dict[str, Dict[str, Any]] = {}
                for r in right_results:
                    key = self._extract_field(r, right_field)
                    if key:
                        right_index[key] = r

                # Join
                joined: List[Dict[str, Any]] = []
                for left in left_results:
                    left_key = self._extract_field(left, left_field)
                    if left_key and left_key in right_index:
                        merged = {**left, **right_index[left_key]}
                        merged["_join_type"] = join_type
                        joined.append(merged)

                component_results = [joined] + component_results[2:]

        return component_results

    def _extract_field(self, record: Dict[str, Any], field_path: str) -> Optional[str]:
        """Extract a field value from a nested record."""
        parts = field_path.split(".")
        current: Any = record
        for part in parts:
            if isinstance(current, dict) and part in current:
                current = current[part]
            else:
                return None
        return str(current) if current is not None else None

    def _fuse_results(
        self,
        component_results: List[List[Dict[str, Any]]],
        strategy: str,
        weights: Dict[str, float],
    ) -> List[Dict[str, Any]]:
        """Fuse results from multiple components."""
        if not component_results:
            return []

        if len(component_results) == 1:
            return component_results[0]

        if strategy == "intersection":
            return self._fuse_intersection(component_results)
        elif strategy == "union":
            return self._fuse_union(component_results)
        elif strategy == "rrf":
            return self._fuse_rrf(component_results, weights)
        elif strategy == "weighted":
            return self._fuse_weighted(component_results, weights)
        else:
            # Default to RRF
            return self._fuse_rrf(component_results, weights)

    def _fuse_intersection(
        self,
        component_results: List[List[Dict[str, Any]]],
    ) -> List[Dict[str, Any]]:
        """Return only records present in all components."""
        if not component_results:
            return []

        # Get IDs from first component
        common_ids = set(r.get("id") for r in component_results[0] if r.get("id"))

        # Intersect with other components
        for results in component_results[1:]:
            ids = set(r.get("id") for r in results if r.get("id"))
            common_ids &= ids

        # Return records with common IDs, merging data from all components
        merged_records: Dict[str, Dict[str, Any]] = {}
        for results in component_results:
            for r in results:
                record_id = r.get("id")
                if record_id in common_ids:
                    if record_id not in merged_records:
                        merged_records[record_id] = r.copy()
                    else:
                        # Merge additional fields
                        for k, v in r.items():
                            if k not in merged_records[record_id]:
                                merged_records[record_id][k] = v

        return list(merged_records.values())

    def _fuse_union(
        self,
        component_results: List[List[Dict[str, Any]]],
    ) -> List[Dict[str, Any]]:
        """Return all records from any component (deduplicated)."""
        seen_ids: set = set()
        result: List[Dict[str, Any]] = []

        for results in component_results:
            for r in results:
                record_id = r.get("id")
                if record_id and record_id not in seen_ids:
                    seen_ids.add(record_id)
                    result.append(r)
                elif not record_id:
                    result.append(r)

        return result

    def _fuse_rrf(
        self,
        component_results: List[List[Dict[str, Any]]],
        weights: Dict[str, float],
        k: int = 60,
    ) -> List[Dict[str, Any]]:
        """Reciprocal Rank Fusion.

        RRF score = sum(weight_i / (k + rank_i)) for each component

        This is a robust fusion method that works well when different
        components have different score scales.
        """
        scores: Dict[str, float] = {}
        records: Dict[str, Dict[str, Any]] = {}

        for comp_idx, results in enumerate(component_results):
            weight = weights.get(f"component_{comp_idx}", 1.0)
            # Also check for type-based weights
            if results and results[0].get("_source_type"):
                source_type = results[0]["_source_type"]
                weight = weights.get(f"{source_type}_{comp_idx + 1}", weight)

            for rank, record in enumerate(results):
                record_id = record.get("id", f"_anon_{comp_idx}_{rank}")
                rrf_score = weight / (k + rank + 1)

                if record_id in scores:
                    scores[record_id] += rrf_score
                else:
                    scores[record_id] = rrf_score
                    records[record_id] = record

        # Sort by RRF score
        sorted_ids = sorted(scores.keys(), key=lambda x: scores[x], reverse=True)

        result: List[Dict[str, Any]] = []
        for record_id in sorted_ids:
            record = records[record_id].copy()
            record["_rrf_score"] = scores[record_id]
            result.append(record)

        return result

    def _fuse_weighted(
        self,
        component_results: List[List[Dict[str, Any]]],
        weights: Dict[str, float],
    ) -> List[Dict[str, Any]]:
        """Weighted score combination."""
        scores: Dict[str, float] = {}
        records: Dict[str, Dict[str, Any]] = {}

        for comp_idx, results in enumerate(component_results):
            weight = weights.get(f"component_{comp_idx}", 1.0)

            for record in results:
                record_id = record.get("id", id(record))
                component_score = record.get("score", 1.0) * weight

                if record_id in scores:
                    scores[record_id] += component_score
                else:
                    scores[record_id] = component_score
                    records[record_id] = record

        # Sort by weighted score
        sorted_ids = sorted(scores.keys(), key=lambda x: scores[x], reverse=True)

        result: List[Dict[str, Any]] = []
        for record_id in sorted_ids:
            record = records[record_id].copy()
            record["_weighted_score"] = scores[record_id]
            result.append(record)

        return result

    def _apply_time_decay(
        self,
        records: List[Dict[str, Any]],
        time_decay: tuple,
    ) -> List[Dict[str, Any]]:
        """Apply time decay to record scores."""
        import math

        func, params = time_decay
        reference_time = params.get("reference_time", int(time.time() * 1e9))
        halflife_ns = params.get("halflife_hours", 24) * 3600 * 1e9
        time_field = params.get("time_field", "timestamp")

        for record in records:
            timestamp = record.get(time_field)
            if timestamp is None:
                continue

            age_ns = reference_time - timestamp
            if age_ns < 0:
                age_ns = 0

            # Get function type (handle both enum and string)
            func_name = func.value if hasattr(func, 'value') else str(func)

            if func_name == "linear":
                decay = max(0, 1 - (age_ns / (halflife_ns * 2)))
            elif func_name == "exponential":
                decay = math.exp(-0.693 * age_ns / halflife_ns)  # ln(2) = 0.693
            elif func_name == "gaussian":
                decay = math.exp(-0.5 * (age_ns / halflife_ns) ** 2)
            else:
                decay = 1.0

            # Apply decay to existing score
            current_score = record.get("score", record.get("_rrf_score", 1.0))
            record["_decayed_score"] = current_score * decay
            record["_time_decay"] = decay

        # Re-sort by decayed score
        records.sort(key=lambda x: x.get("_decayed_score", 0), reverse=True)

        return records
