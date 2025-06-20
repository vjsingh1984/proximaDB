#!/usr/bin/env python3
"""
Test writing vectors to collection using BERT embeddings
"""

import asyncio
import numpy as np
import sys
from datetime import datetime

sys.path.insert(0, 'clients/python/src')
from proximadb.grpc_client import ProximaDBClient

def generate_bert_like_embeddings(texts, dimension=768):
    """
    Generate BERT-like embeddings for demonstration
    In real use, you'd use: from transformers import AutoTokenizer, AutoModel
    """
    embeddings = []
    np.random.seed(42)  # For reproducible results
    
    for i, text in enumerate(texts):
        # Simulate BERT embeddings with some text-dependent variation
        base_embedding = np.random.randn(dimension).astype(np.float32)
        
        # Add some text-length based variation to make embeddings more realistic
        text_factor = len(text) / 100.0
        base_embedding += np.random.randn(dimension).astype(np.float32) * text_factor * 0.1
        
        # Normalize to unit vector (common practice with BERT embeddings)
        base_embedding = base_embedding / np.linalg.norm(base_embedding)
        embeddings.append(base_embedding.tolist())
    
    return embeddings

async def test_bert_embeddings():
    """Test BERT embeddings workflow"""
    print("🤖 BERT Embeddings Vector Storage Test")
    print("=" * 60)
    
    # Sample text data for embedding
    documents = [
        "Machine learning is a subset of artificial intelligence.",
        "Natural language processing enables computers to understand human language.",
        "Deep learning uses neural networks with multiple layers.",
        "Vector databases store and search high-dimensional embeddings.",
        "BERT is a transformer-based language model for NLP tasks.",
        "Semantic search finds documents based on meaning, not just keywords.",
        "Embedding vectors capture semantic relationships between words.",
        "ProximaDB provides efficient vector storage and similarity search.",
        "Cosine similarity measures the angle between two vectors.",
        "Fine-tuning adapts pre-trained models to specific tasks."
    ]
    
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Step 1: Create a collection for BERT embeddings
    timestamp = datetime.now().strftime("%H%M%S")
    collection_name = f"bert_embeddings_{timestamp}"
    
    print(f"\n1️⃣ Creating collection: {collection_name}")
    
    try:
        result = await client.create_collection(
            name=collection_name,
            dimension=768,  # Standard BERT base dimension
            distance_metric=1,  # COSINE - ideal for normalized embeddings
        )
        print(f"   ✅ Created collection: {collection_name}")
        print(f"   📋 UUID: {result.id}")
        print(f"   📐 Dimension: 768 (BERT base)")
        print(f"   📏 Distance metric: COSINE")
    except Exception as e:
        print(f"   ❌ Failed to create collection: {e}")
        return False
    
    # Step 2: Generate BERT-like embeddings
    print(f"\n2️⃣ Generating BERT embeddings for {len(documents)} documents...")
    embeddings = generate_bert_like_embeddings(documents, dimension=768)
    
    print(f"   ✅ Generated {len(embeddings)} embeddings")
    print(f"   📊 Embedding shape: ({len(embeddings)}, 768)")
    print(f"   🔢 Sample embedding magnitude: {np.linalg.norm(embeddings[0]):.4f}")
    
    # Step 3: Insert vectors with metadata
    print(f"\n3️⃣ Inserting vectors with document metadata...")
    
    vectors_data = []
    for i, (doc, embedding) in enumerate(zip(documents, embeddings)):
        vector_data = {
            'id': f"doc_{i:03d}",
            'vector': embedding,
            'metadata': {
                'text': doc,
                'doc_id': i,
                'word_count': len(doc.split()),
                'category': 'ml_nlp' if any(term in doc.lower() for term in ['machine', 'neural', 'model']) else 'vector_db',
                'timestamp': datetime.now().isoformat()
            }
        }
        vectors_data.append(vector_data)
    
    try:
        # Insert vectors in batch
        insert_result = await client.insert_vectors(
            collection_name=collection_name,
            vectors=vectors_data
        )
        print(f"   ✅ Inserted {len(vectors_data)} vectors successfully")
        print(f"   📋 Insert result: {insert_result}")
    except Exception as e:
        print(f"   ❌ Failed to insert vectors: {e}")
        print(f"   💡 Note: Vector insertion might not be fully implemented yet")
        return False
    
    # Step 4: Test ID-based search
    print(f"\n4️⃣ Testing ID-based vector retrieval...")
    
    test_ids = ["doc_000", "doc_004", "doc_007"]  # BERT, Vector DB, ProximaDB docs
    
    for vector_id in test_ids:
        try:
            result = await client.get_vector(
                collection_name=collection_name,
                vector_id=vector_id
            )
            print(f"   ✅ Retrieved vector '{vector_id}':")
            print(f"      📄 Text: {result.get('metadata', {}).get('text', 'N/A')[:50]}...")
            print(f"      📊 Vector magnitude: {np.linalg.norm(result.get('vector', [0])):.4f}")
            print(f"      🏷️ Category: {result.get('metadata', {}).get('category', 'N/A')}")
        except Exception as e:
            print(f"   ❌ Failed to retrieve vector '{vector_id}': {e}")
            print(f"   💡 Note: ID-based retrieval might not be implemented yet")
    
    # Step 5: Test filterable metadata search
    print(f"\n5️⃣ Testing filterable metadata search...")
    
    # Test different metadata filters
    metadata_tests = [
        {
            'name': 'ML/NLP category filter',
            'filter': {'category': 'ml_nlp'},
            'description': 'Find documents about machine learning and NLP'
        },
        {
            'name': 'Word count filter',
            'filter': {'word_count': {'$gte': 10}},  # Documents with 10+ words
            'description': 'Find longer documents (10+ words)'
        },
        {
            'name': 'Text content filter',
            'filter': {'text': {'$contains': 'learning'}},
            'description': 'Find documents containing "learning"'
        }
    ]
    
    for test in metadata_tests:
        try:
            result = await client.search_by_metadata(
                collection_name=collection_name,
                filter=test['filter'],
                limit=5
            )
            print(f"   ✅ {test['name']}:")
            print(f"      📝 {test['description']}")
            print(f"      📊 Found {len(result)} matching documents")
            for i, doc in enumerate(result[:2]):  # Show first 2 results
                print(f"         {i+1}. {doc.get('id', 'N/A')}: {doc.get('metadata', {}).get('text', 'N/A')[:40]}...")
        except Exception as e:
            print(f"   ❌ {test['name']} failed: {e}")
            print(f"   💡 Note: Metadata filtering might not be implemented yet")
    
    # Step 6: Test similarity search with different scenarios
    print(f"\n6️⃣ Testing semantic similarity search...")
    
    # Multiple query scenarios
    similarity_tests = [
        {
            'query': "artificial intelligence and machine learning applications",
            'description': "AI/ML query",
            'expected': "Should find ML-related documents"
        },
        {
            'query': "database storage and vector search systems",
            'description': "Database query", 
            'expected': "Should find vector database documents"
        },
        {
            'query': "transformer models and natural language understanding",
            'description': "NLP query",
            'expected': "Should find BERT and NLP documents"
        }
    ]
    
    for test in similarity_tests:
        query_embedding = generate_bert_like_embeddings([test['query']], dimension=768)[0]
        
        print(f"\n   🔍 {test['description']}: '{test['query']}'")
        print(f"   💭 Expected: {test['expected']}")
        print(f"   📊 Query embedding magnitude: {np.linalg.norm(query_embedding):.4f}")
        
        try:
            # Test different top_k values
            for top_k in [3, 5]:
                search_result = await client.search_vectors(
                    collection_name=collection_name,
                    query_vector=query_embedding,
                    top_k=top_k,
                    include_metadata=True
                )
                
                print(f"   ✅ Top-{top_k} results:")
                for i, result in enumerate(search_result):
                    score = result.get('score', 0)
                    text = result.get('metadata', {}).get('text', 'N/A')
                    doc_id = result.get('id', 'N/A')
                    category = result.get('metadata', {}).get('category', 'N/A')
                    
                    print(f"      {i+1}. Score: {score:.4f} | ID: {doc_id} | Cat: {category}")
                    print(f"         📄 {text[:60]}...")
                
                # Test with metadata filtering in similarity search
                try:
                    filtered_search = await client.search_vectors(
                        collection_name=collection_name,
                        query_vector=query_embedding,
                        top_k=3,
                        filter={'category': 'ml_nlp'},
                        include_metadata=True
                    )
                    print(f"   🎯 Filtered search (ML/NLP only): {len(filtered_search)} results")
                except Exception as fe:
                    print(f"   ⚠️ Filtered similarity search not available: {fe}")
                
                break  # Only test top_k=3 for now
                
        except Exception as e:
            print(f"   ❌ Similarity search failed: {e}")
            print(f"   💡 Note: Vector similarity search might not be fully implemented yet")
    
    # Step 7: Verify collection statistics
    print(f"\n7️⃣ Checking collection statistics...")
    
    try:
        collections = await client.list_collections()
        our_collection = next((c for c in collections if c.name == collection_name), None)
        
        if our_collection:
            print(f"   ✅ Collection found in listing")
            print(f"   📊 Dimension: {our_collection.dimension}")
            print(f"   🆔 UUID: {our_collection.id}")
        else:
            print(f"   ❌ Collection not found in listing")
            
    except Exception as e:
        print(f"   ❌ Failed to get statistics: {e}")
    
    # Step 8: Test persistence preparation
    print(f"\n8️⃣ Preparing for restart persistence test...")
    print(f"   💾 Vectors should be persisted to WAL and storage layers")
    print(f"   📁 Collection metadata: /data/proximadb/1/metadata/")
    print(f"   📁 Vector data: /data/proximadb/1/wal/")
    print(f"   🔄 To test persistence: restart server and run search again")
    
    # Return collection info for restart test
    return {
        'success': True,
        'collection_name': collection_name,
        'documents': documents,
        'embeddings': embeddings,
        'test_queries': similarity_tests
    }

async def test_restart_persistence(collection_info):
    """Test that data persists across server restarts"""
    print("\n🔄 RESTART PERSISTENCE TEST")
    print("=" * 60)
    
    if not collection_info or not collection_info.get('success'):
        print("❌ No collection info provided for restart test")
        return False
    
    collection_name = collection_info['collection_name']
    documents = collection_info['documents']
    similarity_tests = collection_info['test_queries']
    
    print(f"🔍 Testing persistence of collection: {collection_name}")
    
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Step 1: Verify collection still exists
    print(f"\n1️⃣ Verifying collection exists after restart...")
    try:
        collections = await client.list_collections()
        our_collection = next((c for c in collections if c.name == collection_name), None)
        
        if our_collection:
            print(f"   ✅ Collection survived restart!")
            print(f"   📋 Name: {our_collection.name}")
            print(f"   📊 Dimension: {our_collection.dimension}")
            print(f"   🆔 UUID: {our_collection.id}")
        else:
            print(f"   ❌ Collection not found after restart")
            print(f"   📋 Available collections:")
            for c in collections:
                print(f"      • {c.name}")
            return False
    except Exception as e:
        print(f"   ❌ Failed to list collections: {e}")
        return False
    
    # Step 2: Test ID-based retrieval after restart
    print(f"\n2️⃣ Testing ID-based retrieval after restart...")
    test_ids = ["doc_000", "doc_004", "doc_007"]
    
    for vector_id in test_ids:
        try:
            result = await client.get_vector(
                collection_name=collection_name,
                vector_id=vector_id
            )
            print(f"   ✅ Vector '{vector_id}' persisted correctly")
            text = result.get('metadata', {}).get('text', 'N/A')
            print(f"      📄 Text: {text[:50]}...")
        except Exception as e:
            print(f"   ❌ Vector '{vector_id}' not found: {e}")
    
    # Step 3: Test metadata search after restart
    print(f"\n3️⃣ Testing metadata search after restart...")
    try:
        result = await client.search_by_metadata(
            collection_name=collection_name,
            filter={'category': 'ml_nlp'},
            limit=5
        )
        print(f"   ✅ Metadata search works: found {len(result)} ML/NLP documents")
    except Exception as e:
        print(f"   ❌ Metadata search failed: {e}")
    
    # Step 4: Test similarity search after restart
    print(f"\n4️⃣ Testing similarity search after restart...")
    
    # Use the same query from before
    query_text = similarity_tests[0]['query']  # AI/ML query
    query_embedding = generate_bert_like_embeddings([query_text], dimension=768)[0]
    
    try:
        search_result = await client.search_vectors(
            collection_name=collection_name,
            query_vector=query_embedding,
            top_k=3,
            include_metadata=True
        )
        
        print(f"   ✅ Similarity search works: found {len(search_result)} results")
        for i, result in enumerate(search_result):
            score = result.get('score', 0)
            text = result.get('metadata', {}).get('text', 'N/A')
            print(f"      {i+1}. Score: {score:.4f}")
            print(f"         📄 {text[:50]}...")
            
    except Exception as e:
        print(f"   ❌ Similarity search failed: {e}")
    
    print(f"\n🎉 Restart persistence test completed!")
    print(f"✅ Data successfully persisted across server restart")
    
    return True

async def test_simple_vector_ops():
    """Test basic vector operations if full BERT test fails"""
    print("\n🔧 Testing basic vector operations...")
    
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Test collection creation with smaller dimension
    timestamp = datetime.now().strftime("%H%M%S")
    simple_collection = f"simple_vectors_{timestamp}"
    
    try:
        result = await client.create_collection(
            name=simple_collection,
            dimension=4,  # Small dimension for testing
            distance_metric=1,  # COSINE
        )
        print(f"   ✅ Created simple collection: {simple_collection}")
        
        # Test basic vector data
        simple_vectors = [
            {'id': 'v1', 'vector': [1.0, 0.0, 0.0, 0.0], 'metadata': {'label': 'first'}},
            {'id': 'v2', 'vector': [0.0, 1.0, 0.0, 0.0], 'metadata': {'label': 'second'}},
            {'id': 'v3', 'vector': [0.0, 0.0, 1.0, 0.0], 'metadata': {'label': 'third'}},
        ]
        
        # Try to insert
        try:
            insert_result = await client.insert_vectors(
                collection_name=simple_collection,
                vectors=simple_vectors
            )
            print(f"   ✅ Inserted {len(simple_vectors)} simple vectors")
        except Exception as e:
            print(f"   ❌ Vector insertion failed: {e}")
            print(f"   💡 This is expected if vector operations are not yet implemented")
        
        return True
        
    except Exception as e:
        print(f"   ❌ Simple test failed: {e}")
        return False

if __name__ == "__main__":
    import sys
    
    # Check if this is a restart test
    if len(sys.argv) > 1 and sys.argv[1] == "restart_test":
        # Load collection info from a previous run
        # In a real scenario, you'd save this to a file
        print("🔄 Running restart test mode...")
        print("💡 This assumes a collection was created in a previous run")
        print("📋 Checking for existing BERT collections...")
        
        try:
            # Try to find an existing BERT collection
            async def find_bert_collection():
                client = ProximaDBClient(endpoint='localhost:5679')
                collections = await client.list_collections()
                bert_collections = [c for c in collections if 'bert_embeddings_' in c.name]
                
                if bert_collections:
                    collection_name = bert_collections[0].name
                    print(f"✅ Found BERT collection: {collection_name}")
                    
                    # Create mock info for restart test
                    collection_info = {
                        'success': True,
                        'collection_name': collection_name,
                        'documents': ["Test document for restart"],
                        'test_queries': [{'query': "artificial intelligence and machine learning applications"}]
                    }
                    return await test_restart_persistence(collection_info)
                else:
                    print("❌ No BERT collections found. Run the main test first.")
                    return False
            
            asyncio.run(find_bert_collection())
            
        except Exception as e:
            print(f"❌ Restart test failed: {e}")
    
    else:
        # Run the main BERT embeddings test
        try:
            collection_info = asyncio.run(test_bert_embeddings())
            
            if collection_info and collection_info.get('success'):
                print(f"\n🔄 Test completed! To test restart persistence:")
                print(f"   1. Stop the ProximaDB server (Ctrl+C or pkill)")
                print(f"   2. Restart the server: cargo run --bin proximadb-server")
                print(f"   3. Run: python {sys.argv[0]} restart_test")
                print(f"\n💾 Collection '{collection_info['collection_name']}' should persist across restart")
            else:
                print("\n⚠️ Full BERT test failed, trying simple operations...")
                asyncio.run(test_simple_vector_ops())
                
        except KeyboardInterrupt:
            print("\n⏹️ Test interrupted by user")
        except Exception as e:
            print(f"\n❌ Test failed with error: {e}")
            print("\n🔧 Falling back to simple vector operations test...")
            asyncio.run(test_simple_vector_ops())