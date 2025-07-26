"""
Integration tests for SQL API with running ProximaDB server
"""

import pytest
import numpy as np
import time
from proximadb import connect, ProximaDBError
from proximadb.models import CollectionConfig, StorageEngine


@pytest.mark.integration
class TestSqlIntegration:
    """Integration tests for SQL functionality"""
    
    @pytest.fixture(scope="class")
    def client(self):
        """Create client connected to actual ProximaDB server"""
        # Try to connect to local server
        client = connect(url="http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture
    def sql_test_collection(self, client):
        """Create collection with diverse test data"""
        collection_name = f"sql_test_{int(time.time())}"
        
        # Create collection
        config = CollectionConfig(
            name=collection_name,
            dimension=384,  # Common embedding dimension
            storage_engine=StorageEngine.LSM
        )
        client.create_collection(config)
        
        # Insert diverse test data
        vectors = []
        categories = ["electronics", "books", "clothing", "food", "sports"]
        brands = ["BrandA", "BrandB", "BrandC", "BrandD", "BrandE"]
        
        for i in range(50):
            # Create somewhat clustered vectors based on category
            base_vector = np.random.rand(384)
            category_idx = i % len(categories)
            # Add category-specific bias to create clusters
            base_vector[category_idx * 70:(category_idx + 1) * 70] += 0.5
            
            vectors.append({
                "id": f"item_{i:04d}",
                "vector": base_vector.tolist(),
                "metadata": {
                    "name": f"Product {i}",
                    "category": categories[category_idx],
                    "brand": brands[i % len(brands)],
                    "price": float(50 + (i * 7) % 200),
                    "rating": round(3.0 + (i % 20) / 10, 1),
                    "in_stock": i % 3 != 0,
                    "tags": [categories[category_idx], f"tag_{i % 10}"],
                    "created_at": f"2024-01-{(i % 28) + 1:02d}"
                }
            })
        
        response = client.insert_vectors(collection_name, vectors)
        assert response.success
        
        # Give time for indexing
        time.sleep(1)
        
        yield collection_name
        
        # Cleanup
        try:
            client.delete_collection(collection_name)
        except:
            pass
    
    def test_basic_similarity_search(self, client, sql_test_collection):
        """Test basic vector similarity search"""
        # Create a query vector similar to electronics category
        query_vector = np.random.rand(384)
        query_vector[0:70] += 0.5  # Electronics bias
        query_str = str(query_vector.tolist())
        
        sql = f"""
        SELECT id, metadata.name, metadata.category
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10
        """
        
        result = client.execute_sql(sql)
        
        assert result["row_count"] == 10
        assert len(result["rows"]) == 10
        
        # Should find mostly electronics items at the top
        electronics_count = sum(1 for row in result["rows"][:5] 
                               if row.get("metadata.category") == "electronics" or 
                               (isinstance(row.get("metadata"), dict) and 
                                row["metadata"].get("category") == "electronics"))
        assert electronics_count >= 3  # At least 3 out of top 5 should be electronics
    
    def test_filtered_similarity_search(self, client, sql_test_collection):
        """Test similarity search with metadata filters"""
        query_vector = np.random.rand(384).tolist()
        query_str = str(query_vector)
        
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
        
        # Verify all results match filter criteria
        for row in result["rows"]:
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
        query_str = str(query_vector)
        
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
        
        for row in result["rows"]:
            metadata = row.get("metadata", {})
            if isinstance(metadata, dict):
                assert metadata["category"] in ["electronics", "books"]
                assert metadata["brand"] in ["BrandA", "BrandB"]
                assert metadata["price"] < 120
    
    def test_pagination_with_offset(self, client, sql_test_collection):
        """Test pagination using OFFSET and LIMIT"""
        query_vector = np.random.rand(384).tolist()
        query_str = str(query_vector)
        
        # Get first page
        sql_page1 = f"""
        SELECT id, metadata.name
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10
        """
        page1 = client.execute_sql(sql_page1)
        
        # Get second page
        sql_page2 = f"""
        SELECT id, metadata.name
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 10 OFFSET 10
        """
        page2 = client.execute_sql(sql_page2)
        
        # Ensure no overlap
        ids_page1 = {row["id"] for row in page1["rows"]}
        ids_page2 = {row["id"] for row in page2["rows"]}
        assert len(ids_page1.intersection(ids_page2)) == 0
        
        # Get overlapping page to verify consistency
        sql_overlap = f"""
        SELECT id
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 15 OFFSET 5
        """
        overlap = client.execute_sql(sql_overlap)
        
        # Last 5 of page1 should match first 5 of overlap
        assert [row["id"] for row in page1["rows"][5:]] == [row["id"] for row in overlap["rows"][:5]]
    
    def test_all_supported_metrics(self, client, sql_test_collection):
        """Test all distance metrics with real data"""
        query_vector = np.random.rand(384).tolist()
        query_str = str(query_vector)
        
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
            WHERE metadata.in_stock = true
            ORDER BY VECTOR_SIMILARITY(vector, {query_str}, '{metric}')
            LIMIT 5
            """
            
            result = client.execute_sql(sql)
            results[metric] = [row["id"] for row in result["rows"]]
            
            assert len(result["rows"]) == 5, f"Failed for {desc}"
        
        # Different metrics should generally give different rankings
        # (though occasionally they might agree on top results)
        unique_rankings = len(set(tuple(ids) for ids in results.values()))
        assert unique_rankings >= 2  # At least 2 different rankings
    
    def test_large_result_set(self, client, sql_test_collection):
        """Test handling larger result sets"""
        query_vector = np.random.rand(384).tolist()
        query_str = str(query_vector)
        
        sql = f"""
        SELECT id, metadata.category, metadata.price
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 40
        """
        
        result = client.execute_sql(sql)
        
        assert result["row_count"] == 40
        assert len(result["rows"]) == 40
        
        # Verify all categories are represented
        categories = {row.get("metadata.category") or row.get("metadata", {}).get("category") 
                     for row in result["rows"]}
        assert len(categories) >= 4  # Should see multiple categories
    
    def test_select_all_fields(self, client, sql_test_collection):
        """Test selecting all fields including vector"""
        query_vector = np.random.rand(384).tolist()
        query_str = str(query_vector)
        
        sql = f"""
        SELECT *
        FROM {sql_test_collection}
        ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
        LIMIT 3
        """
        
        result = client.execute_sql(sql)
        
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
            query_str = str(query_vectors[query_idx])
            sql = f"""
            SELECT id, metadata.category
            FROM {sql_test_collection}
            WHERE metadata.category = '{"electronics" if query_idx % 2 == 0 else "books"}'
            ORDER BY VECTOR_SIMILARITY(vector, {query_str}, 'cosine')
            LIMIT 5
            """
            return client.execute_sql(sql)
        
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
        query_str = str(query_vector)
        
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
        result = client.execute_sql(sql)
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
        result_filtered = client.execute_sql(sql_filtered)
        elapsed_filtered = time.time() - start
        
        assert result_filtered["row_count"] <= 10
        assert elapsed_filtered < 2.0, f"Filtered query took {elapsed_filtered:.2f}s"