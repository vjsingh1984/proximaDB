"""
ProximaDB Filter Builder API

Provides a fluent interface for building complex metadata filters with AND/OR/NOT operators.
"""

from typing import Any, Dict, List, Optional, Union
from enum import Enum
from dataclasses import dataclass, field


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
            "value": self.value
        }


@dataclass
class FilterGroup:
    """A group of filter conditions with a logical operator"""
    operator: LogicalOp = LogicalOp.AND
    conditions: List[Union[FilterCondition, 'FilterGroup']] = field(default_factory=list)
    
    def add_condition(self, field: str, operation: FilterOp, value: Any = None) -> 'FilterGroup':
        """Add a filter condition to this group"""
        self.conditions.append(FilterCondition(field, operation, value))
        return self
    
    def add_group(self, group: 'FilterGroup') -> 'FilterGroup':
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
            ]
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
    
    def and_(self) -> 'FilterBuilder':
        """Start a new AND group"""
        new_group = FilterGroup(operator=LogicalOp.AND)
        self._current_group.add_group(new_group)
        self._group_stack.append(self._current_group)
        self._current_group = new_group
        return self
    
    def or_(self) -> 'FilterBuilder':
        """Start a new OR group"""
        new_group = FilterGroup(operator=LogicalOp.OR)
        self._current_group.add_group(new_group)
        self._group_stack.append(self._current_group)
        self._current_group = new_group
        return self
    
    def not_(self) -> 'FilterBuilder':
        """Start a new NOT group"""
        new_group = FilterGroup(operator=LogicalOp.NOT)
        self._current_group.add_group(new_group)
        self._group_stack.append(self._current_group)
        self._current_group = new_group
        return self
    
    def end_group(self) -> 'FilterBuilder':
        """End the current group and return to parent"""
        if self._group_stack:
            self._current_group = self._group_stack.pop()
        return self
    
    def and_group(self, other: 'FilterBuilder') -> 'FilterBuilder':
        """Add another filter builder's conditions as an AND group"""
        self._current_group.add_group(other._root)
        return self
    
    def or_group(self, other: 'FilterBuilder') -> 'FilterBuilder':
        """Add another filter builder's conditions as an OR group"""
        other._root.operator = LogicalOp.OR
        self._current_group.add_group(other._root)
        return self
    
    def equals(self, field: str, value: Any) -> 'FilterBuilder':
        """Add an equality filter"""
        self._current_group.add_condition(field, FilterOp.EQUALS, value)
        return self
    
    def not_equals(self, field: str, value: Any) -> 'FilterBuilder':
        """Add a not-equals filter"""
        self._current_group.add_condition(field, FilterOp.NOT_EQUALS, value)
        return self
    
    def greater_than(self, field: str, value: Any) -> 'FilterBuilder':
        """Add a greater-than filter"""
        self._current_group.add_condition(field, FilterOp.GREATER_THAN, value)
        return self
    
    def gte(self, field: str, value: Any) -> 'FilterBuilder':
        """Add a greater-than-or-equal filter"""
        self._current_group.add_condition(field, FilterOp.GREATER_THAN_OR_EQUAL, value)
        return self
    
    def less_than(self, field: str, value: Any) -> 'FilterBuilder':
        """Add a less-than filter"""
        self._current_group.add_condition(field, FilterOp.LESS_THAN, value)
        return self
    
    def lte(self, field: str, value: Any) -> 'FilterBuilder':
        """Add a less-than-or-equal filter"""
        self._current_group.add_condition(field, FilterOp.LESS_THAN_OR_EQUAL, value)
        return self
    
    def in_(self, field: str, values: List[Any]) -> 'FilterBuilder':
        """Add an IN filter"""
        self._current_group.add_condition(field, FilterOp.IN, values)
        return self
    
    def not_in(self, field: str, values: List[Any]) -> 'FilterBuilder':
        """Add a NOT IN filter"""
        self._current_group.add_condition(field, FilterOp.NOT_IN, values)
        return self
    
    def contains(self, field: str, value: str) -> 'FilterBuilder':
        """Add a contains filter (for string fields)"""
        self._current_group.add_condition(field, FilterOp.CONTAINS, value)
        return self
    
    def exists(self, field: str) -> 'FilterBuilder':
        """Add an exists filter"""
        self._current_group.add_condition(field, FilterOp.EXISTS)
        return self
    
    def not_exists(self, field: str) -> 'FilterBuilder':
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
        """Convert to ProximaDB proto MetadataFilter"""
        from . import proximadb_pb2 as pb2
        return self._build_proto_filter(self._root)
    
    def _build_proto_filter(self, group: FilterGroup) -> Any:
        """Recursively build proto filter from filter group"""
        from . import proximadb_pb2 as pb2
        
        # Map operators
        op_map = {
            LogicalOp.AND: pb2.FilterOperator.AND,
            LogicalOp.OR: pb2.FilterOperator.OR,
            LogicalOp.NOT: pb2.FilterOperator.NOT,
        }
        
        # Map filter operations (only those available in proto)
        filter_op_map = {
            FilterOp.EQUALS: pb2.FilterOperation.EQUALS,
            FilterOp.NOT_EQUALS: pb2.FilterOperation.NOT_EQUALS,
            FilterOp.GREATER_THAN: pb2.FilterOperation.GREATER_THAN,
            FilterOp.GREATER_THAN_OR_EQUAL: pb2.FilterOperation.GREATER_THAN_OR_EQUAL,
            FilterOp.LESS_THAN: pb2.FilterOperation.LESS_THAN,
            FilterOp.LESS_THAN_OR_EQUAL: pb2.FilterOperation.LESS_THAN_OR_EQUAL,
            FilterOp.IN: pb2.FilterOperation.IN,
            FilterOp.NOT_IN: pb2.FilterOperation.NOT_IN,
            FilterOp.CONTAINS: pb2.FilterOperation.CONTAINS,
            # These don't exist in proto yet:
            # FilterOp.NOT_CONTAINS: pb2.FilterOperation.NOT_CONTAINS,
            # FilterOp.EXISTS: pb2.FilterOperation.EXISTS,
            # FilterOp.NOT_EXISTS: pb2.FilterOperation.NOT_EXISTS,
        }
        
        meta_filter = pb2.MetadataFilter(
            operator=op_map.get(group.operator, pb2.FilterOperator.AND)
        )
        
        for condition in group.conditions:
            if isinstance(condition, FilterCondition):
                # Convert condition to proto
                proto_op = filter_op_map.get(condition.operation)
                if proto_op is None:
                    # Skip unsupported operations with a warning
                    import warnings
                    warnings.warn(f"Filter operation {condition.operation} not supported in proto, skipping")
                    continue
                    
                filter_cond = pb2.FilterCondition(
                    field_name=condition.field,
                    operation=proto_op
                )
                
                # Set value if present
                if condition.value is not None:
                    filter_cond.value.CopyFrom(self._value_to_metadata_value(condition.value))
                
                meta_filter.conditions.append(filter_cond)
            elif isinstance(condition, FilterGroup):
                # Nested group - we need to handle this differently
                # Proto doesn't support nested MetadataFilter, so we flatten with appropriate logic
                # This is a limitation we'll need to work around
                nested_filter = self._build_proto_filter(condition)
                # For now, just add all conditions from nested group
                meta_filter.conditions.extend(nested_filter.conditions)
        
        return meta_filter
    
    def _value_to_metadata_value(self, value: Any) -> Any:
        """Convert Python value to MetadataValue proto"""
        from . import proximadb_pb2 as pb2
        
        meta_value = pb2.MetadataValue()
        
        if isinstance(value, str):
            meta_value.string_value = value
        elif isinstance(value, bool):
            meta_value.bool_value = value
        elif isinstance(value, int):
            meta_value.int_value = value
        elif isinstance(value, float):
            meta_value.double_value = value
        elif isinstance(value, list):
            if all(isinstance(v, str) for v in value):
                meta_value.string_array.values.extend(value)
            elif all(isinstance(v, int) for v in value):
                meta_value.int_array.values.extend(value)
            elif all(isinstance(v, (int, float)) for v in value):
                meta_value.double_array.values.extend([float(v) for v in value])
            else:
                # Convert to string array as fallback
                meta_value.string_array.values.extend([str(v) for v in value])
        else:
            # Fallback to string representation
            meta_value.string_value = str(value)
        
        return meta_value


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
                builder._current_group.add_condition(condition.field, condition.operation, condition.value)
            else:
                builder._current_group.add_group(condition)
    builder.end_group()
    return builder