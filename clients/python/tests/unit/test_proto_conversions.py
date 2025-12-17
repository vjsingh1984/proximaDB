"""
Unit tests for proto conversion utilities (v1 proto)

Tests filter conversion and SearchParams conversion with v1 proto structure.
"""
import pytest
from proximadb_sdk.filters import FilterBuilder, FilterOp, LogicalOp
from proximadb_sdk.search_utils import build_search_params_grpc, _python_value_to_sql_value


class TestFilterProtoConversion:
    """Test FilterBuilder to_proto_filter() conversion"""

    def test_simple_equality_filter(self):
        """Test simple equality filter conversion"""
        from proximadb_sdk.v1 import entity_pb2

        filter_builder = FilterBuilder().equals('category', 'electronics')
        proto_filter = filter_builder.to_proto_filter()

        assert isinstance(proto_filter, entity_pb2.MetadataFilter)
        assert proto_filter.op == entity_pb2.AND
        assert len(proto_filter.clauses) == 1
        assert proto_filter.clauses[0].field == 'category'
        assert proto_filter.clauses[0].op == entity_pb2.EQ
        assert proto_filter.clauses[0].string_value == 'electronics'

    def test_multiple_and_conditions(self):
        """Test multiple AND conditions"""
        from proximadb_sdk.v1 import entity_pb2

        filter_builder = (FilterBuilder()
            .equals('category', 'electronics')
            .greater_than('price', 100)
            .less_than('price', 1000))
        proto_filter = filter_builder.to_proto_filter()

        assert proto_filter.op == entity_pb2.AND
        assert len(proto_filter.clauses) == 3

        # Check first clause
        assert proto_filter.clauses[0].field == 'category'
        assert proto_filter.clauses[0].op == entity_pb2.EQ
        assert proto_filter.clauses[0].string_value == 'electronics'

        # Check second clause
        assert proto_filter.clauses[1].field == 'price'
        assert proto_filter.clauses[1].op == entity_pb2.GT
        assert proto_filter.clauses[1].int_value == 100

        # Check third clause
        assert proto_filter.clauses[2].field == 'price'
        assert proto_filter.clauses[2].op == entity_pb2.LT
        assert proto_filter.clauses[2].int_value == 1000

    def test_or_filter(self):
        """Test OR filter"""
        from proximadb_sdk.v1 import entity_pb2

        filter_builder = (FilterBuilder()
            .or_()
            .equals('brand', 'Apple')
            .equals('brand', 'Samsung'))
        proto_filter = filter_builder.to_proto_filter()

        # The root is still AND, but it contains an OR group
        # Due to flattening, the OR group's clauses are added
        assert len(proto_filter.clauses) == 2

    def test_comparison_operators(self):
        """Test all comparison operators"""
        from proximadb_sdk.v1 import entity_pb2

        test_cases = [
            (FilterOp.EQUALS, entity_pb2.EQ),
            (FilterOp.NOT_EQUALS, entity_pb2.NE),
            (FilterOp.GREATER_THAN, entity_pb2.GT),
            (FilterOp.GREATER_THAN_OR_EQUAL, entity_pb2.GTE),
            (FilterOp.LESS_THAN, entity_pb2.LT),
            (FilterOp.LESS_THAN_OR_EQUAL, entity_pb2.LTE),
            (FilterOp.IN, entity_pb2.IN),
            (FilterOp.NOT_IN, entity_pb2.NOT_IN),
            (FilterOp.CONTAINS, entity_pb2.CONTAINS),
        ]

        for filter_op, expected_proto_op in test_cases:
            filter_builder = FilterBuilder()
            filter_builder._current_group.add_condition('field', filter_op, 'value')
            proto_filter = filter_builder.to_proto_filter()

            assert proto_filter.clauses[0].op == expected_proto_op

    def test_value_type_conversion(self):
        """Test different value types in filters"""
        from proximadb_sdk.v1 import entity_pb2

        # Test string value
        f1 = FilterBuilder().equals('name', 'test')
        p1 = f1.to_proto_filter()
        assert p1.clauses[0].string_value == 'test'

        # Test integer value
        f2 = FilterBuilder().equals('count', 42)
        p2 = f2.to_proto_filter()
        assert p2.clauses[0].int_value == 42

        # Test float value
        f3 = FilterBuilder().equals('rating', 4.5)
        p3 = f3.to_proto_filter()
        assert p3.clauses[0].double_value == 4.5

        # Test boolean value
        f4 = FilterBuilder().equals('active', True)
        p4 = f4.to_proto_filter()
        assert p4.clauses[0].bool_value is True

    def test_in_filter_with_list(self):
        """Test IN filter with list values"""
        from proximadb_sdk.v1 import entity_pb2

        filter_builder = FilterBuilder().in_('category', ['electronics', 'books'])
        proto_filter = filter_builder.to_proto_filter()

        assert proto_filter.clauses[0].op == entity_pb2.IN
        # List is converted to comma-separated string
        assert proto_filter.clauses[0].string_value == 'electronics,books'

    def test_nested_filter_flattening(self):
        """Test nested filter groups get flattened"""
        from proximadb_sdk.v1 import entity_pb2

        # Create nested filter
        inner_filter = FilterBuilder().equals('brand', 'Apple')
        outer_filter = (FilterBuilder()
            .equals('category', 'electronics')
            .and_group(inner_filter))

        proto_filter = outer_filter.to_proto_filter()

        # Both conditions should be flattened into root
        assert len(proto_filter.clauses) == 2

    def test_unsupported_operation_warning(self):
        """Test that unsupported operations emit warnings"""
        import warnings

        filter_builder = FilterBuilder().exists('field')

        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            proto_filter = filter_builder.to_proto_filter()

            # Should have warning about unsupported operation
            assert len(w) > 0
            assert "not supported in v1 proto" in str(w[0].message)


class TestSearchParamsProtoConversion:
    """Test build_search_params_grpc() conversion"""

    def test_basic_search_params(self):
        """Test basic SearchParams creation"""
        from proximadb_sdk.v1 import vector_types_pb2

        params = build_search_params_grpc(
            top_k=10,
            accuracy_threshold=0.95
        )

        assert isinstance(params, vector_types_pb2.SearchParams)
        assert params.top_k == 10
        assert params.accuracy_threshold == 0.95

    def test_all_scalar_fields(self):
        """Test all scalar optional fields"""
        from proximadb_sdk.v1 import vector_types_pb2

        params = build_search_params_grpc(
            top_k=10,
            accuracy_threshold=0.95,
            include_expired=True,
            timeout_ms=5000,
            enable_two_stage=True,
            enable_clustering_hint=True,
            enable_metadata_filtering_hint=False
        )

        assert params.top_k == 10
        assert params.accuracy_threshold == 0.95
        assert params.include_expired is True
        assert params.timeout_ms == 5000
        assert params.enable_two_stage is True
        assert params.enable_clustering_hint is True
        assert params.enable_metadata_filtering_hint is False

    def test_custom_hints(self):
        """Test custom hints map"""
        params = build_search_params_grpc(
            custom_hints={
                'my_string': 'value',
                'my_int': 42,
                'my_float': 3.14,
                'my_bool': True
            }
        )

        assert 'my_string' in params.custom_hints
        assert params.custom_hints['my_string'].string_value == 'value'
        assert params.custom_hints['my_int'].int64_value == 42
        assert params.custom_hints['my_float'].number_value == 3.14
        assert params.custom_hints['my_bool'].bool_value is True

    def test_additional_params_as_hints(self):
        """Test additional parameters are added as custom hints"""
        params = build_search_params_grpc(
            distance_metric='cosine',
            requires_ordering=True,
            candidate_multiplier=1.5
        )

        assert params.custom_hints['distance_metric'].string_value == 'cosine'
        assert params.custom_hints['requires_ordering'].bool_value is True
        assert params.custom_hints['candidate_multiplier'].number_value == 1.5

    def test_streaming_params_as_hints(self):
        """Test streaming parameters are added as custom hints"""
        params = build_search_params_grpc(
            streaming_buffer_size=1000,
            streaming_concurrent_search=True,
            streaming_max_concurrent_tasks=4,
            streaming_batch_size=100
        )

        assert params.custom_hints['streaming_buffer_size'].int64_value == 1000
        assert params.custom_hints['streaming_concurrent_search'].bool_value is True
        assert params.custom_hints['streaming_max_concurrent_tasks'].int64_value == 4
        assert params.custom_hints['streaming_batch_size'].int64_value == 100

    def test_combined_hints(self):
        """Test combining custom hints with additional params"""
        params = build_search_params_grpc(
            top_k=5,
            custom_hints={'custom_key': 'custom_value'},
            distance_metric='euclidean',
            streaming_buffer_size=500
        )

        assert params.top_k == 5
        assert 'custom_key' in params.custom_hints
        assert 'distance_metric' in params.custom_hints
        assert 'streaming_buffer_size' in params.custom_hints
        assert len(params.custom_hints) == 3


class TestSqlValueConversion:
    """Test _python_value_to_sql_value() helper"""

    def test_string_value(self):
        """Test string to SqlValue"""
        from proximadb_sdk.v1 import types_pb2

        sql_value = _python_value_to_sql_value('test', types_pb2)
        assert sql_value.string_value == 'test'

    def test_int_value(self):
        """Test int to SqlValue"""
        from proximadb_sdk.v1 import types_pb2

        sql_value = _python_value_to_sql_value(42, types_pb2)
        assert sql_value.int64_value == 42

    def test_float_value(self):
        """Test float to SqlValue"""
        from proximadb_sdk.v1 import types_pb2

        sql_value = _python_value_to_sql_value(3.14, types_pb2)
        assert sql_value.number_value == 3.14

    def test_bool_value(self):
        """Test bool to SqlValue"""
        from proximadb_sdk.v1 import types_pb2

        sql_value = _python_value_to_sql_value(True, types_pb2)
        assert sql_value.bool_value is True

    def test_bytes_value(self):
        """Test bytes to SqlValue"""
        from proximadb_sdk.v1 import types_pb2

        sql_value = _python_value_to_sql_value(b'binary', types_pb2)
        assert sql_value.bytes_value == b'binary'

    def test_none_value(self):
        """Test None to SqlValue"""
        from proximadb_sdk.v1 import types_pb2
        from google.protobuf import struct_pb2

        sql_value = _python_value_to_sql_value(None, types_pb2)
        assert sql_value.null_value == struct_pb2.NULL_VALUE

    def test_list_value(self):
        """Test list to SqlValue (SqlArray)"""
        from proximadb_sdk.v1 import types_pb2

        sql_value = _python_value_to_sql_value([1, 2, 3], types_pb2)
        assert len(sql_value.array_value.values) == 3
        assert sql_value.array_value.values[0].int64_value == 1
        assert sql_value.array_value.values[1].int64_value == 2
        assert sql_value.array_value.values[2].int64_value == 3

    def test_dict_value(self):
        """Test dict to SqlValue (SqlObject)"""
        from proximadb_sdk.v1 import types_pb2

        sql_value = _python_value_to_sql_value({'key': 'value', 'count': 42}, types_pb2)
        assert 'key' in sql_value.object_value.fields
        assert 'count' in sql_value.object_value.fields
        assert sql_value.object_value.fields['key'].string_value == 'value'
        assert sql_value.object_value.fields['count'].int64_value == 42

    def test_nested_structures(self):
        """Test nested lists and dicts"""
        from proximadb_sdk.v1 import types_pb2

        # Nested list
        sql_value = _python_value_to_sql_value([[1, 2], [3, 4]], types_pb2)
        assert len(sql_value.array_value.values) == 2
        assert len(sql_value.array_value.values[0].array_value.values) == 2

        # Nested dict
        sql_value2 = _python_value_to_sql_value({'outer': {'inner': 'value'}}, types_pb2)
        assert 'outer' in sql_value2.object_value.fields
        assert 'inner' in sql_value2.object_value.fields['outer'].object_value.fields

    def test_fallback_string_conversion(self):
        """Test fallback to string for unknown types"""
        from proximadb_sdk.v1 import types_pb2

        class CustomObject:
            def __str__(self):
                return "custom_object"

        sql_value = _python_value_to_sql_value(CustomObject(), types_pb2)
        assert sql_value.string_value == "custom_object"


class TestEdgeCases:
    """Test edge cases and error handling"""

    def test_empty_filter(self):
        """Test empty filter builder"""
        from proximadb_sdk.v1 import entity_pb2

        filter_builder = FilterBuilder()
        proto_filter = filter_builder.to_proto_filter()

        assert isinstance(proto_filter, entity_pb2.MetadataFilter)
        assert len(proto_filter.clauses) == 0
        assert proto_filter.op == entity_pb2.AND

    def test_search_params_with_none_values(self):
        """Test SearchParams with None values (should be skipped)"""
        from proximadb_sdk.v1 import vector_types_pb2

        params = build_search_params_grpc(
            top_k=None,
            accuracy_threshold=None,
            enable_two_stage=None
        )

        assert isinstance(params, vector_types_pb2.SearchParams)
        # Fields should not be set when None
        assert not params.HasField('top_k')
        assert not params.HasField('accuracy_threshold')
        assert not params.HasField('enable_two_stage')

    def test_search_params_empty_custom_hints(self):
        """Test SearchParams with empty custom hints"""
        from proximadb_sdk.v1 import vector_types_pb2

        params = build_search_params_grpc(custom_hints={})

        assert isinstance(params, vector_types_pb2.SearchParams)
        assert len(params.custom_hints) == 0
