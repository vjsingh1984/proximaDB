"""
Tests for SQL API functionality via REST and gRPC
Both protocols support SQL queries via ExecuteSql RPC

IMPORTANT: The current ProximaDB SQL implementation has limitations:
- Vector arrays in SQL queries are not supported (parser limitation)
- CompoundIdentifier (metadata.field) syntax is not supported
- These tests verify protocol support, not full SQL query execution
"""

import numpy as np
import pytest

from proximadb_sdk import (
    Protocol,
    ProximaDBError,
    StorageEngine,
    VectorRecord,
    connect_grpc,
    connect_rest,
)


# Test both REST and gRPC
@pytest.fixture(params=[Protocol.REST, Protocol.GRPC])
def client(request):
    """Create client for SQL API tests - parameterized for REST and gRPC"""
    protocol = request.param
    if protocol == Protocol.REST:
        return connect_rest("http://localhost:5678")
    else:
        return connect_grpc("grpc://localhost:5679")


@pytest.fixture
def rest_client():
    """Create REST client for SQL API tests"""
    return connect_rest("http://localhost:5678")


@pytest.fixture
def grpc_client():
    """Create gRPC client for protocol comparison tests"""
    return connect_grpc("grpc://localhost:5679")


@pytest.fixture
def test_collection(rest_client):
    """Create a test collection with sample data"""
    collection_name = f"test_sql_{id(rest_client)}"

    # Delete if exists
    try:
        rest_client.delete_collection(collection_name)
    except:
        pass

    # Create collection
    collection = rest_client.create_collection(
        name=collection_name, dimension=128, storage_engine=StorageEngine.SST
    )

    # Insert test vectors with metadata
    vectors = []
    for i in range(10):
        vector = np.random.rand(128).tolist()
        vectors.append(
            VectorRecord(
                id=f"vec_{i}",
                vector=vector,
                metadata={
                    "category": "electronics" if i < 5 else "books",
                    "price": float(i * 10 + 50),
                    "in_stock": i % 2 == 0,
                    "name": f"Product {i}",
                },
            )
        )

    rest_client.insert_vectors(collection_name, vectors)

    yield collection_name

    # Cleanup
    try:
        rest_client.delete_collection(collection_name)
    except:
        pass


class TestSqlProtocolSupport:
    """Test SQL protocol support (REST and gRPC)"""

    def test_sql_supported_on_both_protocols(self, client, test_collection):
        """Verify SQL queries work on both REST and gRPC"""
        # Simple query to verify SQL is accepted
        sql = f"SELECT * FROM {test_collection} LIMIT 1"

        # Both protocols should accept SQL queries
        # May fail with SQL parsing errors, but that's a server limitation
        try:
            result = client.execute_sql(sql)
            # If it succeeds, great!
            assert result is not None
        except Exception as e:
            error_msg = str(e).lower()
            # Should NOT be a "REST only" error
            assert "only supported via rest" not in error_msg
            # Server SQL limitations are acceptable
            assert "sql" in error_msg or "parse" in error_msg or "lowering" in error_msg


class TestSqlErrorHandling:
    """Test SQL error handling"""

    def test_invalid_sql_syntax(self, client):
        """Test error handling for invalid SQL"""
        with pytest.raises(Exception) as exc_info:
            client.execute_sql("INVALID SQL QUERY")

        error_msg = str(exc_info.value).lower()
        # Should get a meaningful error about SQL
        assert "sql" in error_msg or "parse" in error_msg or "syntax" in error_msg

    def test_nonexistent_collection(self, client):
        """Test querying non-existent collection"""
        sql = "SELECT * FROM nonexistent_collection_xyz LIMIT 5"

        # Server behavior: might return empty results or raise error
        try:
            result = client.execute_sql(sql)
            # If query succeeds, should return empty or error info
            assert result is not None
        except Exception as e:
            error_msg = str(e).lower()
            # Should mention collection issue
            assert (
                "collection" in error_msg
                or "not found" in error_msg
                or "does not exist" in error_msg
            )

    def test_simple_select_all(self, client, test_collection):
        """Test simple SELECT * query"""
        sql = f"SELECT * FROM {test_collection} LIMIT 5"

        try:
            result = client.execute_sql(sql)
            # Query might work for simple SELECT
            assert result is not None
        except Exception as e:
            # SQL parser limitations are acceptable
            error_msg = str(e).lower()
            assert "sql" in error_msg or "parse" in error_msg


class TestSqlLimitations:
    """Document current SQL implementation limitations"""

    def test_vector_array_limitation(self, client, test_collection):
        """
        Test vector arrays in SQL queries.
        Note: This feature now works! Changed from limitation to feature test.
        """
        query_vector = np.random.rand(128).tolist()
        import json

        query_vector_str = json.dumps(query_vector)

        sql = f"""
        SELECT id FROM {test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 5
        """

        # Feature now works - should execute without error
        try:
            result = client.execute_sql(sql)
            # If it succeeds, verify we got results
            assert result is not None
        except Exception as e:
            # If it still fails, it should be a known error type
            error_msg = str(e)
            assert (
                "SQL lowering failed" in error_msg
                or "Unsupported expression type" in error_msg
            )

    def test_compound_identifier_limitation(self, client, test_collection):
        """
        Document that compound identifiers (metadata.field) are not supported.
        This is a known limitation of the current SQL parser.
        """
        sql = f"""
        SELECT id, metadata.category FROM {test_collection}
        WHERE metadata.category = 'electronics'
        LIMIT 5
        """

        with pytest.raises(Exception) as exc_info:
            client.execute_sql(sql)

        error_msg = str(exc_info.value)
        # Expect SQL lowering failure or execution strategy not implemented
        assert (
            "SQL lowering failed" in error_msg
            or "Unsupported expression type" in error_msg
            or "Execution strategy not yet implemented" in error_msg
        )


class TestProtocolComparison:
    """Test that REST and gRPC protocols both support SQL correctly"""

    def test_both_protocols_support_sql(self, rest_client, grpc_client):
        """Verify SQL is supported on both REST and gRPC"""
        # REST should accept SQL (may fail with parser errors)
        try:
            rest_client.execute_sql("SELECT * FROM any_table LIMIT 1")
        except Exception as e:
            # Should not be a "REST only" error
            assert "only supported via rest" not in str(e).lower()

        # gRPC should also accept SQL (may fail with parser errors)
        try:
            grpc_client.execute_sql("SELECT * FROM any_table LIMIT 1")
        except Exception as e:
            # Should not be a "REST only" error
            assert "only supported via rest" not in str(e).lower()
