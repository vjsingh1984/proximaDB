"""ProximaDB integrations with third-party frameworks.

Submodules are intentionally loaded lazily so optional dependencies for one
integration do not break imports for unrelated integrations.
"""

__all__ = [
    "autogen",
    "agentic_store",
    "agentic_io",
    "agentic_ddl",
    "mlops",
    "crewai",
    "dspy",
    "dual_use_store",
    "graph_walk_client",
    "haystack",
    "langchain",
    "langgraph",
    "llama_index",
    "mcp_tools",
    "victor",
    "victor_embedded",
    "victor_graph",
    "victor_multi",
]
