"""Helper functions for ProximaDB tests."""

import logging

logger = logging.getLogger(__name__)


def ensure_collection(client, collection_name: str, dimension: int = 128, distance_metric: str = "cosine", **kwargs):
    """Ensure a collection exists, deleting and recreating if necessary.
    
    Args:
        client: ProximaDB client instance
        collection_name: Name of the collection
        dimension: Vector dimension (default: 128)
        distance_metric: Distance metric (default: "cosine")
        **kwargs: Additional collection configuration parameters
    
    Returns:
        Collection object
    """
    try:
        # Check if collection exists
        collections = client.list_collections()
        collection_exists = False
        
        # Handle different response formats
        if isinstance(collections, list):
            for col in collections:
                if isinstance(col, str) and col == collection_name:
                    collection_exists = True
                    break
                elif hasattr(col, 'name') and col.name == collection_name:
                    collection_exists = True
                    break
                elif hasattr(col, 'config') and hasattr(col.config, 'name') and col.config.name == collection_name:
                    collection_exists = True
                    break
                elif hasattr(col, 'id') and col.id == collection_name:
                    collection_exists = True
                    break
        
        # Delete if exists
        if collection_exists:
            logger.debug(f"Collection '{collection_name}' exists, deleting...")
            client.delete_collection(collection_name)
            logger.debug(f"Collection '{collection_name}' deleted")
    except Exception as e:
        logger.debug(f"Error checking/deleting collection: {e}")
    
    # Create collection
    logger.debug(f"Creating collection '{collection_name}'...")
    try:
        # Try with config object first
        from proximadb import CollectionConfig
        config = CollectionConfig(
            name=collection_name,
            dimension=dimension,
            distance_metric=distance_metric,
            **kwargs
        )
        collection = client.create_collection(collection_name, config)
    except Exception:
        # Fallback to direct parameters
        collection = client.create_collection(
            collection_name,
            dimension=dimension,
            distance_metric=distance_metric,
            **kwargs
        )
    
    logger.debug(f"Collection '{collection_name}' created")
    return collection


def cleanup_collection(client, collection_name: str):
    """Clean up a collection, ignoring errors."""
    try:
        client.delete_collection(collection_name)
        logger.debug(f"Cleaned up collection '{collection_name}'")
    except Exception as e:
        logger.debug(f"Cleanup failed for '{collection_name}': {e}")


# Deterministic collection names for each test module
COLLECTION_NAMES = {
    "test_vector_operations": {
        "crud": "test_vec_ops_crud",
        "batch": "test_vec_ops_batch",
        "validation": "test_vec_ops_validation",
        "metadata": "test_vec_ops_metadata",
        "concurrent": "test_vec_ops_concurrent"
    },
    "test_search_operations": {
        "basic": "test_search_basic",
        "advanced": "test_search_advanced", 
        "filtered": "test_search_filtered",
        "sql": "test_search_sql",
        "optimization": "test_search_optimized"
    },
    "test_client_sdk": {
        "config": "test_client_config",
        "health": "test_client_health",
        "interop": "test_client_interop",
        "errors": "test_client_errors"
    },
    "test_collection_operations": {
        "basic": "test_coll_ops_basic",
        "config": "test_coll_ops_config",
        "metadata": "test_coll_ops_metadata",
        "listing": "test_coll_ops_listing"
    },
    "test_chunking": {
        "basic": "test_chunk_basic",
        "strategies": "test_chunk_strategies",
        "metadata": "test_chunk_metadata",
        "integration": "test_chunk_integration"
    }
}