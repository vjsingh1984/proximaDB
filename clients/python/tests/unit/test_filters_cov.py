"""Offline unit tests for proximadb_sdk.filters.

Pure, no network/server/model. Exercises every filter type, dict + proto
serialization, combination/operator logic, nested flattening, and value-type
coercion paths in FilterClause.
"""

import warnings

import pytest

from proximadb_sdk.filters import (
    FilterBuilder,
    FilterCondition,
    FilterGroup,
    FilterOp,
    LogicalOp,
    and_filters,
    eq,
    gt,
    in_list,
    lt,
    or_filters,
)


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------
def test_filter_op_values():
    assert FilterOp.EQUALS.value == "equals"
    assert FilterOp.NOT_EQUALS.value == "not_equals"
    assert FilterOp.GREATER_THAN.value == "gt"
    assert FilterOp.GREATER_THAN_OR_EQUAL.value == "gte"
    assert FilterOp.LESS_THAN.value == "lt"
    assert FilterOp.LESS_THAN_OR_EQUAL.value == "lte"
    assert FilterOp.IN.value == "in"
    assert FilterOp.NOT_IN.value == "not_in"
    assert FilterOp.CONTAINS.value == "contains"
    assert FilterOp.NOT_CONTAINS.value == "not_contains"
    assert FilterOp.EXISTS.value == "exists"
    assert FilterOp.NOT_EXISTS.value == "not_exists"


def test_logical_op_values():
    assert LogicalOp.AND.value == "and"
    assert LogicalOp.OR.value == "or"
    assert LogicalOp.NOT.value == "not"


# ---------------------------------------------------------------------------
# FilterCondition
# ---------------------------------------------------------------------------
def test_filter_condition_to_dict():
    c = FilterCondition("category", FilterOp.EQUALS, "electronics")
    assert c.to_dict() == {
        "field": "category",
        "operation": "equals",
        "value": "electronics",
    }


def test_filter_condition_default_value():
    c = FilterCondition("foo", FilterOp.EXISTS)
    d = c.to_dict()
    assert d["field"] == "foo"
    assert d["operation"] == "exists"
    assert d["value"] is None


# ---------------------------------------------------------------------------
# FilterGroup
# ---------------------------------------------------------------------------
def test_filter_group_defaults():
    g = FilterGroup()
    assert g.operator == LogicalOp.AND
    assert g.conditions == []


def test_filter_group_add_condition_chains():
    g = FilterGroup()
    ret = g.add_condition("price", FilterOp.GREATER_THAN, 100)
    assert ret is g
    assert len(g.conditions) == 1
    assert isinstance(g.conditions[0], FilterCondition)


def test_filter_group_add_group_chains():
    parent = FilterGroup(operator=LogicalOp.OR)
    child = FilterGroup(operator=LogicalOp.AND)
    ret = parent.add_group(child)
    assert ret is parent
    assert parent.conditions[0] is child


def test_filter_group_to_dict_nested():
    inner = FilterGroup(operator=LogicalOp.OR)
    inner.add_condition("brand", FilterOp.EQUALS, "Apple")
    outer = FilterGroup(operator=LogicalOp.AND)
    outer.add_condition("category", FilterOp.EQUALS, "phones")
    outer.add_group(inner)
    d = outer.to_dict()
    assert d["operator"] == "and"
    assert len(d["conditions"]) == 2
    assert d["conditions"][0] == {
        "field": "category",
        "operation": "equals",
        "value": "phones",
    }
    assert d["conditions"][1]["operator"] == "or"
    assert d["conditions"][1]["conditions"][0]["value"] == "Apple"


# ---------------------------------------------------------------------------
# FilterBuilder — all condition methods
# ---------------------------------------------------------------------------
def test_builder_init_state():
    b = FilterBuilder()
    assert b._root.operator == LogicalOp.AND
    assert b._current_group is b._root
    assert b._group_stack == []


def test_builder_all_condition_ops():
    b = (
        FilterBuilder()
        .equals("a", 1)
        .not_equals("b", 2)
        .greater_than("c", 3)
        .gte("d", 4)
        .less_than("e", 5)
        .lte("f", 6)
        .in_("g", [1, 2])
        .not_in("h", [3, 4])
        .contains("i", "sub")
        .exists("j")
        .not_exists("k")
    )
    conds = b._root.conditions
    ops = [c.operation for c in conds]
    assert ops == [
        FilterOp.EQUALS,
        FilterOp.NOT_EQUALS,
        FilterOp.GREATER_THAN,
        FilterOp.GREATER_THAN_OR_EQUAL,
        FilterOp.LESS_THAN,
        FilterOp.LESS_THAN_OR_EQUAL,
        FilterOp.IN,
        FilterOp.NOT_IN,
        FilterOp.CONTAINS,
        FilterOp.EXISTS,
        FilterOp.NOT_EXISTS,
    ]
    # exists / not_exists carry no value
    assert conds[-1].value is None
    assert conds[-2].value is None


def test_builder_chaining_returns_self():
    b = FilterBuilder()
    assert b.equals("x", 1) is b


def test_builder_build_returns_root():
    b = FilterBuilder().equals("x", 1)
    assert b.build() is b._root


def test_builder_to_dict():
    b = FilterBuilder().equals("category", "electronics").greater_than("price", 100)
    d = b.to_dict()
    assert d["operator"] == "and"
    assert len(d["conditions"]) == 2


# ---------------------------------------------------------------------------
# Group navigation: and_ / or_ / not_ / end_group
# ---------------------------------------------------------------------------
def test_builder_or_group_navigation():
    b = FilterBuilder().or_().equals("brand", "Apple").equals("brand", "Samsung")
    # current group is the new OR group
    assert b._current_group.operator == LogicalOp.OR
    assert len(b._current_group.conditions) == 2
    # root has the OR group nested
    assert b._root.conditions[0] is b._current_group


def test_builder_and_group_navigation():
    b = FilterBuilder()
    b.and_()
    assert b._current_group.operator == LogicalOp.AND
    assert b._current_group is not b._root
    assert b._group_stack == [b._root]


def test_builder_not_group_navigation():
    b = FilterBuilder().not_().equals("status", "deleted")
    assert b._current_group.operator == LogicalOp.NOT


def test_builder_end_group_returns_to_parent():
    b = FilterBuilder()
    b.or_().equals("brand", "Apple")
    nested = b._current_group
    b.end_group()
    assert b._current_group is b._root
    assert b._current_group is not nested


def test_builder_end_group_on_empty_stack_noop():
    b = FilterBuilder()
    assert b._group_stack == []
    ret = b.end_group()
    assert ret is b
    assert b._current_group is b._root


# ---------------------------------------------------------------------------
# and_group / or_group with another builder
# ---------------------------------------------------------------------------
def test_builder_and_group_merges_other():
    other = FilterBuilder().or_().equals("brand", "Apple").equals("brand", "Samsung")
    b = FilterBuilder().equals("category", "electronics").and_group(other)
    # the merged group is the other builder's root
    assert b._root.conditions[-1] is other._root


def test_builder_or_group_sets_operator():
    other = FilterBuilder().equals("brand", "Apple")
    assert other._root.operator == LogicalOp.AND
    b = FilterBuilder().or_group(other)
    # or_group mutates other root operator to OR
    assert other._root.operator == LogicalOp.OR
    assert b._root.conditions[-1] is other._root


# ---------------------------------------------------------------------------
# Convenience functions
# ---------------------------------------------------------------------------
def test_eq_helper():
    b = eq("category", "books")
    c = b._root.conditions[0]
    assert c.operation == FilterOp.EQUALS
    assert c.value == "books"


def test_gt_helper():
    c = gt("price", 50)._root.conditions[0]
    assert c.operation == FilterOp.GREATER_THAN
    assert c.value == 50


def test_lt_helper():
    c = lt("price", 50)._root.conditions[0]
    assert c.operation == FilterOp.LESS_THAN


def test_in_list_helper():
    c = in_list("tag", ["a", "b"])._root.conditions[0]
    assert c.operation == FilterOp.IN
    assert c.value == ["a", "b"]


def test_and_filters_combines():
    f1 = eq("a", 1)
    f2 = eq("b", 2)
    combined = and_filters(f1, f2)
    # each filter root is added as a nested group
    assert combined._root.operator == LogicalOp.AND
    assert combined._root.conditions[0] is f1._root
    assert combined._root.conditions[1] is f2._root


def test_and_filters_empty():
    combined = and_filters()
    assert combined._root.conditions == []


def test_or_filters_flattens_conditions():
    f1 = eq("a", 1)
    f2 = eq("b", 2)
    combined = or_filters(f1, f2)
    # or_filters builds an OR group, then end_group returns to root
    assert combined._current_group is combined._root
    # the OR group is nested under root
    or_group = combined._root.conditions[0]
    assert isinstance(or_group, FilterGroup)
    assert or_group.operator == LogicalOp.OR
    fields = [c.field for c in or_group.conditions]
    assert fields == ["a", "b"]


def test_or_filters_with_nested_group():
    # a filter whose root contains a group (not a plain condition)
    nested = FilterBuilder().or_().equals("x", 1)
    nested.end_group()
    # nested._root.conditions[0] is a FilterGroup
    combined = or_filters(nested)
    or_group = combined._root.conditions[0]
    # the nested group should be carried over as a group
    assert any(isinstance(c, FilterGroup) for c in or_group.conditions)


# ---------------------------------------------------------------------------
# Proto serialization
# ---------------------------------------------------------------------------
def test_to_proto_basic_ops():
    from proximadb_sdk.v1 import entity_pb2

    b = (
        FilterBuilder()
        .equals("a", "x")
        .not_equals("b", "y")
        .greater_than("c", 1)
        .gte("d", 2)
        .less_than("e", 3)
        .lte("f", 4)
        .contains("g", "sub")
    )
    proto = b.to_proto_filter()
    assert proto.op == entity_pb2.AND
    assert len(proto.clauses) == 7
    ops = {cl.field: cl.op for cl in proto.clauses}
    assert ops["a"] == entity_pb2.EQ
    assert ops["b"] == entity_pb2.NE
    assert ops["c"] == entity_pb2.GT
    assert ops["d"] == entity_pb2.GTE
    assert ops["e"] == entity_pb2.LT
    assert ops["f"] == entity_pb2.LTE
    assert ops["g"] == entity_pb2.CONTAINS


def test_to_proto_in_not_in_ops():
    from proximadb_sdk.v1 import entity_pb2

    b = FilterBuilder().in_("tag", ["a", "b"]).not_in("cat", ["c"])
    proto = b.to_proto_filter()
    ops = {cl.field: cl.op for cl in proto.clauses}
    assert ops["tag"] == entity_pb2.IN
    assert ops["cat"] == entity_pb2.NOT_IN


def test_to_proto_value_types():
    b = (
        FilterBuilder()
        .equals("s", "hello")
        .equals("i", 42)
        .equals("f", 3.14)
        .equals("bt", True)
        .equals("bf", False)
    )
    proto = b.to_proto_filter()
    by_field = {cl.field: cl for cl in proto.clauses}
    assert by_field["s"].string_value == "hello"
    assert by_field["i"].int_value == 42
    assert abs(by_field["f"].double_value - 3.14) < 1e-9
    assert by_field["bt"].bool_value is True
    # False sets bool_value but it's the default; check the oneof is set
    assert by_field["bf"].WhichOneof("value") == "bool_value"


def test_to_proto_list_string_values():
    b = FilterBuilder().in_("tag", ["a", "b", "c"])
    proto = b.to_proto_filter()
    assert proto.clauses[0].string_value == "a,b,c"


def test_to_proto_list_numeric_values():
    b = FilterBuilder().in_("nums", [1, 2, 3])
    proto = b.to_proto_filter()
    assert proto.clauses[0].string_value == "1,2,3"


def test_to_proto_list_mixed_values():
    b = FilterBuilder().in_("mixed", [1, "two", 3.0])
    proto = b.to_proto_filter()
    # mixed -> str of each
    assert proto.clauses[0].string_value == "1,two,3.0"


def test_to_proto_fallback_value_type():
    # a value that's not bool/int/float/str/list -> str(value)
    b = FilterBuilder().equals("d", {"k": "v"})
    proto = b.to_proto_filter()
    assert proto.clauses[0].string_value == str({"k": "v"})


def test_to_proto_none_value_skips_value_set():
    # exists has value None -> but EXISTS is unsupported and warns/skips entirely
    b = FilterBuilder().equals("a", None)
    # equals with None value: op supported, value None -> no value set
    proto = b.to_proto_filter()
    assert len(proto.clauses) == 1
    assert proto.clauses[0].WhichOneof("value") is None


def test_to_proto_unsupported_op_warns_and_skips():
    b = FilterBuilder().exists("present").equals("ok", 1)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        proto = b.to_proto_filter()
    # exists is unsupported -> skipped + warning; equals stays
    assert len(proto.clauses) == 1
    assert proto.clauses[0].field == "ok"
    assert any("not supported" in str(w.message) for w in caught)


def test_to_proto_not_exists_and_not_contains_unsupported():
    b = FilterBuilder().not_exists("x").contains("y", "z")  # contains supported
    b._current_group.add_condition("w", FilterOp.NOT_CONTAINS, "v")
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        proto = b.to_proto_filter()
    fields = [cl.field for cl in proto.clauses]
    assert fields == ["y"]  # only contains survived
    assert len(caught) >= 2


def test_to_proto_nested_same_operator_flattens():
    from proximadb_sdk.v1 import entity_pb2

    inner = FilterBuilder().equals("b", 2).equals("c", 3)
    b = FilterBuilder().equals("a", 1).and_group(inner)
    proto = b.to_proto_filter()
    # same operator (AND) -> flattened, no warning
    assert proto.op == entity_pb2.AND
    fields = sorted(cl.field for cl in proto.clauses)
    assert fields == ["a", "b", "c"]


def test_to_proto_nested_different_operator_warns():
    inner = FilterBuilder().or_().equals("b", 2).equals("c", 3)
    inner.end_group()
    b = FilterBuilder().equals("a", 1).and_group(inner)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        proto = b.to_proto_filter()
    fields = sorted(cl.field for cl in proto.clauses)
    assert fields == ["a", "b", "c"]
    assert any("lose structure" in str(w.message) for w in caught)


def test_to_proto_or_root_operator():
    from proximadb_sdk.v1 import entity_pb2

    b = FilterBuilder().or_().equals("a", 1).equals("b", 2)
    # build proto from the root (which is AND containing an OR group);
    # to_proto_filter starts at root
    proto = b.to_proto_filter()
    # root op is AND; nested OR group has different op -> flattened with warning
    assert proto.op == entity_pb2.AND


def test_to_proto_import_error(monkeypatch):
    import builtins

    real_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "proximadb_sdk.v1" or name.startswith("proximadb_sdk.v1"):
            raise ImportError("no proto")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    b = FilterBuilder().equals("a", 1)
    with pytest.raises(ImportError, match="Proto modules not available"):
        b.to_proto_filter()


# ---------------------------------------------------------------------------
# End-to-end fluent example from the docstring
# ---------------------------------------------------------------------------
def test_complex_nested_dict_roundtrip():
    f = (
        FilterBuilder()
        .equals("category", "electronics")
        .and_group(
            FilterBuilder().or_().equals("brand", "Apple").equals("brand", "Samsung")
        )
        .greater_than("rating", 4.0)
        .build()
    )
    d = f.to_dict()
    assert d["operator"] == "and"
    assert d["conditions"][0]["field"] == "category"
    # nested group
    nested = d["conditions"][1]
    assert nested["operator"] == "and"  # the and_group root keeps AND, OR is inside
    assert d["conditions"][2]["field"] == "rating"
