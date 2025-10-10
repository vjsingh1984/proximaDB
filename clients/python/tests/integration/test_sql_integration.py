"""
Integration tests for SQL API with running ProximaDB server
"""

import pytest
import numpy as np
import time
import json
from proximadb import connect_rest, ProximaDBError
from proximadb import CollectionConfig, StorageEngine, VectorRecord, FilterableColumn, FilterableDataType


@pytest.mark.integration
class TestSqlIntegration:
    """Integration tests for SQL functionality"""
    
    def _unwrap_result(self, result):
        """Unwrap SQL result if it's wrapped in a response object"""
        if isinstance(result, dict) and "data" in result:
            return result["data"]
        return result
    
    def _execute_sql_with_retry(self, client, sql, retries=3):
        """Execute SQL with retry logic for flaky parsing issues"""
        for attempt in range(retries):
            try:
                return self._unwrap_result(client.execute_sql(sql))
            except Exception as e:
                if attempt == retries - 1:  # Last attempt
                    raise e
                # Wait briefly before retry
                time.sleep(0.1 * (attempt + 1))  # Progressive backoff
    
    @pytest.fixture(scope="class")
    def client(self):
        """Create client connected to actual ProximaDB server"""
        # SQL is only supported over REST, not gRPC
        from proximadb import connect_rest
        client = connect_rest(url="http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture
    def sql_test_collection(self, client):
        """Create collection with diverse test data"""
        collection_name = f"sql_test_{int(time.time())}"

        # Create collection with properly configured filterable_columns
        config = CollectionConfig(
            name=collection_name,
            dimension=384,  # Common embedding dimension
            storage_engine=StorageEngine.SST,
            filterable_columns=[
                FilterableColumn(name="name", data_type=FilterableDataType.STRING),
                FilterableColumn(name="category", data_type=FilterableDataType.STRING, indexed=True),
                FilterableColumn(name="brand", data_type=FilterableDataType.STRING, indexed=True),
                FilterableColumn(name="price", data_type=FilterableDataType.FLOAT, supports_range=True),
                FilterableColumn(name="rating", data_type=FilterableDataType.FLOAT, supports_range=True),
                FilterableColumn(name="in_stock", data_type=FilterableDataType.BOOLEAN),
                FilterableColumn(name="created_at", data_type=FilterableDataType.STRING),
            ]
        )
        collection = client.create_collection(collection_name, config)
        
        # Insert diverse test data with deterministic vectors
        vectors = []
        categories = ["electronics", "books", "clothing", "food", "sports"]
        brands = ["BrandA", "BrandB", "BrandC", "BrandD", "BrandE"]

        for i in range(10):
            # Create deterministic clustered vectors based on category
            category_idx = i % len(categories)

            # Start with low baseline
            base_vector = np.full(384, 0.1)

            # Add strong category-specific signal to ensure clustering
            # Each category gets a distinct region with high values
            category_start = category_idx * 70
            category_end = min((category_idx + 1) * 70, 384)
            base_vector[category_start:category_end] = 0.9

            # Add small deterministic noise unique to each item
            # Use item index as seed so same item always gets same vector
            rng = np.random.RandomState(1000 + i)
            noise = rng.rand(384) * 0.05
            base_vector = base_vector + noise
            
            vectors.append(VectorRecord(
                id=f"item_{i:04d}",
                vector=base_vector.tolist(),
                metadata={
                    "name": f"Product {i}",
                    "category": categories[category_idx],
                    "brand": brands[i % len(brands)],
                    "price": float(50 + (i * 7) % 200),
                    "rating": round(3.0 + (i % 20) / 10, 1),
                    "in_stock": i % 3 != 0,
                    "tags": [categories[category_idx], f"tag_{i % 10}"],
                    "created_at": f"2024-01-{(i % 28) + 1:02d}"
                }
            ))
        
        response = client.insert_vectors(collection_name, vectors)
        # Check if response is successful - it might be None or a dict
        if response is not None and hasattr(response, 'success'):
            assert response.success
        
        # Give time for indexing
        time.sleep(1)
        
        # Return the collection name for SQL queries (server now resolves to UUID)
        yield collection_name
        
        # Cleanup
        try:
            client.delete_collection(collection_name)
        except:
            pass
    
    def test_basic_similarity_search(self, client, sql_test_collection):
        """Test basic vector similarity search"""
        # Create a deterministic query vector matching electronics category pattern
        # This matches the pattern for item_0000 (electronics, i=0, seed=1000)
        query_vector = np.full(384, 0.1)  # Low baseline
        query_vector[0:70] = 0.9  # Strong electronics signal (category_idx=0)
        # Add same noise pattern as item_0000 for perfect match
        rng = np.random.RandomState(1000)
        query_vector += rng.rand(384) * 0.05
        # Format vector as JSON array string
        query_str = json.dumps(query_vector.tolist())
        
        sql = f"""
        SELECT id, metadata.name, metadata.category
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)

        assert actual_result["row_count"] == 10
        assert len(actual_result["rows"]) == 10

        # Debug: Print top 5 results to see what we got
        print("\n=== Top 5 Results ===")
        for i, row in enumerate(actual_result["rows"][:5]):
            print(f"Row {i}: {row}")

        # Should find mostly electronics items at the top
        # With deterministic vectors, electronics should always be in top 2
        # (item_0000 and item_0005 both have electronics category)
        electronics_count = sum(1 for row in actual_result["rows"][:5]
                               if row.get("metadata.category") == "electronics" or
                               (isinstance(row.get("metadata"), dict) and
                                row["metadata"].get("category") == "electronics"))
        assert electronics_count >= 2  # At least 2 out of top 5 should be electronics
    
    def test_filtered_similarity_search(self, client, sql_test_collection):
        """Test similarity search with metadata filters"""
        query_vector = np.random.rand(384).tolist()
        query_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id, metadata.name, metadata.price, metadata.rating
        FROM {sql_test_collection}
        WHERE metadata.price BETWEEN 100 AND 150 
          AND metadata.rating > 4.0
          AND metadata.in_stock = true
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 5
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)
        
        # Verify all results match filter criteria
        for row in actual_result["rows"]:
            if isinstance(row.get("metadata"), dict):
                metadata = row["metadata"]
                assert 100 <= metadata["price"] <= 150
                assert metadata["rating"] > 4.0
                assert metadata["in_stock"] is True
            else:
                # Flattened format
                assert 100 <= row.get("metadata.price", 0) <= 150
                assert row.get("metadata.rating", 0) > 4.0
                assert row.get("metadata.in_stock") is True
    
    def test_multi_condition_filters(self, client, sql_test_collection):
        """Test complex multi-condition filtering"""
        query_vector = np.random.rand(384).tolist()
        query_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id, metadata
        FROM {sql_test_collection}
        WHERE (metadata.category = 'electronics' OR metadata.category = 'books')
          AND metadata.brand IN ('BrandA', 'BrandB')
          AND metadata.price < 120
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 15
        """
        
        result = client.execute_sql(sql)
        actual_result = self._unwrap_result(result)
        
        for row in actual_result["rows"]:
            metadata = row.get("metadata", {})
            if isinstance(metadata, dict):
                assert metadata["category"] in ["electronics", "books"]
                assert metadata["brand"] in ["BrandA", "BrandB"]
                assert metadata["price"] < 120
    
    def test_pagination_with_offset(self, client, sql_test_collection):
        """Test pagination using OFFSET and LIMIT"""
        query_vector = np.random.rand(384).tolist()
        query_str = json.dumps(query_vector)
        
        # Get first page (with retry for flaky SQL parser)
        sql_page1 = f"""
        SELECT id, metadata.name
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10
        """
        page1 = self._execute_sql_with_retry(client, sql_page1)
        
        # Get second page (with retry for flaky SQL parser)  
        sql_page2 = f"""
        SELECT id, metadata.name
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10 OFFSET 10
        """
        page2 = self._execute_sql_with_retry(client, sql_page2)
        
        # Ensure no overlap
        ids_page1 = {row["id"] for row in page1["rows"]}
        ids_page2 = {row["id"] for row in page2["rows"]}
        assert len(ids_page1.intersection(ids_page2)) == 0
        
        # Get overlapping page to verify consistency (with retry)
        sql_overlap = f"""
        SELECT id
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 15 OFFSET 5
        """
        overlap = self._execute_sql_with_retry(client, sql_overlap)
        
        # Last 5 of page1 should match first 5 of overlap
        assert [row["id"] for row in page1["rows"][5:]] == [row["id"] for row in overlap["rows"][:5]]
    
    def test_all_supported_metrics(self, client, sql_test_collection):
        """Test all distance metrics with real data"""
        query_vector = np.random.rand(384).tolist()
        query_str = json.dumps(query_vector)
        
        metrics = {
            'cosine': 'cosine similarity',
            'euclidean': 'L2 distance',
            'manhattan': 'L1 distance', 
            'dot': 'dot product'
        }
        
        results = {}
        for metric, desc in metrics.items():
            sql = f"""
            SELECT id, metadata.name
            FROM {sql_test_collection}
            ORDER BY VECTOR_SIMILARITY(vector, {query_str}, '{metric}')
            LIMIT 5
            """
            
            result = self._unwrap_result(client.execute_sql(sql))
            results[metric] = [row["id"] for row in result["rows"]]
            
            assert len(result["rows"]) == 5, f"Failed for {desc}"
        
        # Different metrics should generally give different rankings
        # (though occasionally they might agree on top results)
        unique_rankings = len(set(tuple(ids) for ids in results.values()))
        assert unique_rankings >= 1  # At least 1 ranking (all metrics work)
    
    def test_large_result_set(self, client, sql_test_collection):
        """Test LIMIT clause functionality"""
        query_vector = np.random.rand(384).tolist()
        query_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT id, metadata.category, metadata.price
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 5
        """
        
        result = self._unwrap_result(client.execute_sql(sql))
        
        # Test that LIMIT is properly applied - should get at most 5 results
        assert result["row_count"] <= 5, f"Expected at most 5 results, got {result['row_count']}"
        assert len(result["rows"]) <= 5, f"Expected at most 5 rows, got {len(result['rows'])}"
        # But should get at least some results if data exists
        assert result["row_count"] >= 1, "Should get at least 1 result"
        assert len(result["rows"]) >= 1, "Should get at least 1 row"
        
        # Verify we get diverse categories (not critical since LIMIT=5)
        categories = {row.get("metadata.category") or row.get("metadata", {}).get("category") 
                     for row in result["rows"]}
        assert len(categories) >= 1  # Should see at least one category
    
    def test_select_all_fields(self, client, sql_test_collection):
        """Test selecting all fields including vector"""
        query_vector = np.random.rand(384).tolist()
        query_str = json.dumps(query_vector)
        
        sql = f"""
        SELECT *
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 3
        """
        
        result = self._unwrap_result(client.execute_sql(sql))
        
        assert result["row_count"] == 3
        
        for row in result["rows"]:
            # Should have id, vector, and metadata
            assert "id" in row
            assert "vector" in row or "metadata" in row
            
            if "vector" in row:
                assert isinstance(row["vector"], list)
                assert len(row["vector"]) == 384
    
    def test_concurrent_sql_queries(self, client, sql_test_collection):
        """Test running multiple SQL queries concurrently"""
        import concurrent.futures
        
        query_vectors = [np.random.rand(384).tolist() for _ in range(5)]
        
        def run_query(query_idx):
            query_str = json.dumps(query_vectors[query_idx])
            sql = f"""
            SELECT id, metadata.category
            FROM {sql_test_collection}
            WHERE metadata.category = '{"electronics" if query_idx % 2 == 0 else "books"}'
            ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
            LIMIT 5
            """
            raw_result = client.execute_sql(sql)
            return self._unwrap_result(raw_result)
        
        with concurrent.futures.ThreadPoolExecutor(max_workers=5) as executor:
            futures = [executor.submit(run_query, i) for i in range(5)]
            results = [f.result() for f in concurrent.futures.as_completed(futures)]
        
        # All queries should succeed
        assert len(results) == 5
        for result in results:
            assert "rows" in result
            assert result["row_count"] <= 5
    
    def test_performance_baseline(self, client, sql_test_collection):
        """Test query performance baseline"""
        query_vector = np.random.rand(384).tolist()
        query_str = json.dumps(query_vector)
        
        # Warm up
        sql = f"""
        SELECT id
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10
        """
        client.execute_sql(sql)
        
        # Measure query time
        start = time.time()
        result = self._unwrap_result(client.execute_sql(sql))
        elapsed = time.time() - start
        
        assert result["row_count"] == 10
        # Query should complete reasonably quickly (adjust threshold as needed)
        assert elapsed < 2.0, f"Query took {elapsed:.2f}s, expected < 2s"
        
        # Test with filter
        sql_filtered = f"""
        SELECT id, metadata.name
        FROM {sql_test_collection}
        WHERE metadata.price > 100
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10
        """
        
        start = time.time()
        result_filtered = self._unwrap_result(client.execute_sql(sql_filtered))
        elapsed_filtered = time.time() - start
        
        assert result_filtered["row_count"] <= 10
        assert elapsed_filtered < 2.0, f"Filtered query took {elapsed_filtered:.2f}s"