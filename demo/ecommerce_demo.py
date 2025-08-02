#!/usr/bin/env python3
"""
ProximaDB Consolidated E-commerce Demo - Real BERT Embeddings with High Performance
=====================================================================================

This demo showcases:
- Real BERT embeddings for semantic product search
- High-performance gRPC for insert/upsert operations 
- Flexible search: REST, gRPC, and SQL options
- Natural language product queries
- Rich metadata filtering and analytics

Prerequisites:
    pip install sentence-transformers proximadb-client

Usage:
    python ecommerce_demo.py
    
Web UI Integration:
    Run serve_ui.py and visit http://localhost:8090 after this demo completes
"""

import time
import json
import requests
import numpy as np
from typing import List, Dict, Optional
from sentence_transformers import SentenceTransformer
from proximadb import connect_grpc, connect_rest
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord

# Comprehensive real product dataset for semantic search
REAL_PRODUCTS = [
    # Electronics - Laptops
    {"id": "laptop_001", "name": "Dell XPS 13 Plus", "category": "Electronics", "subcategory": "Laptops",
     "description": "Ultra-slim laptop with 12th Gen Intel Core i7, 13.4-inch 4K+ touchscreen, 16GB LPDDR5 RAM, 512GB SSD. Features edge-to-edge InfinityEdge display, premium aluminum construction, and exceptional battery life for professionals.",
     "brand": "Dell", "price": 1649.99, "rating": 4.6, "in_stock": True, "tags": ["premium", "business", "4K", "touchscreen"]},
    
    {"id": "laptop_002", "name": "MacBook Air M2", "category": "Electronics", "subcategory": "Laptops", 
     "description": "Revolutionary MacBook Air with Apple M2 chip, 13.6-inch Liquid Retina display, 8-core CPU, 8-core GPU, 16-core Neural Engine, 8GB unified memory, 256GB SSD. Silent fanless design with up to 18 hours battery life.",
     "brand": "Apple", "price": 1199.99, "rating": 4.8, "in_stock": True, "tags": ["M2", "fanless", "premium", "mac"]},
    
    {"id": "laptop_003", "name": "ASUS ROG Strix G15", "category": "Electronics", "subcategory": "Laptops",
     "description": "Gaming laptop powered by AMD Ryzen 9 5900HX, NVIDIA GeForce RTX 3070, 15.6-inch 300Hz IPS display, 16GB DDR4 RAM, 1TB NVMe SSD. Advanced cooling system and RGB backlit keyboard for serious gamers.",
     "brand": "ASUS", "price": 1599.99, "rating": 4.5, "in_stock": True, "tags": ["gaming", "RGB", "high-refresh", "RTX"]},
    
    # Electronics - Smartphones
    {"id": "phone_001", "name": "iPhone 14 Pro Max", "category": "Electronics", "subcategory": "Smartphones",
     "description": "Pro camera system with 48MP Main camera, Dynamic Island, A16 Bionic chip, 6.7-inch Super Retina XDR display with ProMotion, 5G connectivity, Face ID, and Ceramic Shield front. Available in Space Black, Silver, Gold, Deep Purple.",
     "brand": "Apple", "price": 1099.99, "rating": 4.7, "in_stock": True, "tags": ["48MP", "Dynamic Island", "Pro", "5G"]},
    
    {"id": "phone_002", "name": "Samsung Galaxy S23 Ultra", "category": "Electronics", "subcategory": "Smartphones",
     "description": "Premium Android smartphone with 200MP camera, built-in S Pen, 6.8-inch Dynamic AMOLED 2X display, Snapdragon 8 Gen 2, 12GB RAM, 256GB storage. Features advanced night photography and 100x Space Zoom.",
     "brand": "Samsung", "price": 1199.99, "rating": 4.6, "in_stock": True, "tags": ["200MP", "S Pen", "Android", "zoom"]},
    
    {"id": "phone_003", "name": "Google Pixel 7 Pro", "category": "Electronics", "subcategory": "Smartphones",
     "description": "Google's flagship with advanced computational photography, Google Tensor G2 chip, 6.7-inch LTPO OLED display, 12GB RAM, 128GB storage. Features Magic Eraser, Real Tone, and 5 years of security updates.",
     "brand": "Google", "price": 899.99, "rating": 4.5, "in_stock": True, "tags": ["computational photography", "Tensor", "Magic Eraser", "Android"]},
    
    # Fashion - Athletic Wear
    {"id": "shoes_001", "name": "Nike Air Jordan 1 Retro High", "category": "Fashion", "subcategory": "Sneakers",
     "description": "Iconic basketball sneaker with premium leather upper, Air-Sole unit cushioning, rubber outsole with pivot circle. Classic colorway in Chicago Bulls red, black, and white. Timeless style meets modern comfort.",
     "brand": "Nike", "price": 170.00, "rating": 4.8, "in_stock": True, "tags": ["basketball", "retro", "leather", "classic"]},
    
    {"id": "shoes_002", "name": "Adidas Ultraboost 22", "category": "Fashion", "subcategory": "Running Shoes",
     "description": "High-performance running shoes with BOOST midsole, Primeknit+ upper, Continental rubber outsole, and Torsion Spring for energy return. Designed for long-distance running with superior comfort and responsiveness.",
     "brand": "Adidas", "price": 190.00, "rating": 4.6, "in_stock": True, "tags": ["running", "BOOST", "comfort", "long-distance"]},
    
    # Home & Kitchen
    {"id": "coffee_001", "name": "Breville Barista Express Espresso Machine", "category": "Home", "subcategory": "Coffee",
     "description": "Professional-grade espresso machine with built-in conical burr grinder, 15-bar Italian pump, precise temperature control, milk steam wand, and stainless steel construction. Perfect for coffee enthusiasts.",
     "brand": "Breville", "price": 699.95, "rating": 4.4, "in_stock": True, "tags": ["espresso", "grinder", "professional", "barista"]},
    
    {"id": "vacuum_001", "name": "Dyson V15 Detect Cordless Vacuum", "category": "Home", "subcategory": "Cleaning",
     "description": "Advanced cordless vacuum with laser dust detection, intelligent suction adjustment, 60-minute runtime, HEPA filtration, and lightweight design. Transforms to handheld for versatile cleaning.",
     "brand": "Dyson", "price": 749.99, "rating": 4.5, "in_stock": True, "tags": ["cordless", "laser", "HEPA", "lightweight"]},
    
    # Sports & Outdoors
    {"id": "bike_001", "name": "Trek Domane SL 5 Road Bike", "category": "Sports", "subcategory": "Cycling",
     "description": "High-performance carbon road bike with Shimano 105 groupset, endurance geometry, IsoSpeed decoupler for comfort, tubeless-ready wheels, and aerodynamic frame design for serious cyclists.",
     "brand": "Trek", "price": 2999.99, "rating": 4.7, "in_stock": True, "tags": ["carbon", "road", "Shimano", "endurance"]},
    
    {"id": "tent_001", "name": "REI Co-op Half Dome 4 Plus Tent", "category": "Sports", "subcategory": "Camping",
     "description": "Spacious 4-person tent with easy setup, excellent ventilation, rainfly coverage, gear loft, and color-coded clips. Perfect for family camping with ample headroom and vestibule space.",
     "brand": "REI", "price": 349.00, "rating": 4.6, "in_stock": True, "tags": ["4-person", "family", "camping", "spacious"]},
    
    # Beauty & Personal Care
    {"id": "skincare_001", "name": "The Ordinary Niacinamide 10% + Zinc 1%", "category": "Beauty", "subcategory": "Skincare",
     "description": "High-strength vitamin B3 serum for reducing appearance of blemishes, regulating sebum production, and improving skin texture. Water-based formula suitable for all skin types.",
     "brand": "The Ordinary", "price": 7.90, "rating": 4.3, "in_stock": True, "tags": ["vitamin B3", "serum", "blemishes", "affordable"]},
    
    {"id": "perfume_001", "name": "Chanel No. 5 Eau de Parfum", "category": "Beauty", "subcategory": "Fragrance",
     "description": "Iconic feminine fragrance with floral aldehydic composition, featuring ylang-ylang, rose, iris, and sandalwood. Timeless elegance in classic bottle design since 1921.",
     "brand": "Chanel", "price": 165.00, "rating": 4.8, "in_stock": True, "tags": ["iconic", "floral", "luxury", "classic"]},
    
    # Books & Media
    {"id": "book_001", "name": "Atomic Habits by James Clear", "category": "Books", "subcategory": "Self-Help",
     "description": "Practical guide for building good habits and breaking bad ones through proven strategies, scientific research, and real-world examples. Transform your life with tiny changes that deliver remarkable results.",
     "brand": "Avery", "price": 16.99, "rating": 4.8, "in_stock": True, "tags": ["habits", "self-improvement", "bestseller", "practical"]},
    
    {"id": "book_002", "name": "Dune by Frank Herbert", "category": "Books", "subcategory": "Science Fiction",
     "description": "Epic science fiction novel set on desert planet Arrakis, following Paul Atreides in a saga of politics, religion, and ecology. Winner of Hugo and Nebula awards, basis for acclaimed films.",
     "brand": "Ace Books", "price": 17.99, "rating": 4.7, "in_stock": True, "tags": ["sci-fi", "epic", "classic", "award-winning"]},
]

class EcommerceDemo:
    """Consolidated E-commerce Demo with Real BERT Embeddings"""
    
    def __init__(self):
        self.collection_id = "ecommerce_products"
        self.embedding_model = None
        self.grpc_client = None
        self.rest_client = None
        self.api_base = "http://localhost:5678"
        
    def init_connections(self):
        """Initialize both gRPC and REST connections"""
        print("🔌 Connecting to ProximaDB...")
        
        try:
            # gRPC for high-performance inserts
            self.grpc_client = connect_grpc("grpc://localhost:5679")
            print("✅ gRPC connection established (for inserts/upserts)")
        except Exception as e:
            print(f"⚠️  gRPC connection failed: {e}")
            
        try:
            # REST for flexible operations
            self.rest_client = connect_rest("http://localhost:5678")
            print("✅ REST connection established (for search/queries)")
        except Exception as e:
            print(f"⚠️  REST connection failed: {e}")
            
        if not self.grpc_client and not self.rest_client:
            raise Exception("❌ No ProximaDB connections available")
    
    def load_embedding_model(self):
        """Load BERT model for text embeddings"""
        print("🤖 Loading BERT model (all-MiniLM-L6-v2)...")
        try:
            self.embedding_model = SentenceTransformer('all-MiniLM-L6-v2')
            print("✅ BERT model loaded successfully")
        except Exception as e:
            raise Exception(f"❌ Failed to load BERT model: {e}")
    
    def create_embeddings(self, texts: List[str]) -> np.ndarray:
        """Generate BERT embeddings for text list"""
        return self.embedding_model.encode(texts, convert_to_numpy=True)
    
    def setup_collection(self):
        """Create collection with optimal configuration"""
        print(f"\n📦 Setting up collection: {self.collection_id}")
        
        # Try REST first for collection creation (more reliable)
        if self.rest_client:
            try:
                config = CollectionConfig(
                    name=self.collection_id,  # Required field
                    dimension=384,  # all-MiniLM-L6-v2 dimension
                    distance_metric=DistanceMetric.COSINE,
                    engine="viper"  # Columnar engine for analytics
                )
                
                result = self.rest_client.create_collection(self.collection_id, config)
                print(f"✅ Collection created via REST: {result}")
                return
                
            except Exception as e:
                if "already exists" in str(e).lower():
                    print("✅ Collection already exists")
                    return
                else:
                    print(f"⚠️  REST collection creation failed: {e}")
        
        # Fallback to gRPC if REST fails
        if self.grpc_client:
            try:
                config = CollectionConfig(
                    name=self.collection_id,
                    dimension=384,
                    distance_metric=DistanceMetric.COSINE,
                    engine="viper"
                )
                
                result = self.grpc_client.create_collection(self.collection_id, config)
                print(f"✅ Collection created via gRPC: {result}")
                
            except Exception as e:
                if "already exists" in str(e).lower():
                    print("✅ Collection already exists")
                else:
                    print(f"❌ gRPC collection creation failed: {e}")
                    raise
        else:
            raise Exception("❌ No client available for collection creation")
    
    def insert_products_grpc(self):
        """High-performance product insertion via gRPC"""
        if not self.grpc_client:
            print("⚠️  gRPC not available, skipping bulk insert")
            return
            
        print(f"\n⚡ Inserting {len(REAL_PRODUCTS)} products via gRPC...")
        start_time = time.time()
        
        # Generate embeddings for all product descriptions
        descriptions = [f"{p['name']} {p['description']}" for p in REAL_PRODUCTS]
        embeddings = self.create_embeddings(descriptions)
        
        # Create vector records
        vectors = []
        for i, product in enumerate(REAL_PRODUCTS):
            # Convert metadata to list of key-value pairs
            metadata = [
                {"key": "name", "value": product["name"]},
                {"key": "category", "value": product["category"]},
                {"key": "subcategory", "value": product["subcategory"]},
                {"key": "brand", "value": product["brand"]},
                {"key": "price", "value": str(product["price"])},
                {"key": "rating", "value": str(product["rating"])},
                {"key": "in_stock", "value": str(product["in_stock"])},
                {"key": "description", "value": product["description"][:200] + "..."},  # Truncate for metadata
                {"key": "tags", "value": ",".join(product.get("tags", []))}
            ]
            
            vector = VectorRecord(
                id=product["id"],
                vector=embeddings[i].tolist(),
                metadata=metadata
            )
            vectors.append(vector)
        
        try:
            result = self.grpc_client.insert_vectors(self.collection_id, vectors)
            elapsed = time.time() - start_time
            rate = len(vectors) / elapsed
            print(f"✅ Inserted {len(vectors)} products in {elapsed:.2f}s ({rate:.0f} products/sec)")
            print(f"📊 Performance: {result}")
            
        except Exception as e:
            print(f"❌ gRPC insert failed: {e}")
            print("   Falling back to REST insertion...")
            self.insert_products_rest()
    
    def insert_products_rest(self):
        """Fallback product insertion via REST"""
        if not self.rest_client:
            print("❌ No REST client available")
            return
            
        print(f"\n🌐 Inserting products via REST API...")
        
        # Batch insert for better performance
        batch_size = 5
        total_inserted = 0
        
        for i in range(0, len(REAL_PRODUCTS), batch_size):
            batch = REAL_PRODUCTS[i:i + batch_size]
            descriptions = [f"{p['name']} {p['description']}" for p in batch]
            embeddings = self.create_embeddings(descriptions)
            
            vectors = []
            for j, product in enumerate(batch):
                metadata = [
                    {"key": "name", "value": product["name"]},
                    {"key": "category", "value": product["category"]},
                    {"key": "brand", "value": product["brand"]},
                    {"key": "price", "value": str(product["price"])},
                    {"key": "rating", "value": str(product["rating"])}
                ]
                
                vector = VectorRecord(
                    id=product["id"],
                    vector=embeddings[j].tolist(),
                    metadata=metadata
                )
                vectors.append(vector)
            
            try:
                result = self.rest_client.insert_vectors(self.collection_id, vectors)
                total_inserted += len(vectors)
                print(f"✅ Batch {i//batch_size + 1}: {len(vectors)} products")
                
            except Exception as e:
                print(f"❌ REST batch insert failed: {e}")
        
        print(f"✅ Total inserted via REST: {total_inserted} products")
    
    def search_grpc(self, query: str, top_k: int = 5) -> Dict:
        """Semantic search via gRPC"""
        if not self.grpc_client:
            return {"error": "gRPC not available"}
            
        print(f"\n🔍 gRPC Search: '{query}'")
        start_time = time.time()
        
        try:
            query_embedding = self.create_embeddings([query])[0]
            results = self.grpc_client.search_vectors(
                self.collection_id,
                query_embedding.tolist(),
                top_k=top_k
            )
            
            elapsed = time.time() - start_time
            print(f"⚡ gRPC search completed in {elapsed*1000:.1f}ms")
            return {"results": results, "latency_ms": elapsed*1000, "method": "gRPC"}
            
        except Exception as e:
            print(f"❌ gRPC search failed: {e}")
            return {"error": str(e)}
    
    def search_rest(self, query: str, top_k: int = 5) -> Dict:
        """Semantic search via REST API"""
        print(f"\n🌐 REST Search: '{query}'")
        start_time = time.time()
        
        try:
            query_embedding = self.create_embeddings([query])[0]
            
            payload = {
                "vector": query_embedding.tolist(),
                "top_k": top_k,
                "distance_metric": "cosine"
            }
            
            response = requests.post(
                f"{self.api_base}/collections/{self.collection_id}/search",
                json=payload,
                headers={"Content-Type": "application/json"}
            )
            
            elapsed = time.time() - start_time
            
            if response.status_code == 200:
                results = response.json()
                print(f"🌐 REST search completed in {elapsed*1000:.1f}ms")
                return {"results": results, "latency_ms": elapsed*1000, "method": "REST"}
            else:
                return {"error": f"HTTP {response.status_code}: {response.text}"}
                
        except Exception as e:
            print(f"❌ REST search failed: {e}")
            return {"error": str(e)}
    
    def search_sql(self, query: str, top_k: int = 5) -> Dict:
        """Semantic search via SQL interface"""
        print(f"\n📊 SQL Search: '{query}'")
        start_time = time.time()
        
        try:
            query_embedding = self.create_embeddings([query])[0]
            vector_str = "[" + ",".join(map(str, query_embedding)) + "]"
            
            sql_query = f"""
            SELECT id, metadata
            FROM {self.collection_id}
            ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
            LIMIT {top_k}
            """
            
            payload = {"query": sql_query}
            response = requests.post(
                f"{self.api_base}/sql",
                json=payload,
                headers={"Content-Type": "application/json"}
            )
            
            elapsed = time.time() - start_time
            
            if response.status_code == 200:
                results = response.json()
                print(f"📊 SQL search completed in {elapsed*1000:.1f}ms")
                return {"results": results, "latency_ms": elapsed*1000, "method": "SQL"}
            else:
                return {"error": f"HTTP {response.status_code}: {response.text}"}
                
        except Exception as e:
            print(f"❌ SQL search failed: {e}")
            return {"error": str(e)}
    
    def display_results(self, search_result: Dict, query: str):
        """Format and display search results"""
        if "error" in search_result:
            print(f"❌ Search Error: {search_result['error']}")
            return
            
        method = search_result.get("method", "Unknown")
        latency = search_result.get("latency_ms", 0)
        results = search_result.get("results", {})
        
        print(f"\n📋 Results for '{query}' via {method} ({latency:.1f}ms):")
        print("=" * 60)
        
        # Handle different result formats
        if isinstance(results, dict) and "results" in results:
            items = results["results"]
        elif isinstance(results, list):
            items = results
        else:
            items = []
        
        for i, item in enumerate(items[:5], 1):
            score = item.get("score", item.get("distance", "N/A"))
            vector_id = item.get("id", "N/A")
            
            # Extract metadata
            metadata = item.get("metadata", {})
            if isinstance(metadata, list):
                # Convert list format to dict
                meta_dict = {m.get("key", ""): m.get("value", "") for m in metadata}
            else:
                meta_dict = metadata
            
            name = meta_dict.get("name", "Unknown Product")
            brand = meta_dict.get("brand", "Unknown")
            price = meta_dict.get("price", "N/A")
            category = meta_dict.get("category", "N/A")
            
            print(f"{i}. {name}")
            print(f"   Brand: {brand} | Category: {category} | Price: ${price}")
            print(f"   Score: {score} | ID: {vector_id}")
            print()
    
    def run_demo_queries(self):
        """Run comprehensive demo with various search methods"""
        queries = [
            "powerful laptop for programming",
            "comfortable running shoes", 
            "professional coffee machine",
            "luxury skincare products",
            "outdoor camping gear"
        ]
        
        print("\n" + "="*70)
        print("🎯 SEMANTIC SEARCH DEMO - Multiple Methods Comparison")
        print("="*70)
        
        for query in queries:
            print(f"\n🔍 Query: '{query}'")
            print("-" * 50)
            
            # Try different search methods
            grpc_result = self.search_grpc(query, top_k=3)
            self.display_results(grpc_result, query)
            
            rest_result = self.search_rest(query, top_k=3)
            self.display_results(rest_result, query)
            
            sql_result = self.search_sql(query, top_k=3)
            self.display_results(sql_result, query)
            
            time.sleep(1)  # Brief pause between queries
    
    def export_for_ui(self):
        """Export sample data for Web UI integration"""
        print("\n📤 Exporting data for Web UI...")
        
        ui_data = {
            "collection_id": self.collection_id,
            "sample_queries": [
                "gaming laptop with RGB",
                "wireless headphones",
                "running shoes for marathon",
                "coffee machine with grinder",
                "camping tent for family"
            ],
            "products_count": len(REAL_PRODUCTS),
            "embedding_dimension": 384,
            "api_endpoints": {
                "rest": self.api_base,
                "grpc": "localhost:5679",
                "search_rest": f"{self.api_base}/collections/{self.collection_id}/search",
                "sql": f"{self.api_base}/sql"
            }
        }
        
        with open("/home/vsingh/code/proximaDB/demo/ui_integration.json", "w") as f:
            json.dump(ui_data, f, indent=2)
        
        print("✅ UI integration data exported to demo/ui_integration.json")
        print("🌐 Start Web UI with: python demo/serve_ui.py")
        print("   Then visit: http://localhost:8090")

def main():
    """Main demo execution"""
    print("🛍️  ProximaDB E-commerce Demo - Real BERT Embeddings")
    print("=====================================================")
    print("Features:")
    print("• Real BERT embeddings with all-MiniLM-L6-v2")
    print("• High-performance gRPC for inserts/upserts")
    print("• Flexible search: REST, gRPC, and SQL options")
    print("• Comprehensive product catalog with metadata")
    print("• Web UI integration ready")
    print()
    
    demo = EcommerceDemo()
    
    try:
        # Initialize components
        demo.load_embedding_model()
        demo.init_connections()
        demo.setup_collection()
        
        # Insert products using high-performance gRPC
        demo.insert_products_grpc()
        
        # Run demonstration queries
        demo.run_demo_queries()
        
        # Export for UI integration
        demo.export_for_ui()
        
        print("\n" + "="*70)
        print("🎉 Demo completed successfully!")
        print("✅ Real BERT embeddings generated")
        print("✅ High-performance gRPC insertions") 
        print("✅ Multi-protocol search demonstration")
        print("✅ Web UI integration data exported")
        print()
        print("Next steps:")
        print("1. Run: python demo/serve_ui.py")
        print("2. Visit: http://localhost:8090") 
        print("3. Try natural language product searches!")
        print("="*70)
        
    except Exception as e:
        print(f"\n❌ Demo failed: {e}")
        print("\nTroubleshooting:")
        print("1. Ensure ProximaDB server is running: ./target/release/proximadb-server")
        print("2. Install dependencies: pip install sentence-transformers proximadb-client")
        print("3. Check server logs for errors")

if __name__ == "__main__":
    main()