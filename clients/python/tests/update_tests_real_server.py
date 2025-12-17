#!/usr/bin/env python3
"""
Script to update all ProximaDB Python SDK tests to use real server instead of mocks

This script:
1. Identifies all test files using mocks
2. Updates them to use real server connections
3. Provides a summary of changes
"""

import os
import re
from pathlib import Path
from typing import List, Tuple


def find_test_files_with_mocks(test_dir: Path) -> List[Path]:
    """Find all test files that use mocks"""
    mock_patterns = [
        r'from unittest\.mock import',
        r'import mock',
        r'@patch\(',
        r'MagicMock',
        r'Mock\(',
        r'AsyncMock',
        r'mock\.',
        r'patch\(',
    ]
    
    test_files = []
    for file_path in test_dir.rglob("test_*.py"):
        content = file_path.read_text()
        if any(re.search(pattern, content) for pattern in mock_patterns):
            test_files.append(file_path)
    
    return test_files


def create_real_server_template(test_type: str) -> str:
    """Create template for real server tests based on test type"""
    
    base_template = '''"""
{description}

Uses real ProximaDB server for testing.
"""

import pytest
import time
import uuid
import numpy as np
from pathlib import Path
import sys
from typing import List, Dict, Any, Optional

# Add utils to path
sys.path.insert(0, str(Path(__file__).parent.parent))
from utils.base_test import BaseProximaDBTest
from utils.server_utils import ensure_server_running

from proximadb_sdk import ProximaDBClient
from proximadb_sdk.config import ClientConfig
from proximadb_sdk.models import VectorRecord, Protocol

'''

    templates = {
        "batching": base_template + '''from proximadb_sdk.batching import (
    BatchStrategy,
    BatchOperationType, 
    BatchConfig,
    BatchRequest,
    RequestBatcher,
    BatchMetrics
)


class TestBatching(BaseProximaDBTest):
    """Test batching functionality with real server"""
    
    def test_batch_insert_vectors(self):
        """Test batch vector insertion with real server"""
        collection_name = self.create_collection()
        
        # Create test vectors
        vectors = self.insert_test_vectors(
            self.rest_client,
            collection_name,
            count=100
        )
        
        # Verify insertion
        self.wait_for_indexing()
        
        # Search to verify
        query_vector = vectors[0].vector
        results = self.rest_client.search(
            collection_name,
            query_vector,
            top_k=10
        )
        
        self.verify_search_results(results, 10)
        assert results[0]["id"] == vectors[0].id
''',

        "connection_pool": base_template + '''from proximadb_sdk.connection_pool import (
    ConnectionPool,
    GrpcConnectionPool,
    RestConnectionPool,
    PoolConfig,
    PoolMetrics
)


class TestConnectionPool(BaseProximaDBTest):
    """Test connection pooling with real server"""
    
    def test_connection_reuse(self):
        """Test connection reuse with real operations"""
        pool = ConnectionPool(
            url="http://localhost:5678",
            max_connections=5
        )
        
        # Use connections
        for i in range(10):
            with pool.get_connection() as conn:
                # Real operation
                response = conn.get("/health")
                assert response.status_code == 200
        
        # Check metrics
        metrics = pool.get_metrics()
        assert metrics.connections_created <= 5
        assert metrics.connections_reused >= 5
''',

        "semantic_chunking": base_template + '''from proximadb_sdk.semantic_chunking import (
    EnhancedSemanticChunker,
    SemanticChunkingConfig,
    create_enhanced_semantic_chunker
)
from proximadb_sdk.chunking import TextChunk, ChunkingConfig
from proximadb_sdk.embedding_interface import create_embedding_provider


class TestSemanticChunking(BaseProximaDBTest):
    """Test semantic chunking with real embeddings"""
    
    def test_semantic_chunking_with_embeddings(self):
        """Test semantic chunking using real BERT embeddings"""
        # Create embedding provider
        provider = create_embedding_provider("bert")
        
        # Create chunker
        chunker = create_enhanced_semantic_chunker(
            use_embeddings=True,
            embedding_provider=provider
        )
        
        # Test text
        text = """
        Machine learning is a subset of artificial intelligence.
        It focuses on training algorithms to make predictions.
        
        Python is a popular programming language.
        It is widely used for data science and web development.
        """
        
        # Chunk text
        chunks = chunker.chunk_semantically(
            text=text,
            source_id="test_doc",
            chunking_config=ChunkingConfig()
        )
        
        # Verify chunks
        assert len(chunks) > 0
        for chunk in chunks:
            assert isinstance(chunk, TextChunk)
            assert chunk.text
            assert chunk.metadata.get("coherence_score") is not None
'''
    }
    
    return templates.get(test_type, base_template)


def extract_test_type(file_path: Path) -> str:
    """Extract test type from filename"""
    name = file_path.stem.replace("test_", "")
    
    # Map common test names to types
    type_mapping = {
        "batching": "batching",
        "rest_batching": "batching",
        "connection_pools": "connection_pool",
        "connection_pool": "connection_pool",
        "grpc_connection": "connection_pool",
        "semantic_chunking": "semantic_chunking",
        "chunking": "semantic_chunking",
    }
    
    for key, value in type_mapping.items():
        if key in name:
            return value
    
    return "generic"


def update_test_file(file_path: Path) -> bool:
    """Update a single test file to use real server"""
    try:
        content = file_path.read_text()
        
        # Skip if already updated
        if "BaseProximaDBTest" in content and "from utils.base_test import" in content:
            print(f"✓ Already updated: {file_path.name}")
            return False
        
        # Extract existing test content
        test_type = extract_test_type(file_path)
        
        # Get description from docstring
        desc_match = re.search(r'"""(.*?)"""', content, re.DOTALL)
        description = desc_match.group(1).strip() if desc_match else f"Tests for {file_path.stem}"
        
        # Create new content
        template = create_real_server_template(test_type)
        new_content = template.format(description=description)
        
        # Extract and adapt existing test methods
        class_matches = re.findall(
            r'class (Test\w+).*?:\s*\n(.*?)(?=class|\Z)',
            content,
            re.DOTALL
        )
        
        for class_name, class_body in class_matches:
            # Extract test methods
            method_matches = re.findall(
                r'(    def test_\w+.*?\n(?:        .*\n)*)',
                class_body
            )
            
            if method_matches and test_type == "generic":
                # For generic tests, preserve structure but update to use real server
                new_class = f"\n\nclass {class_name}(BaseProximaDBTest):\n"
                new_class += '    """' + class_name.replace('Test', '') + ' tests with real server"""\n\n'
                
                for method in method_matches[:3]:  # Limit to avoid huge files
                    # Remove mock-related lines
                    cleaned_method = re.sub(r'.*(@patch|Mock|mock).*\n', '', method)
                    new_class += cleaned_method + '\n'
                
                new_content += new_class
        
        # Save updated file
        backup_path = file_path.with_suffix('.py.bak')
        file_path.rename(backup_path)
        file_path.write_text(new_content)
        
        print(f"✅ Updated: {file_path.name}")
        return True
        
    except Exception as e:
        print(f"❌ Error updating {file_path.name}: {e}")
        return False


def main():
    """Main function to update all tests"""
    test_dir = Path(__file__).parent
    
    print("🔍 Finding test files with mocks...")
    test_files = find_test_files_with_mocks(test_dir)
    
    print(f"\n📝 Found {len(test_files)} test files to update:")
    for f in test_files:
        print(f"  - {f.relative_to(test_dir)}")
    
    if not test_files:
        print("\n✅ No test files with mocks found!")
        return
    
    # Ask for confirmation
    response = input("\n⚠️  This will update test files to use real server. Continue? (y/N): ")
    if response.lower() != 'y':
        print("❌ Aborted")
        return
    
    print("\n🔄 Updating test files...")
    updated = 0
    for file_path in test_files:
        if update_test_file(file_path):
            updated += 1
    
    print(f"\n✅ Updated {updated}/{len(test_files)} files")
    print("\n📌 Next steps:")
    print("1. Review the updated test files")
    print("2. Restore any incorrectly updated files from .bak backups")
    print("3. Ensure ProximaDB server is running before running tests")
    print("4. Run: pytest tests/ -v")


if __name__ == "__main__":
    main()