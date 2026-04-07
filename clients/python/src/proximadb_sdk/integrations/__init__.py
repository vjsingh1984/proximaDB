"""ProximaDB integrations with third-party frameworks.

Submodules are intentionally loaded lazily so optional dependencies for one
integration do not break imports for unrelated integrations.
"""

__all__ = [
    "autogen",
    "crewai",
    "dspy",
    "haystack",
    "langchain",
    "langgraph",
    "llama_index",
    "victor",
    "victor_multi",
]
