#!/usr/bin/env python3
"""
Fixed Vector Search Demo using direct HTTP calls
Tests search functionality, vector insertion, and basic operations
"""

import json
import time
import numpy as np
import uuid
import requests


class VectorSearchDemo:
    """Demonstration of vector search capabilities using direct HTTP"""
    
    def __init__(self):
        self.base_url = "http://localhost:5678"
        self.collection_name = f"search_demo_{uuid.uuid4().hex[:8]}"
        self.dimension = 384
        
    def run_demo(self):
        """Run the complete demo"""
        print("🚀 ProximaDB Vector Search Demo")
        print("=" * 50)
        
        try:
            # Check server health
            if not self.check_server_health():
                return False
                
            # 1. Setup collection
            if not self.setup_collection():
                return False
            
            # 2. Create and insert sample data
            print("\n📚 Creating sample data...")
            sample_data = self.create_sample_data()
            
            if not self.insert_sample_vectors(sample_data):
                return False
            
            # 3. Wait for indexing
            print("\n⏳ Waiting for indexing...")
            time.sleep(2)
            
            # 4. Demonstrate search operations
            print("\n🔍 Demonstrating Search Operations")
            print("-" * 40)
            
            self.demo_vector_search(sample_data)
            self.demo_search_verification(sample_data)
            
            print("\n🎉 Demo completed successfully!")
            return True
            
        except Exception as e:
            print(f"❌ Demo failed: {e}")
            import traceback
            traceback.print_exc()
            return False
        finally:
            self.cleanup()
    
    def check_server_health(self):
        """Check if the server is healthy"""
        try:
            response = requests.get(f"{self.base_url}/health", timeout=5)
            if response.status_code == 200:
                print("✅ Server is healthy")
                return True
            else:
                print(f"❌ Server health check failed: {response.status_code}")
                return False
        except Exception as e:
            print(f"❌ Cannot connect to server: {e}")
            return False
    
    def setup_collection(self):
        """Create demo collection"""
        print(f"🏗️ Creating collection: {self.collection_name}")
        
        collection_data = {
            "name": self.collection_name,
            "dimension": self.dimension,
            "distance_metric": "cosine",
            "storage_engine": "viper"
        }
        
        try:
            response = requests.post(
                f"{self.base_url}/collections",
                json=collection_data
            )
            
            if response.status_code == 200:
                print(f"✅ Collection created successfully")
                return True
            else:
                print(f"❌ Failed to create collection: {response.status_code}")
                print(f"   Response: {response.text}")
                return False
                
        except Exception as e:
            print(f"❌ Collection creation failed: {e}")
            return False
    
    def create_sample_data(self):
        """Create sample documents with varied content"""
        docs = [
            {
                "id": "ai_001",
                "text": "Artificial intelligence transforming industries",
                "category": "AI",
                "author": "Dr. Smith",
                "year": 2023
            },
            {
                "id": "ml_002", 
                "text": "Machine learning algorithms learn patterns from data",
                "category": "ML",
                "author": "Prof. Johnson", 
                "year": 2023
            },
            {
                "id": "dl_003",
                "text": "Deep learning neural networks solve complex problems", 
                "category": "DL",
                "author": "Dr. Chen",
                "year": 2024
            },
            {
                "id": "nlp_004",
                "text": "Natural language processing understands human text",
                "category": "NLP", 
                "author": "Dr. Williams",
                "year": 2024
            },
            {
                "id": "cv_005",
                "text": "Computer vision recognizes objects in images",
                "category": "CV",
                "author": "Prof. Davis",
                "year": 2023
            }
        ]
        
        # Convert to vectors with embeddings
        vectors = []
        for i, doc in enumerate(docs):
            # Create synthetic embeddings based on content
            # In real use, you'd use BERT or similar
            vector = self.create_synthetic_embedding(doc["text"], i)
            
            vectors.append({
                "id": doc["id"],
                "vector": vector.tolist(),
                "metadata": {
                    "text": doc["text"],
                    "category": doc["category"],
                    "author": doc["author"],
                    "year": doc["year"],
                    "doc_index": i
                }
            })
        
        print(f"   Created {len(vectors)} sample vectors")
        return vectors
    
    def create_synthetic_embedding(self, text, seed):
        """Create synthetic embedding based on text content"""
        # Use text hash and seed for reproducible embeddings
        np.random.seed(hash(text) % 2**32 + seed)
        base_vector = np.random.normal(0, 1, self.dimension)
        
        # Add some semantic structure based on keywords
        if "artificial intelligence" in text.lower() or "ai" in text.lower():
            base_vector[:50] += 0.5  # AI cluster
        if "machine learning" in text.lower() or "ml" in text.lower():
            base_vector[50:100] += 0.5  # ML cluster
        if "deep learning" in text.lower() or "neural" in text.lower():
            base_vector[100:150] += 0.5  # DL cluster
        if "language" in text.lower():
            base_vector[150:200] += 0.5  # NLP cluster
        if "vision" in text.lower() or "image" in text.lower():
            base_vector[200:250] += 0.5  # CV cluster
        
        # Normalize
        norm = np.linalg.norm(base_vector)
        if norm > 0:
            base_vector = base_vector / norm
        
        return base_vector.astype(np.float32)
    
    def insert_sample_vectors(self, vectors):
        """Insert sample vectors into the collection"""
        print(f"📥 Inserting {len(vectors)} vectors...")
        
        successful_inserts = 0
        
        for i, vector_data in enumerate(vectors):
            try:
                response = requests.post(
                    f"{self.base_url}/collections/{self.collection_name}/vectors",
                    json=vector_data
                )
                
                if response.status_code == 200:
                    successful_inserts += 1
                    print(f"   ✅ Inserted vector {vector_data['id']}")
                else:
                    print(f"   ❌ Failed to insert vector {vector_data['id']}: {response.status_code}")
                    
            except Exception as e:
                print(f"   ❌ Exception inserting vector {vector_data['id']}: {e}")
                continue
        
        print(f"📊 Insertion complete: {successful_inserts}/{len(vectors)} vectors inserted")
        return successful_inserts > 0
    
    def demo_vector_search(self, sample_data):
        """Demonstrate vector similarity search"""
        print("\n🔍 Vector Similarity Search")
        print("-" * 30)
        
        # Create a query vector similar to "AI" content
        query_text = "artificial intelligence and machine learning"
        query_vector = self.create_synthetic_embedding(query_text, 999)
        
        print(f"Query: '{query_text}'")
        
        try:
            search_data = {
                "vector": query_vector.tolist(),
                "k": 3
            }
            
            response = requests.post(
                f"{self.base_url}/collections/{self.collection_name}/search",
                json=search_data
            )
            
            if response.status_code == 200:
                search_result = response.json()
                results = search_result.get("data", [])
                
                print(f"✅ Found {len(results)} similar vectors:")
                
                for i, result in enumerate(results):
                    score = result.get("score", 0)
                    vector_id = result.get("id", "unknown")
                    metadata = result.get("metadata", {})
                    text = metadata.get("text", "No text available")
                    category = metadata.get("category", "Unknown")
                    
                    print(f"   {i+1}. ID: {vector_id}")
                    print(f"      Score: {score:.4f}")
                    print(f"      Category: {category}")
                    print(f"      Text: {text}")
                    print()
                
            else:
                print(f"❌ Search failed: {response.status_code}")
                print(f"   Response: {response.text}")
                
        except Exception as e:
            print(f"❌ Search exception: {e}")
    
    def demo_search_verification(self, sample_data):
        """Verify search functionality with different queries"""
        print("\n🔍 Search Verification Tests")
        print("-" * 30)
        
        test_queries = [
            {"text": "neural networks deep learning", "expected_category": "DL"},
            {"text": "natural language text processing", "expected_category": "NLP"},
            {"text": "computer vision image recognition", "expected_category": "CV"}
        ]
        
        for i, query in enumerate(test_queries):
            print(f"\nTest {i+1}: '{query['text']}'")
            
            query_vector = self.create_synthetic_embedding(query["text"], 1000 + i)
            
            try:
                search_data = {
                    "vector": query_vector.tolist(),
                    "k": 2
                }
                
                response = requests.post(
                    f"{self.base_url}/collections/{self.collection_name}/search",
                    json=search_data
                )
                
                if response.status_code == 200:
                    search_result = response.json()
                    results = search_result.get("data", [])
                    
                    if results:
                        top_result = results[0]
                        top_category = top_result.get("metadata", {}).get("category", "Unknown")
                        score = top_result.get("score", 0)
                        
                        if top_category == query["expected_category"]:
                            print(f"   ✅ Correct match: {top_category} (score: {score:.4f})")
                        else:
                            print(f"   ⚠️ Different match: {top_category} (expected: {query['expected_category']}, score: {score:.4f})")
                    else:
                        print(f"   ❌ No results found")
                else:
                    print(f"   ❌ Search failed: {response.status_code}")
                    
            except Exception as e:
                print(f"   ❌ Search exception: {e}")
    
    def cleanup(self):
        """Clean up the demo collection"""
        print(f"\n🧹 Cleaning up collection: {self.collection_name}")
        
        try:
            response = requests.delete(f"{self.base_url}/collections/{self.collection_name}")
            if response.status_code == 200:
                print("✅ Collection deleted successfully")
            else:
                print(f"⚠️ Collection deletion returned: {response.status_code}")
        except Exception as e:
            print(f"⚠️ Cleanup failed: {e}")


def main():
    """Run the search demo"""
    demo = VectorSearchDemo()
    success = demo.run_demo()
    
    if success:
        print("\n🎉 Vector search demo completed successfully!")
    else:
        print("\n💥 Vector search demo failed!")
        exit(1)


if __name__ == "__main__":
    main()