"""
ProximaDB Filter Builder API

Provides a fluent interface for building complex metadata filters with AND/OR/NOT operators.
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional, Union


class FilterOp(str, Enum):
    """Filter operations"""

    EQUALS = "equals"
    NOT_EQUALS = "not_equals"
    GREATER_THAN = "gt"
    GREATER_THAN_OR_EQUAL = "gte"
    LESS_THAN = "lt"
    LESS_THAN_OR_EQUAL = "lte"
    IN = "in"
    NOT_IN = "not_in"
    CONTAINS = "contains"
    NOT_CONTAINS = "not_contains"
    EXISTS = "exists"
    NOT_EXISTS = "not_exists"


class LogicalOp(str, Enum):
    """Logical operators for combining filters"""

    AND = "and"
    OR = "or"
    NOT = "not"


@dataclass
class FilterCondition:
    """A single filter condition"""

    field: str
    operation: FilterOp
    value: Any = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary representation"""
        return {
            "field": self.field,
            "operation": self.operation.value,
            "value": self.value,
        }


@dataclass
class FilterGroup:
    """A group of filter conditions with a logical operator"""

    operator: LogicalOp = LogicalOp.AND
    conditions: List[Union[FilterCondition, "FilterGroup"]] = field(
        default_factory=list
    )

    def add_condition(
        self, field: str, operation: FilterOp, value: Any = None
    ) -> "FilterGroup":
        """Add a filter condition to this group"""
        self.conditions.append(FilterCondition(field, operation, value))
        return self

    def add_group(self, group: "FilterGroup") -> "FilterGroup":
        """Add a nested filter group"""
        self.conditions.append(group)
        return self

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary representation"""
        return {
            "operator": self.operator.value,
            "conditions": [
                c.to_dict() if isinstance(c, FilterCondition) else c.to_dict()
                for c in self.conditions
            ],
        }


class FilterBuilder:
    """
    Fluent filter builder for complex metadata queries

    Examples:
        # Simple equality filter
        filter = FilterBuilder().equals("category", "electronics").build()

        # Multiple AND conditions
        filter = (FilterBuilder()
            .equals("category", "electronics")
            .greater_than("price", 100)
            .less_than("price", 1000)
            .build())

        # OR conditions
        filter = (FilterBuilder()
            .or_()
            .equals("brand", "Apple")
            .equals("brand", "Samsung")
            .build())

        # Complex nested filters
        filter = (FilterBuilder()
            .equals("category", "electronics")
            .and_group(
                FilterBuilder()
                .or_()
                .equals("brand", "Apple")
                .equals("brand", "Samsung")
            )
            .greater_than("rating", 4.0)
            .build())
    """

    def __init__(self):
        self._root = FilterGroup(operator=LogicalOp.AND)
        self._current_group = self._root
        self._group_stack = []

    def and_(self) -> "FilterBuilder":
        """Start a new AND group"""
        new_group = FilterGroup(operator=LogicalOp.AND)
        self._current_group.add_group(new_group)
        self._group_stack.append(self._current_group)
        self._current_group = new_group
        return self

    def or_(self) -> "FilterBuilder":
        """Start a new OR group"""
        new_group = FilterGroup(operator=LogicalOp.OR)
        self._current_group.add_group(new_group)
        self._group_stack.append(self._current_group)
        self._current_group = new_group
        return self

    def not_(self) -> "FilterBuilder":
        """Start a new NOT group"""
        new_group = FilterGroup(operator=LogicalOp.NOT)
        self._current_group.add_group(new_group)
        self._group_stack.append(self._current_group)
        self._current_group = new_group
        return self

    def end_group(self) -> "FilterBuilder":
        """End the current group and return to parent"""
        if self._group_stack:
            self._current_group = self._group_stack.pop()
        return self

    def and_group(self, other: "FilterBuilder") -> "FilterBuilder":
        """Add another filter builder's conditions as an AND group"""
        self._current_group.add_group(other._root)
        return self

    def or_group(self, other: "FilterBuilder") -> "FilterBuilder":
        """Add another filter builder's conditions as an OR group"""
        other._root.operator = LogicalOp.OR
        self._current_group.add_group(other._root)
        return self

    def equals(self, field: str, value: Any) -> "FilterBuilder":
        """Add an equality filter"""
        self._current_group.add_condition(field, FilterOp.EQUALS, value)
        return self

    def not_equals(self, field: str, value: Any) -> "FilterBuilder":
        """Add a not-equals filter"""
        self._current_group.add_condition(field, FilterOp.NOT_EQUALS, value)
        return self

    def greater_than(self, field: str, value: Any) -> "FilterBuilder":
        """Add a greater-than filter"""
        self._current_group.add_condition(field, FilterOp.GREATER_THAN, value)
        return self

    def gte(self, field: str, value: Any) -> "FilterBuilder":
        """Add a greater-than-or-equal filter"""
        self._current_group.add_condition(field, FilterOp.GREATER_THAN_OR_EQUAL, value)
        return self

    def less_than(self, field: str, value: Any) -> "FilterBuilder":
        """Add a less-than filter"""
        self._current_group.add_condition(field, FilterOp.LESS_THAN, value)
        return self

    def lte(self, field: str, value: Any) -> "FilterBuilder":
        """Add a less-than-or-equal filter"""
        self._current_group.add_condition(field, FilterOp.LESS_THAN_OR_EQUAL, value)
        return self

    def in_(self, field: str, values: List[Any]) -> "FilterBuilder":
        """Add an IN filter"""
        self._current_group.add_condition(field, FilterOp.IN, values)
        return self

    def not_in(self, field: str, values: List[Any]) -> "FilterBuilder":
        """Add a NOT IN filter"""
        self._current_group.add_condition(field, FilterOp.NOT_IN, values)
        return self

    def contains(self, field: str, value: str) -> "FilterBuilder":
        """Add a contains filter (for string fields)"""
        self._current_group.add_condition(field, FilterOp.CONTAINS, value)
        return self

    def exists(self, field: str) -> "FilterBuilder":
        """Add an exists filter"""
        self._current_group.add_condition(field, FilterOp.EXISTS)
        return self

    def not_exists(self, field: str) -> "FilterBuilder":
        """Add a not-exists filter"""
        self._current_group.add_condition(field, FilterOp.NOT_EXISTS)
        return self

    def build(self) -> FilterGroup:
        """Build and return the filter group"""
        return self._root

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary representation"""
        return self._root.to_dict()

    def to_proto_filter(self) -> Any:
        """Convert to ProximaDB proto MetadataFilter (v1 proto structure)

        Returns:
            proximadb.v1.entity_pb2.MetadataFilter instance

        Raises:
            ImportError: If gRPC proto modules are not available
            ValueError: If filter contains unsupported operations
        """
        try:
            from proximadb_sdk.v1 import entity_pb2
        except ImportError as e:
            raise ImportError(
                "Proto modules not available. Install with: pip install proximadb[grpc]"
            ) from e

        return self._build_proto_filter(self._root)

    def _build_proto_filter(self, group: FilterGroup) -> Any:
        """Recursively build proto filter from filter group (v1 proto)

        Args:
            group: FilterGroup to convert

        Returns:
            proximadb.v1.entity_pb2.MetadataFilter instance

        Raises:
            ValueError: If filter contains unsupported operations
        """
        from proximadb_sdk.v1 import entity_pb2

        # Map logical operators to v1 proto enums
        # From entity.proto: enum LogicalOp { AND = 0; OR = 1; NOT = 2; }
        logical_op_map = {
            LogicalOp.AND: entity_pb2.AND,
            LogicalOp.OR: entity_pb2.OR,
            LogicalOp.NOT: entity_pb2.NOT,
        }

        # Map comparison operators to v1 proto enums
        # From entity.proto: enum ComparisonOp { EQ = 0; NE = 1; GT = 2; GTE = 3; LT = 4; LTE = 5; IN = 6; NOT_IN = 7; CONTAINS = 8; }
        comparison_op_map = {
            FilterOp.EQUALS: entity_pb2.EQ,
            FilterOp.NOT_EQUALS: entity_pb2.NE,
            FilterOp.GREATER_THAN: entity_pb2.GT,
            FilterOp.GREATER_THAN_OR_EQUAL: entity_pb2.GTE,
            FilterOp.LESS_THAN: entity_pb2.LT,
            FilterOp.LESS_THAN_OR_EQUAL: entity_pb2.LTE,
            FilterOp.IN: entity_pb2.IN,
            FilterOp.NOT_IN: entity_pb2.NOT_IN,
            FilterOp.CONTAINS: entity_pb2.CONTAINS,
            # These operations are not supported in v1 proto ComparisonOp:
            # FilterOp.NOT_CONTAINS
            # FilterOp.EXISTS
            # FilterOp.NOT_EXISTS
        }

        # Create MetadataFilter with logical operator
        meta_filter = entity_pb2.MetadataFilter(
            op=logical_op_map.get(group.operator, entity_pb2.AND)
        )

        # Add all conditions
        for condition in group.conditions:
            if isinstance(condition, FilterCondition):
                # Convert condition to proto FilterClause
                proto_op = comparison_op_map.get(condition.operation)
                if proto_op is None:
                    # Unsupported operation - raise error
                    import warnings

                    warnings.warn(
                        f"Filter operation {condition.operation} not supported in v1 proto, skipping. "
                        f"Supported operations: {list(comparison_op_map.keys())}"
                    )
                    continue

                # Create FilterClause
                filter_clause = entity_pb2.FilterClause(
                    field=condition.field, op=proto_op
                )

                # Set value using oneof based on type
                if condition.value is not None:
                    self._set_filter_clause_value(filter_clause, condition.value)

                meta_filter.clauses.append(filter_clause)

            elif isinstance(condition, FilterGroup):
                # v1 proto doesn't support nested MetadataFilter directly
                # We flatten nested groups by adding their clauses with appropriate logic
                # This is a limitation - deeply nested filters may lose some structure
                nested_filter = self._build_proto_filter(condition)

                # If nested filter has the same operator as parent, flatten completely
                if nested_filter.op == meta_filter.op:
                    meta_filter.clauses.extend(nested_filter.clauses)
                else:
                    # Different operator - we lose nesting structure
                    # Add all clauses but with a warning
                    import warnings

                    warnings.warn(
                        f"Nested filter groups with different operators may lose structure when converted to proto. "
                        f"Parent operator: {group.operator}, Nested operator: {condition.operator}"
                    )
                    meta_filter.clauses.extend(nested_filter.clauses)

        return meta_filter

    def _set_filter_clause_value(self, filter_clause: Any, value: Any) -> None:
        """Set value in FilterClause using oneof (v1 proto)

        Args:
            filter_clause: proximadb.v1.entity_pb2.FilterClause instance
            value: Python value to set

        Notes:
            FilterClause oneof value: string_value, int_value, double_value, bool_value
            Lists are not directly supported - for IN/NOT_IN operations, we use string representation
        """
        # Set value based on type using oneof
        if isinstance(value, bool):
            # Check bool before int (bool is subclass of int in Python)
            filter_clause.bool_value = value
        elif isinstance(value, int):
            filter_clause.int_value = value
        elif isinstance(value, float):
            filter_clause.double_value = value
        elif isinstance(value, str):
            filter_clause.string_value = value
        elif isinstance(value, list):
            # For lists (IN/NOT_IN operations), convert to comma-separated string
            # This is a limitation of the v1 proto FilterClause structure
            # The server should handle parsing this back to a list
            if all(isinstance(v, str) for v in value):
                filter_clause.string_value = ",".join(value)
            elif all(isinstance(v, (int, float)) for v in value):
                filter_clause.string_value = ",".join(str(v) for v in value)
            else:
                # Mixed types - convert all to strings
                filter_clause.string_value = ",".join(str(v) for v in value)
        else:
            # Fallback to string representation
            filter_clause.string_value = str(value)


# Convenience functions
def eq(field: str, value: Any) -> FilterBuilder:
    """Create a simple equality filter"""
    return FilterBuilder().equals(field, value)


def gt(field: str, value: Any) -> FilterBuilder:
    """Create a greater-than filter"""
    return FilterBuilder().greater_than(field, value)


def lt(field: str, value: Any) -> FilterBuilder:
    """Create a less-than filter"""
    return FilterBuilder().less_than(field, value)


def in_list(field: str, values: List[Any]) -> FilterBuilder:
    """Create an IN filter"""
    return FilterBuilder().in_(field, values)


def and_filters(*filters: FilterBuilder) -> FilterBuilder:
    """Combine multiple filters with AND"""
    builder = FilterBuilder()
    for f in filters:
        builder.and_group(f)
    return builder


def or_filters(*filters: FilterBuilder) -> FilterBuilder:
    """Combine multiple filters with OR"""
    builder = FilterBuilder().or_()
    for f in filters:
        for condition in f._root.conditions:
            if isinstance(condition, FilterCondition):
                builder._current_group.add_condition(
                    condition.field, condition.operation, condition.value
                )
            else:
                builder._current_group.add_group(condition)
    builder.end_group()
    return builder
