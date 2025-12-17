"""
Test suite for ProximaDB filter builder API - simplified version
"""
import pytest
from proximadb_sdk.filters import (
    FilterOp,
    LogicalOp,
    FilterCondition,
    FilterGroup,
    FilterBuilder,
    eq,
    gt,
    lt,
    in_list,
    and_filters,
    or_filters
)


class TestFilterCondition:
    """Test FilterCondition dataclass"""
    
    def test_filter_condition_creation(self):
        """Test creating a filter condition"""
        condition = FilterCondition(
            field="price",
            operation=FilterOp.GREATER_THAN,
            value=100
        )
        assert condition.field == "price"
        assert condition.operation == FilterOp.GREATER_THAN
        assert condition.value == 100
    
    def test_filter_condition_to_dict(self):
        """Test converting filter condition to dict"""
        condition = FilterCondition(
            field="category",
            operation=FilterOp.EQUALS,
            value="electronics"
        )
        result = condition.to_dict()
        assert result == {
            "field": "category",
            "operation": "equals",
            "value": "electronics"
        }


class TestFilterGroup:
    """Test FilterGroup dataclass"""
    
    def test_filter_group_creation(self):
        """Test creating a filter group"""
        group = FilterGroup()
        assert group.operator == LogicalOp.AND
        assert group.conditions == []
    
    def test_add_condition(self):
        """Test adding conditions to a group"""
        group = FilterGroup()
        group.add_condition("price", FilterOp.GREATER_THAN, 100)
        group.add_condition("category", FilterOp.EQUALS, "electronics")
        
        assert len(group.conditions) == 2
        assert group.conditions[0].field == "price"
        assert group.conditions[1].field == "category"
    
    def test_filter_group_to_dict(self):
        """Test converting filter group to dict"""
        group = FilterGroup(operator=LogicalOp.AND)
        group.add_condition("price", FilterOp.GREATER_THAN, 100)
        group.add_condition("category", FilterOp.EQUALS, "electronics")
        
        result = group.to_dict()
        assert result == {
            "operator": "and",
            "conditions": [
                {"field": "price", "operation": "gt", "value": 100},
                {"field": "category", "operation": "equals", "value": "electronics"}
            ]
        }


class TestFilterBuilder:
    """Test FilterBuilder fluent API"""
    
    def test_simple_equals_filter(self):
        """Test simple equality filter"""
        builder = FilterBuilder()
        builder.equals("category", "electronics")
        filter_group = builder.build()
        
        assert filter_group.to_dict() == {
            "operator": "and",
            "conditions": [
                {"field": "category", "operation": "equals", "value": "electronics"}
            ]
        }
    
    def test_comparison_operators(self):
        """Test comparison operators"""
        builder = FilterBuilder()
        builder.equals("status", "active")
        builder.not_equals("type", "test")
        builder.greater_than("score", 80)
        builder.less_than("age", 30)
        
        filter_group = builder.build()
        result = filter_group.to_dict()
        
        assert len(result["conditions"]) == 4
        assert result["conditions"][0]["operation"] == "equals"
        assert result["conditions"][1]["operation"] == "not_equals"
        assert result["conditions"][2]["operation"] == "gt"
        assert result["conditions"][3]["operation"] == "lt"
    
    def test_in_operator(self):
        """Test IN operator"""
        builder = FilterBuilder()
        builder.in_("category", ["electronics", "computers"])
        
        filter_group = builder.build()
        result = filter_group.to_dict()
        
        assert result["conditions"][0] == {
            "field": "category",
            "operation": "in",
            "value": ["electronics", "computers"]
        }
    
    def test_exists_operator(self):
        """Test EXISTS operator"""
        builder = FilterBuilder()
        builder.exists("metadata")
        
        filter_group = builder.build()
        result = filter_group.to_dict()
        
        assert result["conditions"][0] == {
            "field": "metadata",
            "operation": "exists",
            "value": None
        }
    
    def test_to_dict_method(self):
        """Test to_dict method of FilterBuilder"""
        builder = FilterBuilder()
        builder.equals("test", "value")
        
        # Test to_dict method directly
        result = builder.to_dict()
        assert result == {
            "operator": "and",
            "conditions": [
                {"field": "test", "operation": "equals", "value": "value"}
            ]
        }


class TestHelperFunctions:
    """Test helper functions"""
    
    def test_eq_function(self):
        """Test eq() helper"""
        filter_builder = eq("category", "electronics")
        result = filter_builder.build().to_dict()
        
        assert result == {
            "operator": "and",
            "conditions": [
                {"field": "category", "operation": "equals", "value": "electronics"}
            ]
        }
    
    def test_gt_function(self):
        """Test gt() helper"""
        filter_builder = gt("price", 100)
        result = filter_builder.build().to_dict()
        
        assert result == {
            "operator": "and",
            "conditions": [
                {"field": "price", "operation": "gt", "value": 100}
            ]
        }
    
    def test_and_filters_function(self):
        """Test and_filters() helper"""
        filter1 = eq("category", "electronics")
        filter2 = gt("price", 100)
        
        combined = and_filters(filter1, filter2)
        result = combined.build().to_dict()
        
        assert result["operator"] == "and"
        assert len(result["conditions"]) == 2
    
    def test_or_filters_function(self):
        """Test or_filters() helper"""
        filter1 = eq("brand", "Apple")
        filter2 = eq("brand", "Samsung")
        
        combined = or_filters(filter1, filter2)
        result = combined.build().to_dict()
        
        assert result["operator"] == "and"
        assert len(result["conditions"]) == 1
        assert result["conditions"][0]["operator"] == "or"