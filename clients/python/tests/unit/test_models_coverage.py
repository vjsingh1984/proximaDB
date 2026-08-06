"""
Test suite to improve models.py coverage to 100%
"""

import pytest

from proximadb_sdk import CollectionConfig, VectorRecord


class TestCollectionConfigEdgeCases:
    """Test edge cases in CollectionConfig"""

    def test_collection_name_validation_edge_case(self):
        """Test collection name validation at boundary (8-char minimum relaxed, #1113)."""
        # 8-character names work
        config = CollectionConfig(name="testcoll", dimension=128)
        assert config.name == "testcoll"

        # Short (7-char) names are now accepted — the 8-char minimum was relaxed
        # for relational-DDL tables; the SDK must not be stricter than the server.
        assert CollectionConfig(name="test123", dimension=128).name == "test123"

        # Empty names are still rejected (Pydantic min_length=1)
        with pytest.raises(Exception):
            CollectionConfig(name="", dimension=128)

    def test_index_config_property_none(self):
        """Test index_config property when no configs exist"""
        config = CollectionConfig(
            name="test_collection", dimension=128, index_configs=[]  # Empty list
        )
        assert config.index_config is None

        # Also test with None
        config2 = CollectionConfig(
            name="test_collection2", dimension=128, index_configs=None
        )
        assert config2.index_config is None


class TestVectorRecordEdgeCases:
    """Test edge cases in VectorRecord"""

    def test_vector_validation_non_numeric(self):
        """Test vector validation with non-numeric values"""
        # Test with string in vector
        with pytest.raises(Exception, match="Input should be a valid number"):
            VectorRecord(
                id="vec1",
                vector=[0.1, 0.2, "invalid"],  # Invalid string value
                metadata={},
            )

        # Test with None in vector
        with pytest.raises(Exception, match="Input should be a valid number"):
            VectorRecord(id="vec2", vector=[0.1, 0.2, None], metadata={})  # None value

        # Test with complex number
        with pytest.raises(Exception, match="Input should be a valid number"):
            VectorRecord(
                id="vec3",
                vector=[0.1, 0.2, complex(1, 2)],  # Complex number
                metadata={},
            )
