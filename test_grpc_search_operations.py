#!/usr/bin/env python3
"""
ProximaDB gRPC Search Operations Test
Demonstrates ID-based search, metadata filtering, and proximity search
"""

import sys
import time
import json
import logging
from datetime import datetime
from typing import List, Dict, Any
import numpy as np
from sentence_transformers import SentenceTransformer

# Add Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb import ProximaDBClient, Protocol
from proximadb.models import CollectionConfig
from proximadb.exceptions import ProximaDBError

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(message)s',
    datefmt='%Y-%m-%dT%H:%M:%S'
)
logger = logging.getLogger(__name__)


class SearchDemoManager:
    """Manages search demonstrations with ProximaDB gRPC client"""
    
    def __init__(self):
        self.client = None
        self.bert_model = None
        self.collection_name = f"search_demo_{int(time.time())}"
        self.test_data = []
        
    def setup(self):
        """Initialize client and BERT model"""
        logger.info("🚀 Setting up ProximaDB gRPC client and BERT model...")
        
        # Initialize gRPC client (port 5679)
        self.client = ProximaDBClient("http://localhost:5679", protocol=Protocol.GRPC)
        
        # Verify connection
        health = self.client.health()
        logger.info(f"✅ Connected to ProximaDB gRPC server: {health}")
        
        # Load BERT model for embeddings
        self.bert_model = SentenceTransformer('all-MiniLM-L6-v2')
        logger.info("✅ BERT model loaded successfully")
        
    def create_collection(self):
        """Create a test collection for search operations"""
        logger.info(f"📦 Creating collection: {self.collection_name}")
        
        config = CollectionConfig(
            dimension=384,  # all-MiniLM-L6-v2 dimension
            distance_metric="cosine",
            description="Search operations demo collection",
            index_config={
                "index_type": "hnsw",
                "ef_construction": 200,
                "max_connections": 16
            }
        )
        
        collection = self.client.create_collection(self.collection_name, config)
        logger.info(f"✅ Collection created: {collection.name} (ID: {collection.id})")
        return collection
        
    def prepare_test_data(self) -> List[Dict[str, Any]]:
        """Prepare diverse test data for comprehensive search testing"""
        logger.info("📊 Preparing test data...")
        
        # Diverse test documents with rich metadata
        documents = [
            # Technology category
            {
                "id": "tech_001",
                "text": "Artificial intelligence and machine learning are revolutionizing software development",
                "category": "technology",
                "subcategory": "ai",
                "importance": 9,
                "author": "Dr. Sarah Chen",
                "publication_date": "2025-06-15",
                "tags": ["AI", "ML", "software", "innovation"],
                "word_count": 10
            },
            {
                "id": "tech_002", 
                "text": "Cloud computing provides scalable infrastructure for modern applications",
                "category": "technology",
                "subcategory": "cloud",
                "importance": 8,
                "author": "Mark Thompson",
                "publication_date": "2025-06-20",
                "tags": ["cloud", "infrastructure", "scalability"],
                "word_count": 9
            },
            {
                "id": "tech_003",
                "text": "Blockchain technology enables decentralized and secure transactions",
                "category": "technology", 
                "subcategory": "blockchain",
                "importance": 7,
                "author": "Dr. Sarah Chen",
                "publication_date": "2025-06-10",
                "tags": ["blockchain", "security", "decentralization"],
                "word_count": 8
            },
            
            # Science category
            {
                "id": "sci_001",
                "text": "Quantum computing promises exponential speedup for complex calculations",
                "category": "science",
                "subcategory": "quantum",
                "importance": 10,
                "author": "Prof. Alan Turing",
                "publication_date": "2025-06-25",
                "tags": ["quantum", "computing", "physics"],
                "word_count": 9
            },
            {
                "id": "sci_002",
                "text": "CRISPR gene editing revolutionizes medical treatment possibilities",
                "category": "science",
                "subcategory": "biology", 
                "importance": 9,
                "author": "Dr. Jennifer Wu",
                "publication_date": "2025-06-18",
                "tags": ["CRISPR", "genetics", "medicine"],
                "word_count": 8
            },
            
            # Business category
            {
                "id": "biz_001",
                "text": "Digital transformation is essential for competitive business advantage",
                "category": "business",
                "subcategory": "strategy",
                "importance": 8,
                "author": "Mark Thompson",
                "publication_date": "2025-06-22",
                "tags": ["digital", "transformation", "strategy"],
                "word_count": 9
            },
            {
                "id": "biz_002",
                "text": "Agile methodology improves software development team productivity",
                "category": "business",
                "subcategory": "management",
                "importance": 7,
                "author": "Lisa Anderson",
                "publication_date": "2025-06-12",
                "tags": ["agile", "productivity", "management"],
                "word_count": 8
            },
            
            # Education category
            {
                "id": "edu_001",
                "text": "Online learning platforms democratize access to quality education worldwide",
                "category": "education",
                "subcategory": "online",
                "importance": 9,
                "author": "Prof. Alan Turing",
                "publication_date": "2025-06-19",
                "tags": ["education", "online", "accessibility"],
                "word_count": 10
            },
            {
                "id": "edu_002",
                "text": "Virtual reality enhances immersive educational experiences for students",
                "category": "education",
                "subcategory": "technology",
                "importance": 8,
                "author": "Dr. Jennifer Wu",
                "publication_date": "2025-06-23",
                "tags": ["VR", "education", "immersive"],
                "word_count": 9
            },
            
            # Healthcare category
            {
                "id": "health_001",
                "text": "Telemedicine expands healthcare access to remote communities globally",
                "category": "healthcare",
                "subcategory": "telemedicine",
                "importance": 10,
                "author": "Dr. Jennifer Wu",
                "publication_date": "2025-06-24",
                "tags": ["telemedicine", "healthcare", "accessibility"],
                "word_count": 9
            }
        ]
        
        # Generate embeddings for all documents
        texts = [doc["text"] for doc in documents]
        embeddings = self.bert_model.encode(texts)
        
        # Combine with metadata
        for i, doc in enumerate(documents):
            doc["embedding"] = embeddings[i].tolist()
            self.test_data.append(doc)
            
        logger.info(f"✅ Prepared {len(self.test_data)} test documents with embeddings")
        return self.test_data
        
    def ingest_data(self):
        """Ingest test data into the collection"""
        logger.info("💾 Ingesting data into collection...")
        
        success_count = 0
        for doc in self.test_data:
            try:
                result = self.client.insert_vector(
                    collection_id=self.collection_name,
                    vector_id=doc["id"],
                    vector=doc["embedding"],
                    metadata={
                        "text": doc["text"],
                        "category": doc["category"],
                        "subcategory": doc["subcategory"],
                        "importance": doc["importance"],
                        "author": doc["author"],
                        "publication_date": doc["publication_date"],
                        "tags": doc["tags"],
                        "word_count": doc["word_count"]
                    }
                )
                success_count += 1
                logger.debug(f"✅ Inserted: {doc['id']}")
            except Exception as e:
                logger.error(f"❌ Failed to insert {doc['id']}: {e}")
                
        logger.info(f"✅ Successfully ingested {success_count}/{len(self.test_data)} documents")
        
        # Allow time for indexing
        time.sleep(1)
        
    def search_by_id(self):
        """Demonstrate ID-based search"""
        logger.info("\n🔍 === ID-BASED SEARCH ===")
        
        # Search for specific IDs
        test_ids = ["tech_001", "sci_001", "biz_999"]  # Last one doesn't exist
        
        for vector_id in test_ids:
            logger.info(f"\n📌 Searching for ID: {vector_id}")
            try:
                result = self.client.get_vector(
                    collection_id=self.collection_name,
                    vector_id=vector_id,
                    include_vector=False,  # Don't return the embedding
                    include_metadata=True
                )
                
                if result:
                    logger.info(f"✅ Found vector: {vector_id}")
                    logger.info(f"   Text: {result['metadata']['text']}")
                    logger.info(f"   Category: {result['metadata']['category']}")
                    logger.info(f"   Author: {result['metadata']['author']}")
                else:
                    logger.info(f"❌ Vector not found: {vector_id}")
                    
            except Exception as e:
                logger.error(f"❌ Error searching for {vector_id}: {e}")
                
    def search_by_metadata(self):
        """Demonstrate metadata field search"""
        logger.info("\n🔍 === METADATA FIELD SEARCH ===")
        
        # Note: Since server doesn't implement filtered search yet,
        # we'll demonstrate the API calls that would be used
        
        # Example 1: Search by category
        logger.info("\n📌 Search by category = 'technology'")
        query_text = "innovative software solutions"
        query_embedding = self.bert_model.encode([query_text])[0]
        
        try:
            results = self.client.search(
                collection_id=self.collection_name,
                query=query_embedding.tolist(),
                k=5,
                filter={"category": "technology"},  # Metadata filter
                include_metadata=True,
                include_vectors=False
            )
            
            logger.info(f"Found {len(results)} results with category='technology':")
            for i, result in enumerate(results, 1):
                logger.info(f"{i}. ID: {result.id}, Score: {result.score:.3f}")
                logger.info(f"   Text: {result.metadata['text']}")
                logger.info(f"   Subcategory: {result.metadata['subcategory']}")
                
        except Exception as e:
            logger.warning(f"⚠️ Filtered search not fully implemented on server: {e}")
            logger.info("Falling back to unfiltered search with client-side filtering...")
            
            # Fallback: Search without filter and filter client-side
            results = self.client.search(
                collection_id=self.collection_name,
                query=query_embedding.tolist(),
                k=10,
                include_metadata=True,
                include_vectors=False
            )
            
            filtered_results = [r for r in results if r.metadata.get('category') == 'technology']
            logger.info(f"Found {len(filtered_results)} technology results (client-side filtered):")
            for i, result in enumerate(filtered_results[:5], 1):
                logger.info(f"{i}. ID: {result.id}, Score: {result.score:.3f}")
                logger.info(f"   Text: {result.metadata['text']}")
                
        # Example 2: Search by author
        logger.info("\n📌 Search by author = 'Dr. Sarah Chen'")
        try:
            # Client-side filtering demonstration
            all_results = self.client.search(
                collection_id=self.collection_name,
                query=query_embedding.tolist(),
                k=10,
                include_metadata=True,
                include_vectors=False
            )
            
            author_results = [r for r in all_results if r.metadata.get('author') == 'Dr. Sarah Chen']
            logger.info(f"Found {len(author_results)} results by Dr. Sarah Chen:")
            for result in author_results:
                logger.info(f"- {result.id}: {result.metadata['text']}")
                
        except Exception as e:
            logger.error(f"❌ Error in metadata search: {e}")
            
        # Example 3: Complex metadata query (high importance)
        logger.info("\n📌 Search for high importance documents (importance >= 8)")
        try:
            all_results = self.client.search(
                collection_id=self.collection_name,
                query=query_embedding.tolist(),
                k=10,
                include_metadata=True,
                include_vectors=False
            )
            
            important_results = [r for r in all_results if r.metadata.get('importance', 0) >= 8]
            logger.info(f"Found {len(important_results)} high importance documents:")
            for result in important_results:
                logger.info(f"- {result.id} (importance={result.metadata['importance']}): {result.metadata['text'][:50]}...")
                
        except Exception as e:
            logger.error(f"❌ Error in importance search: {e}")
            
    def proximity_search(self):
        """Demonstrate proximity/similarity search"""
        logger.info("\n🔍 === PROXIMITY/SIMILARITY SEARCH ===")
        
        # Test different query scenarios
        queries = [
            {
                "text": "artificial intelligence machine learning deep learning",
                "description": "AI/ML related query"
            },
            {
                "text": "healthcare medicine telemedicine remote patient care",
                "description": "Healthcare related query"
            },
            {
                "text": "quantum computing physics exponential speedup algorithms",
                "description": "Quantum computing query"
            },
            {
                "text": "online education virtual learning student engagement",
                "description": "Education technology query"
            }
        ]
        
        for query_info in queries:
            logger.info(f"\n📌 Query: '{query_info['description']}'")
            logger.info(f"   Query text: '{query_info['text']}'")
            
            # Generate query embedding
            query_embedding = self.bert_model.encode([query_info['text']])[0]
            
            try:
                # Perform similarity search
                results = self.client.search(
                    collection_id=self.collection_name,
                    query=query_embedding.tolist(),
                    k=3,  # Top 3 results
                    include_metadata=True,
                    include_vectors=False
                )
                
                logger.info(f"\n   Top {len(results)} similar documents:")
                for i, result in enumerate(results, 1):
                    logger.info(f"\n   {i}. ID: {result.id}")
                    logger.info(f"      Similarity Score: {result.score:.4f}")
                    logger.info(f"      Text: {result.metadata['text']}")
                    logger.info(f"      Category: {result.metadata['category']}")
                    logger.info(f"      Author: {result.metadata['author']}")
                    
            except Exception as e:
                logger.error(f"❌ Error in proximity search: {e}")
                
        # Demonstrate similarity between specific documents
        logger.info("\n📌 Document-to-Document Similarity")
        
        # Find documents similar to "tech_001"
        source_doc = next(d for d in self.test_data if d['id'] == 'tech_001')
        logger.info(f"Finding documents similar to: '{source_doc['text']}'")
        
        try:
            results = self.client.search(
                collection_id=self.collection_name,
                query=source_doc['embedding'],
                k=5,
                include_metadata=True,
                include_vectors=False
            )
            
            logger.info(f"\nDocuments similar to tech_001:")
            for i, result in enumerate(results, 1):
                if result.id != 'tech_001':  # Skip the source document itself
                    logger.info(f"{i}. {result.id} (score: {result.score:.3f}): {result.metadata['text'][:60]}...")
                    
        except Exception as e:
            logger.error(f"❌ Error in document similarity search: {e}")
            
    def cleanup(self):
        """Clean up resources"""
        if self.client and self.collection_name:
            try:
                logger.info(f"\n🧹 Cleaning up collection: {self.collection_name}")
                success = self.client.delete_collection(self.collection_name)
                if success:
                    logger.info("✅ Collection deleted successfully")
            except Exception as e:
                logger.error(f"❌ Error deleting collection: {e}")
                
        if self.client:
            self.client.close()
            

def main():
    """Main demonstration function"""
    logger.info("=" * 80)
    logger.info("🚀 ProximaDB gRPC Search Operations Demonstration")
    logger.info("=" * 80)
    
    demo = SearchDemoManager()
    
    try:
        # Setup
        demo.setup()
        
        # Create collection
        demo.create_collection()
        
        # Prepare and ingest test data
        demo.prepare_test_data()
        demo.ingest_data()
        
        # Demonstrate different search types
        demo.search_by_id()
        demo.search_by_metadata()
        demo.proximity_search()
        
        logger.info("\n" + "=" * 80)
        logger.info("✅ Search demonstration completed successfully!")
        logger.info("=" * 80)
        
    except Exception as e:
        logger.error(f"❌ Demonstration failed: {e}")
        raise
    finally:
        demo.cleanup()
        

if __name__ == "__main__":
    main()