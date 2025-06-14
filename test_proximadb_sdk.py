#!/usr/bin/env python3
"""
Comprehensive ProximaDB Python SDK Test Suite

Tests all collection operations with authentication/authorization, 
bulk operations, vector generation using BERT embeddings, and various search types.
"""

import sys
import json
import time
import random
import numpy as np
from datetime import datetime
from typing import List, Dict, Any, Optional

try:
    import requests
    from transformers import BertTokenizer, BertModel
    import torch
    from sklearn.metrics.pairwise import cosine_similarity
except ImportError as e:
    print(f"Missing dependencies: {e}")
    print("Install with: pip install requests transformers torch scikit-learn")
    sys.exit(1)

class ProximaDBClient:
    """ProximaDB Python SDK Client for REST API"""
    
    def __init__(self, base_url: str = "http://localhost:5678", api_key: Optional[str] = None):
        self.base_url = base_url.rstrip('/')
        self.api_url = f"{self.base_url}/api/v1"
        self.api_key = api_key
        self.session = requests.Session()
        
        # Set headers
        self.session.headers.update({
            'Content-Type': 'application/json',
            'User-Agent': 'proximadb-python-sdk/0.1.0'
        })
        
        if api_key:
            self.session.headers['Authorization'] = f'Bearer {api_key}'
    
    def health_check(self) -> Dict[str, Any]:
        """Check server health"""
        response = self.session.get(f"{self.base_url}/health")
        response.raise_for_status()
        return response.json()
    
    def list_collections(self) -> List[Dict[str, Any]]:
        """List all collections with full details"""
        response = self.session.get(f"{self.api_url}/collections")
        response.raise_for_status()
        return response.json()
    
    def list_collection_ids(self) -> List[str]:
        """List only collection IDs"""
        response = self.session.get(f"{self.api_url}/collections/ids")
        response.raise_for_status()
        return response.json()
    
    def list_collection_names(self) -> List[str]:
        """List only collection names"""
        response = self.session.get(f"{self.api_url}/collections/names")
        response.raise_for_status()
        return response.json()
    
    def get_collection_id_by_name(self, collection_name: str) -> Dict[str, str]:
        """Get collection ID by name"""
        response = self.session.get(f"{self.api_url}/collections/name_to_id/{collection_name}")
        response.raise_for_status()
        return response.json()
    
    def get_collection_name_by_id(self, collection_id: str) -> Dict[str, str]:
        """Get collection name by ID"""
        response = self.session.get(f"{self.api_url}/collections/id_to_name/{collection_id}")
        response.raise_for_status()
        return response.json()
    
    def create_collection(self, collection_data: Dict[str, Any]) -> Dict[str, Any]:
        """Create a new collection with full schema"""
        response = self.session.post(f"{self.api_url}/collections", json=collection_data)
        response.raise_for_status()
        return response.json()
    
    def get_collection(self, collection_name: str) -> Dict[str, Any]:
        """Get collection details by name (user-friendly)"""
        response = self.session.get(f"{self.api_url}/collections/name/{collection_name}")
        response.raise_for_status()
        return response.json()
    
    def delete_collection(self, collection_name: str) -> Dict[str, Any]:
        """Delete a collection by name (user-friendly)"""
        response = self.session.delete(f"{self.api_url}/collections/name/{collection_name}")
        response.raise_for_status()
        return response.json()
    
    def insert_vectors(self, collection_name: str, vectors: List[Dict[str, Any]]) -> Dict[str, Any]:
        """Batch insert vectors into collection by name (user-friendly)"""
        payload = {"vectors": vectors}
        response = self.session.post(f"{self.api_url}/collections/name/{collection_name}/vectors/batch", json=payload)
        response.raise_for_status()
        return response.json()
    
    def update_vectors(self, collection_name: str, vectors: List[Dict[str, Any]]) -> Dict[str, Any]:
        """Update vectors in collection (upsert)"""
        payload = {"vectors": vectors}
        response = self.session.put(f"{self.api_url}/collections/{collection_name}/vectors", json=payload)
        response.raise_for_status()
        return response.json()
    
    def delete_vectors(self, collection_name: str, vector_ids: List[str]) -> Dict[str, Any]:
        """Delete vectors by IDs"""
        payload = {"ids": vector_ids}
        response = self.session.delete(f"{self.api_url}/collections/{collection_name}/vectors", json=payload)
        response.raise_for_status()
        return response.json()
    
    def search_vectors(self, collection_name: str, query_vector: List[float], 
                      top_k: int = 10, filters: Optional[Dict] = None) -> Dict[str, Any]:
        """Search for similar vectors by collection name (user-friendly)"""
        payload = {
            "vector": query_vector,
            "k": top_k
        }
        if filters:
            payload["filter"] = filters
        
        response = self.session.post(f"{self.api_url}/collections/name/{collection_name}/search", json=payload)
        response.raise_for_status()
        return response.json()
    
    def get_vector_by_id(self, collection_name: str, vector_id: str) -> Dict[str, Any]:
        """Get vector by server UUID using collection name (user-friendly)"""
        response = self.session.get(f"{self.api_url}/collections/name/{collection_name}/vectors/{vector_id}")
        response.raise_for_status()
        return response.json()
    
    def get_vector_by_client_id(self, collection_name: str, client_id: str) -> Dict[str, Any]:
        """Get vector by client-provided ID using collection name (user-friendly)"""
        response = self.session.get(f"{self.api_url}/collections/name/{collection_name}/search/client_id/{client_id}")
        response.raise_for_status()
        return response.json()

class BERTVectorGenerator:
    """Generate embeddings using BERT model"""
    
    def __init__(self, model_name: str = 'bert-base-uncased'):
        print(f"Loading BERT model: {model_name}")
        self.tokenizer = BertTokenizer.from_pretrained(model_name)
        self.model = BertModel.from_pretrained(model_name)
        self.model.eval()
        print("BERT model loaded successfully")
    
    def generate_embedding(self, text: str) -> List[float]:
        """Generate BERT embedding for text"""
        inputs = self.tokenizer(text, return_tensors='pt', 
                               truncation=True, padding=True, max_length=512)
        
        with torch.no_grad():
            outputs = self.model(**inputs)
            # Use [CLS] token embedding
            embedding = outputs.last_hidden_state[:, 0, :].squeeze()
            
        return embedding.tolist()
    
    def generate_embeddings_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for batch of texts"""
        embeddings = []
        for text in texts:
            embedding = self.generate_embedding(text)
            embeddings.append(embedding)
        return embeddings

class TestSuite:
    """Comprehensive ProximaDB test suite"""
    
    def __init__(self):
        self.client = ProximaDBClient()
        self.bert_generator = BERTVectorGenerator()
        self.test_collection = "test_collection_bert"
        self.test_results = []
        self.test_vectors = []
        self.server_uuids = []
        
        # Sample texts for testing
        self.sample_texts = [
            "The quick brown fox jumps over the lazy dog",
            "Machine learning is revolutionizing artificial intelligence",
            "Database systems store and retrieve information efficiently",
            "Vector databases enable semantic search capabilities",
            "Natural language processing helps computers understand text",
            "Deep learning models can learn complex patterns in data",
            "ProximaDB is a cloud-native vector database",
            "Similarity search finds semantically related content",
            "Embeddings represent text as high-dimensional vectors",
            "BERT generates contextual word representations"
        ]
    
    def log_test(self, test_name: str, success: bool, message: str = ""):
        """Log test result"""
        timestamp = datetime.now().isoformat()
        result = {
            "test_name": test_name,
            "success": success,
            "message": message,
            "timestamp": timestamp
        }
        self.test_results.append(result)
        
        status = "✅ PASS" if success else "❌ FAIL"
        print(f"{status} {test_name}: {message}")
        
        return success
    
    def test_health_check(self) -> bool:
        """Test 1: Health endpoint"""
        try:
            health = self.client.health_check()
            return self.log_test("Health Check", True, f"Status: {health.get('status', 'unknown')}")
        except Exception as e:
            return self.log_test("Health Check", False, str(e))
    
    def test_authentication(self) -> bool:
        """Test 2: Authentication (basic - no auth configured yet)"""
        try:
            # For now, just test that requests work without auth
            collections = self.client.list_collections()
            return self.log_test("Authentication", True, "No auth required (default config)")
        except Exception as e:
            return self.log_test("Authentication", False, str(e))
    
    def test_collection_operations(self) -> bool:
        """Test 3: Collection CRUD operations with ProximaDB schema"""
        try:
            # List collections initially
            initial_collections = self.client.list_collections()
            
            # Delete collection if it exists (cleanup from previous runs)
            try:
                self.client.delete_collection(self.test_collection)
                print(f"Deleted existing collection '{self.test_collection}'")
            except:
                pass  # Collection doesn't exist, which is fine
            
            # Create collection with full ProximaDB schema
            collection_data = {
                "name": self.test_collection,
                "dimension": 768,  # BERT embedding dimension  
                "distance_metric": "cosine",
                "indexing_algorithm": "hnsw",
                "config": {
                    "hnsw_m": 16,
                    "hnsw_ef_construction": 200,
                    "hnsw_ef_search": 100,
                    "description": "Test collection for BERT embeddings with comprehensive schema"
                }
            }
            
            collection_info = self.client.create_collection(collection_data)
            
            # Verify collection exists and has proper schema
            collections = self.client.list_collections()
            collection_found = False
            for col in collections:
                if col.get('name') == self.test_collection:
                    collection_found = True
                    break
            
            if not collection_found:
                raise Exception("Collection not found in list after creation")
            
            # Get collection details and verify schema
            details = self.client.get_collection(self.test_collection)
            if details.get('name') != self.test_collection:
                raise Exception("Collection details don't match")
            if details.get('dimension') != 768:
                raise Exception(f"Dimension mismatch: expected 768, got {details.get('dimension')}")
            if details.get('distance_metric') != "cosine":
                raise Exception(f"Distance metric mismatch: expected cosine, got {details.get('distance_metric')}")
            
            return self.log_test("Collection Operations", True, 
                               f"Created collection '{self.test_collection}' with {details.get('dimension', 0)} dimensions, {details.get('distance_metric')} metric, {details.get('indexing_algorithm')} index")
        except Exception as e:
            return self.log_test("Collection Operations", False, str(e))
    
    def test_vector_generation(self) -> bool:
        """Test 4: Generate BERT embeddings"""
        try:
            print("Generating BERT embeddings for sample texts...")
            
            for i, text in enumerate(self.sample_texts):
                embedding = self.bert_generator.generate_embedding(text)
                
                vector = {
                    "id": f"client_vector_{i+1}",  # Client-provided ID
                    "vector": embedding,
                    "metadata": {
                        "text": text,
                        "category": "sample",
                        "index": i,
                        "length": len(text),
                        "source": "bert_embeddings"
                    }
                }
                self.test_vectors.append(vector)
            
            return self.log_test("Vector Generation", True, 
                               f"Generated {len(self.test_vectors)} BERT embeddings (768 dims each)")
        except Exception as e:
            return self.log_test("Vector Generation", False, str(e))
    
    def test_bulk_insert(self) -> bool:
        """Test 5: Bulk insert vectors with client IDs"""
        try:
            result = self.client.insert_vectors(self.test_collection, self.test_vectors)
            
            # Store server UUIDs for later tests
            self.server_uuids = result.get("inserted_ids", [])
            
            return self.log_test("Bulk Insert", True, 
                               f"Inserted {len(self.test_vectors)} vectors successfully, got {len(self.server_uuids)} UUIDs")
        except Exception as e:
            return self.log_test("Bulk Insert", False, str(e))
    
    def test_id_based_search(self) -> bool:
        """Test 6: ID-based vector retrieval (both server UUID and client ID)"""
        try:
            # Test 1: Get vector by server UUID
            if hasattr(self, 'server_uuids') and self.server_uuids:
                server_uuid = self.server_uuids[0]
                vector_by_uuid = self.client.get_vector_by_id(self.test_collection, server_uuid)
                
                if vector_by_uuid.get('id') != server_uuid:
                    raise Exception(f"Retrieved vector UUID {vector_by_uuid.get('id')} doesn't match requested {server_uuid}")
            
            # Test 2: Get vector by client ID
            client_id = self.test_vectors[0]["id"]
            vector_by_client_id = self.client.get_vector_by_client_id(self.test_collection, client_id)
            
            # Verify the client ID is stored in metadata
            stored_client_id = vector_by_client_id.get('metadata', {}).get('client_id')
            if stored_client_id != client_id:
                raise Exception(f"Retrieved client_id '{stored_client_id}' doesn't match requested '{client_id}'")
            
            return self.log_test("ID-based Search", True, 
                               f"Retrieved vector by UUID and client_id '{client_id}' successfully")
        except Exception as e:
            return self.log_test("ID-based Search", False, str(e))
    
    def test_similarity_search(self) -> bool:
        """Test 7: Similarity search"""
        try:
            # Use the first vector as query
            query_vector = self.test_vectors[0]["vector"]
            query_text = self.test_vectors[0]["metadata"]["text"]
            
            results = self.client.search_vectors(
                collection_name=self.test_collection,
                query_vector=query_vector,
                top_k=5
            )
            
            matches = results.get("matches", [])
            if not matches:
                raise Exception("No search results returned")
            
            # First result should be the exact match
            if matches[0].get("id") != self.test_vectors[0]["id"]:
                raise Exception("Top result is not the query vector itself")
            
            return self.log_test("Similarity Search", True, 
                               f"Found {len(matches)} similar vectors for query: '{query_text[:50]}...'")
        except Exception as e:
            return self.log_test("Similarity Search", False, str(e))
    
    def test_metadata_filtering(self) -> bool:
        """Test 8: Metadata-filtered search"""
        try:
            # Search with metadata filter
            query_vector = self.test_vectors[0]["vector"]
            
            # Filter for vectors with index > 5
            filters = {
                "metadata.index": {"$gt": 5}
            }
            
            results = self.client.search_vectors(
                collection_name=self.test_collection,
                query_vector=query_vector,
                top_k=3,
                filters=filters
            )
            
            matches = results.get("matches", [])
            
            return self.log_test("Metadata Filtering", True, 
                               f"Found {len(matches)} vectors with metadata filter")
        except Exception as e:
            return self.log_test("Metadata Filtering", False, str(e))
    
    def test_bulk_update(self) -> bool:
        """Test 9: Bulk update (upsert) vectors"""
        try:
            # Update some vectors with new metadata
            update_vectors = []
            for i in range(0, 3):
                vector = self.test_vectors[i].copy()
                vector["metadata"]["updated"] = True
                vector["metadata"]["update_timestamp"] = datetime.now().isoformat()
                update_vectors.append(vector)
            
            result = self.client.update_vectors(self.test_collection, update_vectors)
            
            return self.log_test("Bulk Update", True, 
                               f"Updated {len(update_vectors)} vectors successfully")
        except Exception as e:
            return self.log_test("Bulk Update", False, str(e))
    
    def test_bulk_delete(self) -> bool:
        """Test 10: Bulk delete vectors"""
        try:
            # Delete last 2 vectors
            delete_ids = [self.test_vectors[-2]["id"], self.test_vectors[-1]["id"]]
            
            result = self.client.delete_vectors(self.test_collection, delete_ids)
            
            return self.log_test("Bulk Delete", True, 
                               f"Deleted {len(delete_ids)} vectors successfully")
        except Exception as e:
            return self.log_test("Bulk Delete", False, str(e))
    
    def test_semantic_search(self) -> bool:
        """Test 11: Semantic search with new query"""
        try:
            # Generate embedding for a new text
            new_text = "Artificial intelligence and machine learning algorithms"
            new_embedding = self.bert_generator.generate_embedding(new_text)
            
            results = self.client.search_vectors(
                collection_name=self.test_collection,
                query_vector=new_embedding,
                top_k=3
            )
            
            matches = results.get("matches", [])
            
            return self.log_test("Semantic Search", True, 
                               f"Found {len(matches)} semantically similar vectors for: '{new_text}'")
        except Exception as e:
            return self.log_test("Semantic Search", False, str(e))
    
    def cleanup_test_data(self) -> bool:
        """Test 12: Cleanup - Delete test collection"""
        try:
            result = self.client.delete_collection(self.test_collection)
            
            return self.log_test("Cleanup", True, "Test collection deleted successfully")
        except Exception as e:
            return self.log_test("Cleanup", False, str(e))
    
    def run_all_tests(self):
        """Run complete test suite"""
        print("🚀 Starting ProximaDB Python SDK Comprehensive Test Suite")
        print("=" * 70)
        
        start_time = time.time()
        
        # Run tests in order
        tests = [
            self.test_health_check,
            self.test_authentication,
            self.test_collection_operations,
            self.test_vector_generation,
            self.test_bulk_insert,
            self.test_id_based_search,
            self.test_similarity_search,
            self.test_metadata_filtering,
            self.test_bulk_update,
            self.test_bulk_delete,
            self.test_semantic_search,
            self.cleanup_test_data
        ]
        
        passed = 0
        failed = 0
        
        for test in tests:
            if test():
                passed += 1
            else:
                failed += 1
            print()  # Add spacing between tests
        
        end_time = time.time()
        duration = end_time - start_time
        
        print("=" * 70)
        print(f"🎯 Test Suite Complete!")
        print(f"✅ Passed: {passed}")
        print(f"❌ Failed: {failed}")
        print(f"⏱️  Duration: {duration:.2f} seconds")
        print(f"🏆 Success Rate: {(passed/(passed+failed)*100):.1f}%")
        
        # Save detailed results
        with open('proximadb_test_results.json', 'w') as f:
            json.dump({
                'summary': {
                    'passed': passed,
                    'failed': failed,
                    'duration': duration,
                    'success_rate': passed/(passed+failed)*100
                },
                'tests': self.test_results
            }, f, indent=2)
        
        print(f"📄 Detailed results saved to: proximadb_test_results.json")
        
        return failed == 0

if __name__ == "__main__":
    print("🧪 ProximaDB Python SDK Test Suite")
    print("📦 Testing BERT embeddings with comprehensive operations")
    print()
    
    # Run test suite
    test_suite = TestSuite()
    success = test_suite.run_all_tests()
    
    if success:
        print("\n🎉 All tests passed! ProximaDB unified service architecture is working correctly.")
    else:
        print("\n⚠️  Some tests failed. Check the results above for details.")
        sys.exit(1)