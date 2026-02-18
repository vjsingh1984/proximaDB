"""LangGraph retriever integration for ProximaDB.

LangGraph reuses LangChain's ``VectorStore`` interface. This module provides
a thin helper that creates a LangGraph-compatible retriever tool from the
existing ``ProximaDBVectorStore``.

Requires: ``pip install proximadb-python[langgraph]``

Example::

    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.integrations.langgraph import create_retriever_tool
    from langchain_openai import OpenAIEmbeddings

    client = ProximaDBClient(url="http://localhost:5678")
    tool = create_retriever_tool(
        client=client,
        collection_name="docs",
        embedding=OpenAIEmbeddings(),
        name="search_docs",
        description="Search the documentation for relevant information.",
    )
    # Use in a LangGraph StateGraph node or ToolNode
"""

from __future__ import annotations

from typing import Any

from langchain_core.embeddings import Embeddings
from langchain_core.tools import BaseTool
from langchain_core.tools import create_retriever_tool as _lc_create_retriever_tool

from proximadb_sdk.integrations.langchain import ProximaDBVectorStore


def create_retriever_tool(
    client: Any,
    collection_name: str,
    embedding: Embeddings,
    *,
    k: int = 4,
    name: str = "proximadb_retriever",
    description: str = "Search ProximaDB for relevant documents.",
    text_key: str = "text",
    **kwargs: Any,
) -> BaseTool:
    """Create a LangGraph-compatible retriever tool backed by ProximaDB.

    This wraps ``ProximaDBVectorStore.as_retriever()`` with LangChain's
    ``create_retriever_tool``, producing a tool that fits directly into
    LangGraph's ``ToolNode``.

    Args:
        client: A ``ProximaDBClient`` instance.
        collection_name: Name of the ProximaDB collection.
        embedding: LangChain ``Embeddings`` implementation.
        k: Number of documents to retrieve per query.
        name: Tool name visible to the LLM agent.
        description: Tool description visible to the LLM agent.
        text_key: Metadata key used to store original text.
        **kwargs: Extra keyword arguments forwarded to ``as_retriever``.

    Returns:
        A LangChain ``BaseTool`` usable in LangGraph graphs.
    """
    store = ProximaDBVectorStore(
        client=client,
        collection_name=collection_name,
        embedding=embedding,
        text_key=text_key,
    )
    retriever = store.as_retriever(search_kwargs={"k": k, **kwargs})
    return _lc_create_retriever_tool(
        retriever,
        name=name,
        description=description,
    )
