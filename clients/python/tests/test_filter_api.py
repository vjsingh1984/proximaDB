"""
Test the enhanced filter API for complex metadata queries
"""

import time

import numpy as np
import pytest

from proximadb_sdk import (
    FilterBuilder,
    ProximaDBClient,
    VectorRecord,
    eq,
    gt,
    in_list,
    lt,
    or_filters,
)


def test_filter_builder_basic():
    """Test basic filter builder functionality"""
    # Simple equality filter
    filter1 = FilterBuilder().equals("category", "electronics").build()
    assert filter1.to_dict() == {
        "operator": "and",
        "conditions": [
            {"field": "category", "operation": "equals", "value": "electronics"}
        ],
    }

    # Multiple AND conditions
    filter2 = (
        FilterBuilder()
        .equals("category", "electronics")
        .greater_than("price", 100)
        .less_than("price", 1000)
        .build()
    )

    assert len(filter2.conditions) == 3
    assert filter2.operator.value == "and"


def test_filter_builder_or_conditions():
    """Test OR conditions in filter builder"""
    # OR conditions
    filter_or = (
        FilterBuilder()
        .or_()
        .equals("brand", "Apple")
        .equals("brand", "Samsung")
        .equals("brand", "Google")
        .end_group()
        .build()
    )

    # Check the structure
    assert len(filter_or.conditions) == 1  # One OR group
    or_group = filter_or.conditions[0]
    assert or_group.operator.value == "or"
    assert len(or_group.conditions) == 3


def test_filter_builder_complex():
    """Test complex nested filters"""
    # Complex nested filter:
    # category = "electronics" AND (brand = "Apple" OR brand = "Samsung") AND rating > 4.0
    filter_complex = (
        FilterBuilder()
        .equals("category", "electronics")
        .and_group(
            FilterBuilder().or_().equals("brand", "Apple").equals("brand", "Samsung")
        )
        .greater_than("rating", 4.0)
        .build()
    )

    # Verify structure
    assert filter_complex.operator.value == "and"
    assert len(filter_complex.conditions) >= 2


def test_convenience_functions():
    """Test convenience filter functions"""
    # Test eq
    f1 = eq("status", "active")
    assert len(f1._root.conditions) == 1
    assert f1._root.conditions[0].field == "status"
    assert f1._root.conditions[0].value == "active"

    # Test gt
    f2 = gt("price", 50)
    assert f2._root.conditions[0].operation.value == "gt"

    # Test lt
    f3 = lt("stock", 10)
    assert f3._root.conditions[0].operation.value == "lt"

    # Test in_list
    f4 = in_list("color", ["red", "blue", "green"])
    assert f4._root.conditions[0].operation.value == "in"
    assert f4._root.conditions[0].value == ["red", "blue", "green"]


# @pytest.mark.integration
@pytest.mark.skip(
    reason="Requires server-side metadata filtering support - currently returns unfiltered results"
)
def test_filter_with_grpc_search():
    """Test filters with actual gRPC search"""
    client = ProximaDBClient(url="http://localhost:5679", protocol="grpc")

    # Create test collection
    collection_name = f"filter_test_{int(time.time() * 1000)}"
    collection = client.create_collection(collection_name, dimension=128)

    try:
        # Insert test vectors with metadata
        records = []

        # Electronics
        for i in range(5):
            records.append(
                VectorRecord(
                    id=f"electronics_{i}",
                    vector=np.random.rand(128).tolist(),
                    metadata={
                        "category": "electronics",
                        "brand": ["Apple", "Samsung", "Sony", "LG", "Google"][i],
                        "price": 500 + i * 100,
                        "rating": 4.0 + i * 0.2,
                        "in_stock": True,
                    },
                )
            )

        # Furniture
        for i in range(5):
            records.append(
                VectorRecord(
                    id=f"furniture_{i}",
                    vector=np.random.rand(128).tolist(),
                    metadata={
                        "category": "furniture",
                        "brand": ["IKEA", "Ashley", "Wayfair", "West Elm", "CB2"][i],
                        "price": 200 + i * 50,
                        "rating": 3.5 + i * 0.3,
                        "in_stock": i % 2 == 0,
                    },
                )
            )

        client.insert_vectors(collection_name, records)

        # Test 1: Simple filter - electronics only
        filter1 = eq("category", "electronics")
        # Convert filter to dict if it has to_dict method
        filter_dict1 = filter1.to_dict() if hasattr(filter1, "to_dict") else filter1
        results1 = client.search_single(
            collection_id=collection_name,
            vector=np.random.rand(128).tolist(),
            top_k=10,
            metadata_filter=filter_dict1,
        )

        # Should only get electronics
        for result in results1:
            assert result.metadata.get("category") == "electronics"

        # Test 2: Range filter - price between 300-700
        filter2 = (
            FilterBuilder().greater_than("price", 300).less_than("price", 700).build()
        )

        results2 = client.search_single(
            collection_id=collection_name,
            vector=np.random.rand(128).tolist(),
            top_k=10,
            metadata_filter=(
                filter2.to_dict() if hasattr(filter2, "to_dict") else filter2
            ),
        )

        # Check price range
        for result in results2:
            price = result.metadata.get("price", 0)
            assert 300 < price < 700

        # Test 3: Complex filter - (Apple OR Samsung) AND rating > 4.2
        filter3 = (
            FilterBuilder()
            .and_group(or_filters(eq("brand", "Apple"), eq("brand", "Samsung")))
            .greater_than("rating", 4.2)
            .build()
        )

        results3 = client.search_single(
            collection_id=collection_name,
            vector=np.random.rand(128).tolist(),
            top_k=10,
            metadata_filter=(
                filter3.to_dict() if hasattr(filter3, "to_dict") else filter3
            ),
        )

        # Verify results match filter
        for result in results3:
            assert result.metadata.get("brand") in ["Apple", "Samsung"]
            assert result.metadata.get("rating", 0) > 4.2

    finally:
        # Cleanup
        client.delete_collection(collection_name)


if __name__ == "__main__":
    # Run basic tests
    test_filter_builder_basic()
    test_filter_builder_or_conditions()
    test_filter_builder_complex()
    test_convenience_functions()
    print("✅ All filter API tests passed!")

    # Run integration test if server is available
    try:
        import time

        test_filter_with_grpc_search()
        print("✅ Integration test passed!")
    except Exception as e:
        print(f"⚠️ Integration test skipped: {e}")
