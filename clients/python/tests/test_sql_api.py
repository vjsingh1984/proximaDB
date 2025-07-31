"""
Tests for SQL API functionality via REST
"""

import pytest
import numpy as np
import json
from proximadb import connect_rest, ProximaDBError
from proximadb.models import CollectionConfig, StorageEngine, VectorRecord


class TestSqlApi:
    """Test SQL query execution via REST API"""
    
    @pytest.fixture
    def client(self, rest_client):
        """Use the shared REST client for testing"""
        return rest_client
    
    def _unwrap_result(self, result):
        """Unwrap SQL result if it's wrapped in a response object"""
        if isinstance(result, dict) and "data" in result:
            return result["data"]
        return result
    
    @pytest.fixture
    def test_collection(self, client):
        """Create a test collection with sample data"""
        collection_name = "test_sql_collection"
        
        # Delete if exists
        try:
            client.delete_collection(collection_name)
        except:
            pass
        
        # Create collection
        collection = client.create_collection(
            name=collection_name,
            dimension=128,
            storage_engine=StorageEngine.SST
        )
        
        # Insert test vectors with metadata
        vectors = []
        for i in range(10):
            vector = np.random.rand(128).tolist()
            vectors.append(VectorRecord(
                id=f"vec_{i}",
                vector=vector,
                metadata={
                    "category": "electronics" if i < 5 else "books",
                    "price": float(i * 10 + 50),
                    "in_stock": i % 2 == 0,
                    "name": f"Product {i}"
                }
            ))
        
        client.insert_vectors(collection_name, vectors)
        
        # Return the collection name for SQL queries (server now resolves to UUID)
        yield collection_name
        
        # Cleanup
        try:
            client.delete_collection(collection_name)
        except:
            pass
    
    def test_basic_vector_similarity_query(self, client, test_collection):
        """Test basic vector similarity SQL query"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id, metadata
        FROM {test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 5
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)
        
        assert "rows" in actual_result
        assert "row_count" in actual_result
        assert actual_result["row_count"] == 5
        assert len(actual_result["rows"]) == 5
        
        # Check each row has expected fields
        for row in actual_result["rows"]:
            assert "id" in row
            assert "metadata" in row
            assert row["id"].startswith("vec_")
    
    def test_vector_similarity_with_metadata_filter(self, client, test_collection):
        """Test vector similarity query with metadata filtering"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id, metadata.category, metadata.price
        FROM {test_collection}
        WHERE metadata.category = 'electronics'
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 3
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)
        
        assert actual_result["row_count"] <= 3
        
        # Verify all results have electronics category
        for row in actual_result["rows"]:
            assert row.get("metadata.category") == "electronics" or \
                   (isinstance(row.get("metadata"), dict) and row["metadata"].get("category") == "electronics")
    
    def test_different_distance_metrics(self, client, test_collection):
        """Test different distance metrics in vector similarity"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        # Test cosine similarity
        sql_cosine = f"""
        SELECT id FROM {test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 3
        """
        result_cosine = client.execute_sql(sql_cosine)
        actual_cosine = self._unwrap_result(result_cosine)
        assert actual_cosine["row_count"] == 3
        
        # Test euclidean distance
        sql_euclidean = f"""
        SELECT id FROM {test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'euclidean')
        LIMIT 3
        """
        result_euclidean = client.execute_sql(sql_euclidean)
        actual_euclidean = self._unwrap_result(result_euclidean)
        assert actual_euclidean["row_count"] == 3
        
        # Results might be different due to different metrics
        ids_cosine = [row["id"] for row in actual_cosine["rows"]]
        ids_euclidean = [row["id"] for row in actual_euclidean["rows"]]
        # Can't guarantee order is different, but both should return valid results
        assert all(id.startswith("vec_") for id in ids_cosine)
        assert all(id.startswith("vec_") for id in ids_euclidean)
    
    def test_complex_metadata_conditions(self, client, test_collection):
        """Test complex metadata filtering conditions"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id, metadata.price, metadata.in_stock
        FROM {test_collection}
        WHERE metadata.price > 80 AND metadata.in_stock = true
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 10
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)
        
        # Verify conditions are met
        for row in actual_result["rows"]:
            if isinstance(row.get("metadata"), dict):
                assert row["metadata"]["price"] > 80
                assert row["metadata"]["in_stock"] is True
            else:
                # Flattened metadata
                assert row.get("metadata.price", 0) > 80
                assert row.get("metadata.in_stock") is True
    
    def test_select_specific_fields(self, client, test_collection):
        """Test selecting specific fields including vector"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id, vector, metadata.name
        FROM {test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 2
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)
        
        assert actual_result["row_count"] == 2
        
        for row in actual_result["rows"]:
            assert "id" in row
            assert "vector" in row
            assert isinstance(row["vector"], list)
            assert len(row["vector"]) == 128
            # Check for metadata.name field
            assert "metadata.name" in row or (isinstance(row.get("metadata"), dict) and "name" in row["metadata"])
    
    def test_offset_and_limit(self, client, test_collection):
        """Test OFFSET and LIMIT clauses"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        # First query without offset
        sql1 = f"""
        SELECT id
        FROM {test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 5
        """
        result1 = client.execute_sql(sql1)
        actual1 = self._unwrap_result(result1)
        ids1 = [row["id"] for row in actual1["rows"]]
        
        # Second query with offset
        sql2 = f"""
        SELECT id
        FROM {test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 5 OFFSET 3
        """
        result2 = client.execute_sql(sql2)
        actual2 = self._unwrap_result(result2)
        ids2 = [row["id"] for row in actual2["rows"]]
        
        # Check that offset worked correctly
        assert ids1[3] == ids2[0]  # 4th item from first query should be 1st in second
        assert ids1[4] == ids2[1]  # 5th item from first query should be 2nd in second
    
    def test_collection_hint_parameter(self, client, test_collection):
        """Test using collection parameter hint"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        # Query without FROM clause, using collection parameter
        sql = f"""
        SELECT id, metadata
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 3
        """
        
        result = client.execute_sql(sql, collection=test_collection)
        actual_result = self._unwrap_result(result)
        
        assert actual_result["row_count"] == 3
        assert len(actual_result["rows"]) == 3
    
    def test_invalid_query_syntax(self, client, test_collection):
        """Test error handling for invalid SQL syntax"""
        with pytest.raises(Exception) as exc_info:
            client.execute_sql("INVALID SQL QUERY")
        
        # Should get a meaningful error
        assert "parse" in str(exc_info.value).lower() or "sql" in str(exc_info.value).lower()
    
    def test_nonexistent_collection(self, client):
        """Test querying non-existent collection"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id
        FROM nonexistent_collection
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 5
        """
        
        # Try executing the SQL - it might return an error response instead of raising
        try:
            result = client.execute_sql(sql)
            # If it doesn't raise, check if it returns an error response
            if isinstance(result, dict):
                if "error" in result or "error_message" in result:
                    error_msg = result.get("error") or result.get("error_message", "")
                    assert "not found" in str(error_msg).lower() or "collection" in str(error_msg).lower()
                else:
                    # If no error in response, the test should fail
                    pytest.fail("Expected error for non-existent collection, but query succeeded")
            else:
                pytest.fail("Expected error for non-existent collection, but query succeeded")
        except Exception as e:
            # If it does raise an exception, that's also fine
            assert "not found" in str(e).lower() or "collection" in str(e).lower()
    
    def test_empty_result_set(self, client, test_collection):
        """Test query that returns no results"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        # Query with impossible condition
        sql = f"""
        SELECT id, metadata
        FROM {test_collection}
        WHERE metadata.category = 'nonexistent_category'
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine')
        LIMIT 10
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)
        
        assert actual_result["row_count"] == 0
        assert len(actual_result["rows"]) == 0
    
    def test_all_distance_metrics(self, client, test_collection):
        """Test all supported distance metrics"""
        query_vector = np.random.rand(128).tolist()
        query_vector_str = json.dumps(query_vector)
        
        metrics = ['cosine', 'euclidean', 'manhattan', 'dot']
        
        for metric in metrics:
            sql = f"""
            SELECT id
            FROM {test_collection}
            ORDER BY VECTOR_SIMILARITY(vector, {query_vector_str}, '{metric}')
            LIMIT 2
            """
            
            result = client.execute_sql(sql)
            actual_result = self._unwrap_result(result)
            assert actual_result["row_count"] == 2, f"Failed for metric: {metric}"
            assert len(actual_result["rows"]) == 2, f"Failed for metric: {metric}"
    
    def test_metadata_only_query(self, client, test_collection):
        """Test metadata-only query without vector search"""
        # This might not be supported yet, but test the behavior
        sql = f"""
        SELECT id, metadata
        FROM {test_collection}
        WHERE metadata.category = 'books'
        LIMIT 5
        """
        
        try:
            result = client.execute_sql(sql)
            actual_result = self._unwrap_result(result)
            # If supported, verify results
            if actual_result["row_count"] > 0:
                for row in actual_result["rows"]:
                    metadata = row.get("metadata", {})
                    if isinstance(metadata, dict):
                        assert metadata.get("category") == "books"
        except Exception as e:
            # If not supported, should get reasonable error
            assert "vector" in str(e).lower() or "not supported" in str(e).lower()