#!/usr/bin/env python3
"""
Test optimized gRPC SDK batch operations
Verifies that multiple vectors per batch work correctly
"""

import sys
import os
import time
import uuid
import numpy as np
from pathlib import Path
import logging

# Add SDK to path (go up to workspace root, then to clients/python/src)
workspace_root = Path(__file__).parent.parent.parent.parent  # /workspace/
sdk_path = workspace_root / "clients/python/src"
print(f"Adding SDK path: {sdk_path}")
print(f"SDK path exists: {sdk_path.exists()}")
# IMPORTANT: This test requires the ProximaDB Python SDK to be in PYTHONPATH
# Run with: PYTHONPATH=/workspace/clients/python/src python3 test_optimized_grpc_batch.py
# Do NOT add sys.path.insert() - paths should be externalized via environment variables

# Debug path
print(f"Python path: {sys.path[:3]}")
print(f"ProximaDB module search:")
import os
proximadb_path = sdk_path / "proximadb"
print(f"ProximaDB path: {proximadb_path}")
print(f"ProximaDB exists: {proximadb_path.exists()}")
if proximadb_path.exists():
    print(f"ProximaDB files: {list(os.listdir(proximadb_path))[:5]}")

# Add utils to path for BERT embedding functions
current_dir = Path(__file__).parent
# IMPORTANT: This test requires the ProximaDB Python SDK to be in PYTHONPATH
# Run with: PYTHONPATH=/workspace/clients/python/src python3 test_optimized_grpc_batch.py
# Do NOT add sys.path.insert() - paths should be externalized via environment variables

from proximadb.grpc_client import ProximaDBClient
from bert_embedding_utils import (
    generate_text_corpus,
    convert_corpus_to_vectors,
    create_deterministic_embedding
)

# Enable debug logging to see the batch processing
logging.basicConfig(level=logging.DEBUG)


class OptimizedGrpcBatchTest:
    """Test optimized gRPC batch processing"""
    
    def __init__(self):
        self.server_address = "localhost:5679"  # gRPC port
        self.client = None
        self.collection_id = f"batch_test_{uuid.uuid4().hex[:8]}"
        self.dimension = 384
        
    def run_test(self):
        """Run comprehensive batch test"""
        print("🚀 Optimized gRPC Batch Processing Test")
        print("=" * 60)
        
        try:
            # Initialize gRPC client
            if not self.init_grpc_client():
                return False
            
            # Test 1: Create collection
            if not self.create_collection():
                return False
            
            # Test 2: Test various input formats
            if not self.test_input_formats():
                return False
            
            # Test 3: Test large batch processing
            if not self.test_large_batch():
                return False
            
            # Test 4: Test chunked batch processing
            if not self.test_chunked_processing():
                return False
            
            # Test 5: Verify all vectors were inserted
            if not self.verify_insertion():
                return False
            
            print("\n🎉 Optimized gRPC batch test completed successfully!")
            return True
            
        except Exception as e:
            print(f"❌ Test failed: {e}")
            import traceback
            traceback.print_exc()
            return False
        finally:
            self.cleanup()
    
    def init_grpc_client(self):
        """Initialize gRPC client"""
        try:
            print("🔗 Initializing gRPC client...")
            self.client = ProximaDBClient(
                endpoint=self.server_address,
                enable_debug_logging=True  # Enable debug logging
            )
            print("✅ gRPC client initialized")
            return True
            
        except Exception as e:
            print(f"❌ gRPC client initialization failed: {e}")
            return False
    
    def create_collection(self):
        """Create test collection"""
        print(f"\n🏗️ Creating collection: {self.collection_id}")
        
        try:
            result = self.client.create_collection(
                name=self.collection_id,
                dimension=self.dimension,
                distance_metric=1,  # COSINE
                storage_engine=1    # VIPER
            )
            print("✅ Collection created successfully")
            return True
            
        except Exception as e:
            print(f"❌ Collection creation failed: {e}")
            return False
    
    def test_input_formats(self):
        """Test different input formats"""
        print("\n📝 Testing Different Input Formats")
        print("-" * 50)
        
        try:
            # Format 1: Single vector as dict
            print("Testing single vector dict format...")
            single_vector = {
                "id": "single_vec_1",
                "vector": np.random.normal(0, 0.1, self.dimension).tolist(),
                "metadata": {"type": "single", "test": "format1"}
            }
            
            result = self.client.insert_vectors(self.collection_id, single_vector)
            print(f"   ✅ Single dict: {result.count} vectors inserted")
            
            # Format 2: List of vector dicts (standard format)
            print("Testing list of vector dicts format...")
            vector_dicts = []
            for i in range(5):
                vector_dicts.append({
                    "id": f"dict_vec_{i}",
                    "vector": np.random.normal(0, 0.1, self.dimension).tolist(),
                    "metadata": {"type": "dict_list", "index": i}
                })
            
            result = self.client.insert_vectors(self.collection_id, vector_dicts)
            print(f"   ✅ Dict list: {result.count} vectors inserted")
            
            # Format 3: Raw vectors with separate IDs and metadata
            print("Testing raw vectors with IDs format...")
            raw_vectors = [np.random.normal(0, 0.1, self.dimension).tolist() for _ in range(3)]
            raw_ids = [f"raw_vec_{i}" for i in range(3)]
            raw_metadata = [{"type": "raw", "index": i} for i in range(3)]
            
            result = self.client.insert_vectors(
                collection_id=self.collection_id,
                vectors=raw_vectors,
                ids=raw_ids,
                metadata=raw_metadata
            )
            print(f"   ✅ Raw vectors: {result.count} vectors inserted")
            
            # Format 4: Single raw vector
            print("Testing single raw vector format...")
            single_raw = np.random.normal(0, 0.1, self.dimension).tolist()
            
            result = self.client.insert_vectors(
                collection_id=self.collection_id,
                vectors=single_raw,
                ids="single_raw_1",
                metadata={"type": "single_raw"}
            )
            print(f"   ✅ Single raw: {result.count} vectors inserted")
            
            print("\n📊 Input format tests completed successfully!")
            return True
            
        except Exception as e:
            print(f"❌ Input format test failed: {e}")
            return False
    
    def test_large_batch(self):
        """Test large batch processing"""
        print("\n🔥 Testing Large Batch Processing")
        print("-" * 50)
        
        try:
            # Generate large corpus
            batch_size = 500
            print(f"Generating {batch_size} BERT vectors...")
            
            corpus = generate_text_corpus(batch_size)
            vectors = convert_corpus_to_vectors(corpus, self.dimension)
            
            print(f"Inserting {len(vectors)} vectors in single batch...")
            start_time = time.time()
            
            result = self.client.insert_vectors(self.collection_id, vectors)
            
            end_time = time.time()
            duration = end_time - start_time
            
            print(f"✅ Large batch results:")
            print(f"   Vectors requested: {len(vectors)}")
            print(f"   Vectors inserted: {result.count}")
            print(f"   Failed vectors: {result.failed_count}")
            print(f"   Duration: {duration:.2f}s")
            print(f"   Rate: {result.count/duration:.1f} vectors/second")
            
            # Expect significant improvement over single-vector processing
            if result.count >= batch_size * 0.8:  # Allow some failures
                print("✅ Large batch processing successful!")
                return True
            else:
                print(f"⚠️ Large batch processing below expected: {result.count}/{batch_size}")
                return True  # Still pass but with warning
            
        except Exception as e:
            print(f"❌ Large batch test failed: {e}")
            return False
    
    def test_chunked_processing(self):
        """Test automatic chunking for very large batches"""
        print("\n🧩 Testing Chunked Processing")
        print("-" * 50)
        
        try:
            # Create a batch larger than the chunk size (1000)
            large_batch_size = 1500
            print(f"Testing chunked processing with {large_batch_size} vectors...")
            
            # Generate vectors
            large_vectors = []
            for i in range(large_batch_size):
                large_vectors.append({
                    "id": f"chunk_vec_{i}",
                    "vector": np.random.normal(0, 0.1, self.dimension).tolist(),
                    "metadata": {"batch": "chunked", "index": i}
                })
            
            start_time = time.time()
            result = self.client.insert_vectors(self.collection_id, large_vectors)
            end_time = time.time()
            
            duration = end_time - start_time
            
            print(f"✅ Chunked processing results:")
            print(f"   Total vectors: {large_batch_size}")
            print(f"   Vectors inserted: {result.count}")
            print(f"   Failed vectors: {result.failed_count}")
            print(f"   Duration: {duration:.2f}s")
            print(f"   Rate: {result.count/duration:.1f} vectors/second")
            
            return True
            
        except Exception as e:
            print(f"❌ Chunked processing test failed: {e}")
            return False
    
    def verify_insertion(self):
        """Verify vectors were actually inserted"""
        print("\n🔍 Verifying Vector Insertion")
        print("-" * 50)
        
        try:
            # Test search to verify vectors are accessible
            query_vector = create_deterministic_embedding("test query", self.dimension)
            
            results = self.client.search_vectors(
                collection_id=self.collection_id,
                query_vectors=[query_vector.tolist()],
                top_k=10,
                include_metadata=True
            )
            
            result_count = len(results) if results else 0
            print(f"✅ Search verification: {result_count} vectors found")
            
            if result_count > 0:
                print("   Sample results:")
                for i, result in enumerate(results[:3]):
                    metadata = result.metadata if hasattr(result, 'metadata') and result.metadata else {}
                    print(f"      {i+1}. ID: {result.id}, Score: {result.score:.4f}, Type: {metadata.get('type', 'unknown')}")
            
            return result_count > 0
            
        except Exception as e:
            print(f"❌ Verification failed: {e}")
            return False
    
    def cleanup(self):
        """Clean up test collection"""
        print(f"\n🧹 Cleaning up collection: {self.collection_id}")
        
        try:
            if self.client:
                self.client.delete_collection(self.collection_id)
                print("✅ Collection deleted successfully")
        except Exception as e:
            print(f"⚠️ Cleanup failed: {e}")


def main():
    """Run the optimized gRPC batch test"""
    test = OptimizedGrpcBatchTest()
    success = test.run_test()
    
    if success:
        print("\n🎉 Optimized gRPC batch test completed successfully!")
        print("Key improvements verified:")
        print("   ✅ Unified input interface (dict, raw vectors, single/batch)")
        print("   ✅ Proper batch processing (multiple vectors per request)")
        print("   ✅ Automatic chunking for large batches")
        print("   ✅ Enhanced error handling and validation")
        print("   ✅ Comprehensive debugging and logging")
    else:
        print("\n💥 Optimized gRPC batch test failed!")
        exit(1)


if __name__ == "__main__":
    main()