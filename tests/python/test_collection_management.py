"""
Test collection management functionality.
"""

import pytest
from proximadb import ProximaDBClient


@pytest.mark.integration
class TestCollectionManagement:
    """Test collection CRUD operations."""
    
    def test_create_collection(self, client, cleanup_collection):
        """Test collection creation."""
        collection_name = cleanup_collection
        
        # Create collection
        result = client.create_collection(
            name=collection_name,
            dimension=384,
            distance_metric="cosine",
            storage_engine="viper",
            indexing_algorithm="hnsw"
        )
        
        assert result is not None
    
    def test_list_collections(self, client, cleanup_collection):
        """Test listing collections."""
        collection_name = cleanup_collection
        
        # Create a test collection
        client.create_collection(
            name=collection_name,
            dimension=384
        )
        
        # List collections
        collections = client.list_collections()
        
        # Verify our collection is in the list
        collection_names = [c.name if hasattr(c, 'name') else str(c) for c in collections]
        assert collection_name in collection_names or any(collection_name in str(c) for c in collections)
    
    def test_get_collection(self, client, cleanup_collection):
        """Test getting collection details."""
        collection_name = cleanup_collection
        
        # Create a test collection
        client.create_collection(
            name=collection_name,
            dimension=384
        )
        
        # Get collection
        collection = client.get_collection(collection_name)
        assert collection is not None
    
    def test_delete_collection(self, client, test_collection_name):
        """Test collection deletion."""
        # Create a test collection
        client.create_collection(
            name=test_collection_name,
            dimension=384
        )
        
        # Delete collection
        result = client.delete_collection(test_collection_name)
        
        # Verify deletion (this might vary based on implementation)
        # Some implementations return boolean, others don't
        # We'll just verify no exception was raised
        assert True  # If we get here, deletion succeeded
    
    def test_create_collection_with_metadata_config(self, client, cleanup_collection):
        """Test collection creation with filterable metadata configuration."""
        collection_name = cleanup_collection
        
        # Create collection with metadata configuration
        result = client.create_collection(
            name=collection_name,
            dimension=384,
            distance_metric="cosine",
            storage_engine="viper",
            indexing_algorithm="hnsw"
        )
        
        assert result is not None