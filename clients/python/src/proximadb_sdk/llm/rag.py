# Copyright 2025 ProximaDB
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.

"""RAG (Retrieval-Augmented Generation) Pipeline using ProximaDB."""

import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, TYPE_CHECKING

from proximadb_sdk.llm.config import EmbeddingConfig, LLMConfig, RAGConfig
from proximadb_sdk.llm.embedding import EmbeddingService
from proximadb_sdk.llm.semantic_cache import SemanticCache

if TYPE_CHECKING:
    from proximadb_sdk import ProximaDBClient


@dataclass
class Document:
    """Document for RAG indexing.

    Attributes:
        id: Unique document ID
        title: Document title
        content: Document content
        source: Source location (file path, URL)
        metadata: Additional metadata
    """

    id: str
    title: str
    content: str
    source: str
    metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass
class Source:
    """Source document in RAG response.

    Attributes:
        id: Document ID
        title: Document title
        url: Source URL/path
        relevance: Relevance score (0-1)
        snippet: Content snippet
    """

    id: str
    title: str
    url: str
    relevance: float
    snippet: str


@dataclass
class RAGResponse:
    """RAG query response.

    Attributes:
        answer: Generated answer
        sources: Source documents used
        confidence: Confidence score (0-1)
        latency_ms: Total latency in milliseconds
        retrieval_latency_ms: Retrieval latency
        generation_latency_ms: Generation latency
        tokens_used: Tokens used in generation
        cached: Whether response was from cache
    """

    answer: str
    sources: List[Source]
    confidence: float
    latency_ms: int
    retrieval_latency_ms: int
    generation_latency_ms: int
    tokens_used: int
    cached: bool


class RAGPipeline:
    """RAG Pipeline using ProximaDB for document storage.

    Provides document indexing, retrieval, and LLM generation with:
    - ProximaDB vector storage for embeddings
    - Semantic caching for repeated queries
    - Flexible LLM provider support via Victor

    Usage:
        from proximadb_sdk import ProximaDBClient
        from proximadb_sdk.llm import RAGPipeline, Document

        client = ProximaDBClient(url="http://localhost:5678")
        rag = RAGPipeline(client)
        await rag.initialize()

        # Index documents
        docs = [Document(id="1", title="Doc", content="...", source="file.txt")]
        await rag.index_documents("knowledge", docs)

        # Query
        response = await rag.query("What is...?", "knowledge")
        print(response.answer)
    """

    def __init__(
        self,
        client: "ProximaDBClient",
        config: Optional[LLMConfig] = None,
    ):
        """Initialize RAG pipeline.

        Args:
            client: ProximaDB client
            config: LLM configuration (optional)
        """
        self.client = client
        self.config = config or LLMConfig()
        self.embedding_service = EmbeddingService(self.config.embedding)
        self.semantic_cache = SemanticCache(self.config.cache, client)
        self._initialized = False

    async def initialize(self) -> None:
        """Initialize the RAG pipeline."""
        if self._initialized:
            return

        await self.embedding_service.initialize()
        await self.semantic_cache.initialize()
        self._initialized = True

    async def index_documents(
        self,
        collection: str,
        documents: List[Document],
        create_collection: bool = True,
    ) -> int:
        """Index documents into a collection.

        Args:
            collection: Collection name
            documents: List of documents to index
            create_collection: Whether to create collection if it doesn't exist

        Returns:
            Number of documents indexed
        """
        if not self._initialized:
            await self.initialize()

        if not documents:
            return 0

        # Create collection if needed
        if create_collection:
            try:
                await self.client.create_collection_async(
                    name=collection,
                    dimension=self.embedding_service.dimension,
                    distance_metric="cosine",
                )
            except Exception:
                # Collection may already exist
                pass

        # Generate embeddings in batches
        batch_size = self.config.embedding.batch_size
        total_indexed = 0

        for i in range(0, len(documents), batch_size):
            batch = documents[i : i + batch_size]
            contents = [doc.content for doc in batch]
            embeddings = await self.embedding_service.embed_batch(contents)

            # Prepare vectors for insertion
            vectors = []
            for doc, embedding in zip(batch, embeddings):
                vectors.append(
                    {
                        "id": doc.id,
                        "vector": embedding,
                        "metadata": {
                            "title": doc.title,
                            "content": doc.content[:1000],  # Truncate for metadata
                            "source": doc.source,
                            **doc.metadata,
                        },
                    }
                )

            # Insert into ProximaDB
            await self.client.insert_vectors_async(collection, vectors)
            total_indexed += len(batch)

        return total_indexed

    async def query(
        self,
        question: str,
        collection: str,
        top_k: Optional[int] = None,
        filter_metadata: Optional[Dict[str, Any]] = None,
        system_prompt: Optional[str] = None,
        skip_cache: bool = False,
    ) -> RAGResponse:
        """Execute a RAG query.

        Args:
            question: Question to answer
            collection: Collection to search
            top_k: Number of documents to retrieve
            filter_metadata: Optional metadata filters
            system_prompt: Custom system prompt
            skip_cache: Whether to skip semantic cache

        Returns:
            RAG response with answer and sources
        """
        if not self._initialized:
            await self.initialize()

        start_time = time.time()
        top_k = top_k or self.config.rag.retrieval_top_k

        # Check semantic cache
        if not skip_cache and self.config.cache.enabled:
            cached = await self.semantic_cache.lookup(question, collection)
            if cached:
                return RAGResponse(
                    answer=cached.response.answer,
                    sources=cached.response.sources,
                    confidence=cached.response.confidence,
                    latency_ms=int((time.time() - start_time) * 1000),
                    retrieval_latency_ms=0,
                    generation_latency_ms=0,
                    tokens_used=0,
                    cached=True,
                )

        # Generate question embedding
        retrieval_start = time.time()
        question_embedding = await self.embedding_service.embed_text(question)

        # Search for relevant documents
        search_results = await self.client.search_async(
            collection=collection,
            vector=question_embedding,
            top_k=top_k,
            filter=filter_metadata,
        )

        retrieval_latency = int((time.time() - retrieval_start) * 1000)

        # Build context from results
        sources = []
        context_parts = []

        for i, result in enumerate(search_results[: self.config.rag.context_top_k]):
            metadata = result.get("metadata", {})
            content = metadata.get("content", "")

            source = Source(
                id=result.get("id", f"doc_{i}"),
                title=metadata.get("title", "Untitled"),
                url=metadata.get("source", ""),
                relevance=1.0 - result.get("distance", 0.0),
                snippet=content[:500],
            )
            sources.append(source)
            context_parts.append(f"[{i + 1}] {content}")

        context = "\n\n".join(context_parts)

        # Generate answer using LLM
        generation_start = time.time()
        answer, tokens_used = await self._generate_answer(
            question=question,
            context=context,
            system_prompt=system_prompt,
        )
        generation_latency = int((time.time() - generation_start) * 1000)

        # Calculate confidence
        confidence = self._calculate_confidence(sources)

        response = RAGResponse(
            answer=answer,
            sources=sources,
            confidence=confidence,
            latency_ms=int((time.time() - start_time) * 1000),
            retrieval_latency_ms=retrieval_latency,
            generation_latency_ms=generation_latency,
            tokens_used=tokens_used,
            cached=False,
        )

        # Cache response
        if not skip_cache and self.config.cache.enabled:
            await self.semantic_cache.store(
                question=question,
                collection=collection,
                embedding=question_embedding,
                response=response,
            )

        return response

    async def _generate_answer(
        self,
        question: str,
        context: str,
        system_prompt: Optional[str] = None,
    ) -> tuple[str, int]:
        """Generate answer using LLM provider.

        Args:
            question: Question to answer
            context: Context from retrieved documents
            system_prompt: Custom system prompt

        Returns:
            Tuple of (answer, tokens_used)
        """
        default_system = (
            "Answer the question based on the provided context. "
            "If the answer cannot be found in the context, say so clearly. "
            "Cite relevant sources by their numbers [1], [2], etc."
        )

        prompt = f"""{system_prompt or default_system}

Context:
{context}

Question: {question}

Answer:"""

        try:
            # Try to use Victor's LLM providers
            from victor.providers.registry import ProviderRegistry

            registry = ProviderRegistry()
            provider = registry.get_provider(self.config.rag.llm_provider)

            response = await provider.chat(
                messages=[{"role": "user", "content": prompt}],
                model=self.config.rag.llm_model,
                temperature=self.config.rag.temperature,
                max_tokens=self.config.rag.max_tokens,
            )

            return response.content, response.usage.total_tokens

        except ImportError:
            # Fall back to simple prompt-based answer
            return self._fallback_answer(question, context), 0

    def _fallback_answer(self, question: str, context: str) -> str:
        """Generate a simple answer when no LLM is available.

        This provides basic extractive QA without an LLM.
        """
        # Simple extractive approach: find most relevant sentence
        sentences = context.split(". ")
        question_words = set(question.lower().split())

        best_sentence = ""
        best_score = 0

        for sentence in sentences:
            sentence_words = set(sentence.lower().split())
            overlap = len(question_words & sentence_words)
            if overlap > best_score:
                best_score = overlap
                best_sentence = sentence

        if best_sentence:
            return f"Based on the context: {best_sentence}."
        return "I couldn't find a relevant answer in the provided context."

    def _calculate_confidence(self, sources: List[Source]) -> float:
        """Calculate confidence score based on sources."""
        if not sources:
            return 0.0

        # Average relevance of top sources
        top_relevances = [s.relevance for s in sources[:3]]
        return sum(top_relevances) / len(top_relevances)

    async def delete_documents(
        self,
        collection: str,
        document_ids: List[str],
    ) -> int:
        """Delete documents from collection.

        Args:
            collection: Collection name
            document_ids: IDs of documents to delete

        Returns:
            Number of documents deleted
        """
        deleted = 0
        for doc_id in document_ids:
            try:
                await self.client.delete_vector_async(collection, doc_id)
                deleted += 1
            except Exception:
                pass
        return deleted

    async def clear_collection(self, collection: str) -> None:
        """Clear all documents from a collection.

        Args:
            collection: Collection name to clear
        """
        try:
            await self.client.delete_collection_async(collection)
        except Exception:
            pass

    async def close(self) -> None:
        """Clean up resources."""
        await self.embedding_service.close()
        self._initialized = False
