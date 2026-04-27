"""ProximaDB integrations with third-party frameworks.

Submodules are intentionally loaded lazily so optional dependencies for one
integration do not break imports for unrelated integrations.
"""

__all__ = [
    "autogen",
    "crewai",
    "dspy",
    "graph_walk_client",
    "haystack",
    "langchain",
    "langgraph",
    "llama_index",
    "mcp_tools",
    "victor",
    "victor_multi",
]
