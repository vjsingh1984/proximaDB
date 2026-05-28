"""
Search Builder

Fluent interface for building complex search operations.
"""

from typing import Any

FilterDict = dict[str, Any]
from ..filters import FilterBuilder


class SearchBuilder:
    """
    Fluent interface for building complex search queries.

    Examples:
        # Simple similarity search
        search = (SearchBuilder(query_vector)
            .top_k(10)
            .include_metadata()
            .build())

        # Advanced search with filters
        search = (SearchBuilder(query_vector)
            .top_k(20)
            .include_vectors()
            .filter_by("category", "electronics")
            .filter_range("price", 100, 1000)
            .timeout(5000)
            .explain()
            .build())

        # Using FilterBuilder for complex conditions
        filter_builder = (FilterBuilder()
            .equals("category", "electronics")
            .or_()
            .equals("category", "books")
            .and_()
            .greater_than("rating", 4.0))

        search = (SearchBuilder(query_vector)
            .top_k(15)
            .filter(filter_builder)
            .build())
    """

    def __init__(self, query_vector: list[float]):
        """
        Initialize search builder with query vector

        Args:
            query_vector: The query vector for similarity search
        """
        self.query_vector = query_vector
        self._top_k = 10
        self._include_vectors = False
        self._include_metadata = True
        self._filter_dict: FilterDict | None = None
        self._explain = False
        self._use_index = True
        self._timeout_ms: int | None = None
        self._filters: list[dict[str, Any]] = []

    def top_k(self, k: int) -> "SearchBuilder":
        """Set number of results to return"""
        if k <= 0:
            raise ValueError("top_k must be positive")
        if k > 10000:
            raise ValueError("top_k cannot exceed 10000")
        self._top_k = k
        return self

    def include_vectors(self, include: bool = True) -> "SearchBuilder":
        """Include vector data in results"""
        self._include_vectors = include
        return self

    def include_metadata(self, include: bool = True) -> "SearchBuilder":
        """Include metadata in results"""
        self._include_metadata = include
        return self

    def filter(self, filter_builder: FilterBuilder) -> "SearchBuilder":
        """Apply complex filter using FilterBuilder"""
        self._filter_dict = filter_builder.to_dict()
        return self

    def filter_by(self, field: str, value: Any) -> "SearchBuilder":
        """Add simple equality filter"""
        if self._filter_dict is None:
            self._filter_dict = {"operator": "and", "conditions": []}

        condition = {"field": field, "operation": "equals", "value": value}
        self._filter_dict["conditions"].append(condition)
        return self

    def filter_range(
        self,
        field: str,
        min_value: int | float | None = None,
        max_value: int | float | None = None,
    ) -> "SearchBuilder":
        """Add range filter"""
        if min_value is None and max_value is None:
            raise ValueError("At least one of min_value or max_value must be specified")

        if self._filter_dict is None:
            self._filter_dict = {"operator": "and", "conditions": []}

        conditions = []
        if min_value is not None:
            conditions.append({"field": field, "operation": "gte", "value": min_value})
        if max_value is not None:
            conditions.append({"field": field, "operation": "lte", "value": max_value})

        self._filter_dict["conditions"].extend(conditions)
        return self

    def filter_in(self, field: str, values: list[Any]) -> "SearchBuilder":
        """Add IN filter for field matching any of the values"""
        if not values:
            raise ValueError("Values list cannot be empty")

        if self._filter_dict is None:
            self._filter_dict = {"operator": "and", "conditions": []}

        condition = {"field": field, "operation": "in", "value": values}
        self._filter_dict["conditions"].append(condition)
        return self

    def filter_exists(self, field: str) -> "SearchBuilder":
        """Add exists filter to check if field is present"""
        if self._filter_dict is None:
            self._filter_dict = {"operator": "and", "conditions": []}

        condition = {"field": field, "operation": "exists"}
        self._filter_dict["conditions"].append(condition)
        return self

    def explain(self, enable: bool = True) -> "SearchBuilder":
        """Enable query execution plan explanation"""
        self._explain = enable
        return self

    def use_index(self, enable: bool = True) -> "SearchBuilder":
        """Enable/disable index usage (for debugging)"""
        self._use_index = enable
        return self

    def timeout(self, timeout_ms: int) -> "SearchBuilder":
        """Set query timeout in milliseconds"""
        if timeout_ms <= 0:
            raise ValueError("Timeout must be positive")
        self._timeout_ms = timeout_ms
        return self

    def build(self) -> dict[str, Any]:
        """Build search options dictionary"""
        return {
            "top_k": self._top_k,
            "include_vectors": self._include_vectors,
            "include_metadata": self._include_metadata,
            "filter": self._filter_dict,
            "explain": self._explain,
            "use_index": self._use_index,
            "timeout_ms": self._timeout_ms,
        }

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary representation"""
        options = self.build()
        result = {
            "vector": self.query_vector,
            "k": options["top_k"],
            "include_vectors": options["include_vectors"],
            "include_metadata": options["include_metadata"],
            "explain": options["explain"],
            "use_index": options["use_index"],
        }

        if options["filter"]:
            result["filter"] = options["filter"]

        if options["timeout_ms"]:
            result["timeout_ms"] = options["timeout_ms"]

        return result


# Convenience functions
def search(query_vector: list[float]) -> SearchBuilder:
    """Create a new SearchBuilder"""
    return SearchBuilder(query_vector)


def similarity_search(
    query_vector: list[float],
    top_k: int = 10,
    include_metadata: bool = True,
    include_vectors: bool = False,
) -> dict[str, Any]:
    """Create simple similarity search options"""
    return (
        SearchBuilder(query_vector)
        .top_k(top_k)
        .include_metadata(include_metadata)
        .include_vectors(include_vectors)
        .build()
    )
