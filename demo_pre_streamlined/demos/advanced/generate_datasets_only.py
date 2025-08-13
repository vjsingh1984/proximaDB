#!/usr/bin/env python3
"""
Generate demo datasets without starting ProximaDB server
Creates JSON files that can be loaded later
"""

import json
import os
import sys
from pathlib import Path
import numpy as np
from datetime import datetime

# Add path utilities
sys.path.insert(0, str(Path(__file__).parent))
from utils.path_utils import setup_demo_environment

# Setup environment
env_info = setup_demo_environment()

# Import sentence transformers for embeddings
from sentence_transformers import SentenceTransformer
import torch

# Output directory
OUTPUT_DIR = Path("pre")

def main():
    """Main function to generate datasets"""
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    
    print("🚀 Generating demo datasets...")
    print("=" * 60)

    # Load BERT model for embeddings
    print("Loading BERT model...")
    torch.manual_seed(42)
    np.random.seed(42)
    model = SentenceTransformer('all-mpnet-base-v2')
    print("✅ Model loaded")

    # 1. Generate E-commerce Dataset
    print("\n📦 Generating e-commerce dataset (1200 products)...")

    ecommerce_data = []
    products = [
        # Laptops with subtypes
        ("laptop", "gaming", "High-performance gaming laptop with RTX graphics", 1200, 3500),
        ("laptop", "ultrabook", "Ultra-thin lightweight laptop for professionals", 800, 2000),
        ("laptop", "workstation", "Mobile workstation for CAD and 3D rendering", 1500, 4000),
        ("laptop", "budget", "Affordable laptop for students and basic tasks", 300, 700),
        ("laptop", "2-in-1", "Convertible laptop with touchscreen", 700, 1800),
        
        # Smartphones with subtypes
        ("smartphone", "flagship", "Premium flagship phone with latest features", 800, 1500),
        ("smartphone", "mid-range", "Balance of features and affordability", 300, 600),
        ("smartphone", "budget", "Essential smartphone for everyday use", 100, 300),
        ("smartphone", "gaming", "Gaming phone with cooling system", 600, 1200),
        ("smartphone", "rugged", "Durable phone for outdoor activities", 400, 800),
        
        # Headphones with subtypes
        ("headphones", "over-ear", "Premium over-ear headphones with ANC", 200, 600),
        ("headphones", "on-ear", "Portable on-ear headphones", 100, 300),
        ("headphones", "earbuds", "True wireless earbuds with charging case", 80, 350),
        ("headphones", "sports", "Sweat-resistant sports earphones", 50, 200),
        ("headphones", "studio", "Professional studio monitoring headphones", 150, 800),
        
        # Smartwatches with subtypes
        ("smartwatch", "fitness", "Advanced fitness tracking smartwatch", 200, 500),
        ("smartwatch", "luxury", "Premium smartwatch with luxury materials", 500, 2000),
        ("smartwatch", "basic", "Essential smartwatch features", 100, 250),
        ("smartwatch", "outdoor", "Rugged smartwatch for adventures", 300, 700),
        ("smartwatch", "fashion", "Stylish smartwatch with customizable bands", 150, 400),
        
        # Additional categories...
        ("tablet", "pro", "Professional tablet with stylus support", 600, 1500),
        ("camera", "dslr", "Professional DSLR with interchangeable lenses", 800, 5000),
        ("speaker", "smart", "AI-powered smart speaker", 50, 300),
        ("monitor", "gaming", "High refresh rate gaming monitor", 300, 1500),
        ("keyboard", "mechanical", "Mechanical gaming keyboard with RGB", 80, 300),
    ]

    brands = ["TechCorp", "ElectroMax", "SmartBrand", "ProGear", "NextGen", 
             "InnoTech", "FutureTech", "PrimeTech", "UltraGear", "MaxPro",
             "TechFlow", "DigiPro", "SmartCore", "TechVision", "ProLine"]

    # Generate 1200 products
    texts = []
    for i in range(1200):
        product_type, subtype, description, min_price, max_price = products[i % len(products)]
        brand = brands[i % len(brands)]
        
        model_num = f"{brand[:2].upper()}-{subtype[:3].upper()}-{1000 + i}"
        features = ["latest technology", "premium quality", "advanced features", "high performance",
                    "eco-friendly", "energy efficient", "award-winning design", "industry leading"]
        feature_text = ", ".join(features[j % len(features)] for j in range(i % 3 + 2))
        
        text = f"{brand} {product_type} {subtype} Model {model_num}: {description}. Features: {feature_text}"
        texts.append(text)
        
        product = {
            "id": f"ecommerce_demo_product_{i:04d}",
            "text": text,
            "category": "electronics",
            "subcategory": product_type,
            "subtype": subtype,
            "brand": brand,
            "model": model_num,
            "price": round(min_price + (i * 10.5) % (max_price - min_price), 2),
            "rating": round(3.5 + (i % 15) / 10.0, 1),
            "reviews": 10 + (i % 500),
            "in_stock": i % 3 != 0,
            "tags": ["featured", "sale"] if i % 4 == 0 else ["new", "trending"] if i % 5 == 0 else ["popular"],
            "created_at": f"2024-01-{(i % 30) + 1:02d}T10:00:00Z",
            "warehouse_location": ["US-West", "US-East", "EU-Central", "Asia-Pacific"][i % 4],
            "warranty_months": [12, 24, 36][i % 3],
            "color_options": ["Black", "Silver", "White", "Blue", "Red"][:(i % 3) + 2]
        }
        ecommerce_data.append(product)

    # Generate embeddings in batches
    print("Generating embeddings for e-commerce products...")
    embeddings = model.encode(texts, batch_size=32, show_progress_bar=True)

    # Add embeddings to products
    for i, product in enumerate(ecommerce_data):
        product["vector"] = embeddings[i].tolist()

    # Save e-commerce data
    with open(OUTPUT_DIR / "ecommerce_data.json", "w") as f:
        json.dump(ecommerce_data, f)
    print(f"✅ Saved {len(ecommerce_data)} e-commerce products")

    # 2. Generate SEC EDGAR Dataset
    print("\n📄 Generating SEC EDGAR dataset (S&P 50 companies)...")

    sec_data = []
    companies = [
        # Top Technology (15 companies)
        ("Apple Inc.", "AAPL"),
        ("Microsoft Corporation", "MSFT"),
        ("Alphabet Inc.", "GOOGL"),
        ("Amazon.com Inc.", "AMZN"),
        ("Meta Platforms Inc.", "META"),
        ("Tesla Inc.", "TSLA"),
        ("NVIDIA Corporation", "NVDA"),
        ("Broadcom Inc.", "AVGO"),
        ("Oracle Corporation", "ORCL"),
        ("Adobe Inc.", "ADBE"),
        ("Salesforce Inc.", "CRM"),
        ("Cisco Systems Inc.", "CSCO"),
        ("Intel Corporation", "INTC"),
        ("Advanced Micro Devices", "AMD"),
        ("Qualcomm Inc.", "QCOM"),
        
        # Top Finance (10 companies)
        ("Berkshire Hathaway", "BRK.B"),
        ("JPMorgan Chase & Co.", "JPM"),
        ("Visa Inc.", "V"),
        ("Mastercard Inc.", "MA"),
        ("Bank of America", "BAC"),
        ("Wells Fargo & Company", "WFC"),
        ("Goldman Sachs Group", "GS"),
        ("Morgan Stanley", "MS"),
        ("American Express", "AXP"),
        ("BlackRock Inc.", "BLK"),
        
        # Top Healthcare (10 companies)
        ("UnitedHealth Group", "UNH"),
        ("Johnson & Johnson", "JNJ"),
        ("Eli Lilly and Company", "LLY"),
        ("AbbVie Inc.", "ABBV"),
        ("Pfizer Inc.", "PFE"),
        ("Merck & Co.", "MRK"),
        ("Thermo Fisher Scientific", "TMO"),
        ("Abbott Laboratories", "ABT"),
        ("Danaher Corporation", "DHR"),
        ("CVS Health Corporation", "CVS"),
        
        # Top Consumer (10 companies)
        ("Walmart Inc.", "WMT"),
        ("Home Depot Inc.", "HD"),
        ("Procter & Gamble", "PG"),
        ("Coca-Cola Company", "KO"),
        ("PepsiCo Inc.", "PEP"),
        ("Costco Wholesale", "COST"),
        ("McDonald's Corporation", "MCD"),
        ("Nike Inc.", "NKE"),
        ("Starbucks Corporation", "SBUX"),
        ("Disney Company", "DIS"),
    ]

    sections = ["business_overview", "risk_factors", "financial_performance", "legal_proceedings"]

    # Generate documents and chunks
    chunk_size = 512
    overlap = 64
    chunk_id = 0

    for company_name, ticker in companies:
        for section in sections:
            # Generate section content (simplified version)
            content = f"""
{section.upper().replace('_', ' ')}

{company_name} ({ticker}) operates in a dynamic market environment. This section provides detailed information about {section.replace('_', ' ')}.

Our company has demonstrated strong performance in recent years, with significant growth in key metrics. We continue to invest in innovation and strategic initiatives to maintain our competitive position.

The {section.replace('_', ' ')} analysis reveals important insights about our operations, challenges, and opportunities. We have implemented various strategies to address market dynamics and regulatory requirements.

Key highlights include our focus on technology advancement, operational efficiency, and sustainable growth. Our teams work diligently to ensure compliance with all applicable regulations while pursuing strategic objectives.

Market conditions continue to evolve, presenting both opportunities and challenges. We maintain a disciplined approach to risk management and capital allocation to create long-term value for stakeholders.

Additional details about {section.replace('_', ' ')} are provided in subsequent sections of this filing. We encourage investors to review all relevant information when evaluating our business.
"""
            
            # Extend content to reach ~32KB
            while len(content.encode('utf-8')) < 32 * 1024:
                content += f"\n\nFurther analysis of {section.replace('_', ' ')} for {company_name} reveals additional considerations. "
                content += "Our strategic initiatives continue to drive value creation across multiple dimensions. "
                content += "We remain focused on operational excellence and sustainable growth strategies. "
            
            # Create chunks
            for i in range(0, len(content), chunk_size - overlap):
                chunk_text = content[i:i + chunk_size]
                if len(chunk_text.strip()) < 50:  # Skip very small chunks
                    continue
                    
                chunk_data = {
                    "id": f"sec_edgar_{ticker}_{section}_chunk_{chunk_id}",
                    "text": chunk_text,
                    "company": company_name,
                    "ticker": ticker,
                    "filing_type": "10-K" if section != "legal_proceedings" else "8-K",
                    "section": section,
                    "filing_date": datetime.now().strftime("%Y-%m-%d"),
                    "fiscal_year": 2023,
                    "chunk_index": chunk_id,
                    "document_id": f"{ticker}_{section}"
                }
                
                sec_data.append(chunk_data)
                chunk_id += 1

    # Generate embeddings for SEC chunks
    print(f"Generating embeddings for {len(sec_data)} SEC EDGAR chunks...")
    sec_texts = [chunk["text"] for chunk in sec_data]

    # Process in batches to avoid memory issues
    batch_size = 100
    all_embeddings = []

    for i in range(0, len(sec_texts), batch_size):
        batch_texts = sec_texts[i:i + batch_size]
        batch_embeddings = model.encode(batch_texts, batch_size=32, show_progress_bar=False)
        all_embeddings.extend(batch_embeddings)
        print(f"  Processed {min(i + batch_size, len(sec_texts))}/{len(sec_texts)} chunks")

    # Add embeddings to chunks
    for i, chunk in enumerate(sec_data):
        chunk["vector"] = all_embeddings[i].tolist()

    # Save SEC data
    with open(OUTPUT_DIR / "sec_edgar_data.json", "w") as f:
        json.dump(sec_data, f)
    print(f"✅ Saved {len(sec_data)} SEC EDGAR chunks")

    # 3. Generate Knowledge Base Dataset
    print("\n📚 Generating knowledge base dataset...")

    kb_data = []
    topics = [
        "Vector databases and their applications in AI",
        "Machine learning embeddings and semantic search",
        "Natural language processing with transformers",
        "Building scalable search systems",
        "Real-time data processing architectures",
        "Distributed computing and data sharding strategies",
        "Query optimization techniques for large-scale databases",
        "Hardware acceleration for vector operations",
        "Cloud-native architectures for modern applications",
        "Security and privacy in vector databases",
        "Multi-modal embeddings for image and text search",
        "Graph databases and knowledge representation",
        "Time-series data analysis with vectors",
        "Recommendation systems using embeddings",
        "Anomaly detection with vector similarity"
    ]

    # Generate more comprehensive knowledge base content
    kb_texts = []
    for i in range(256):  # 256 chunks
        topic = topics[i % len(topics)]
        doc_id = f"doc_{i // 16:03d}"  # 16 chunks per document
        
        # Create more detailed text content
        text = f"""
Knowledge Base Article {doc_id} - Chunk {i % 16 + 1}

Topic: {topic}

This section provides in-depth analysis and insights about {topic.lower()}. 
The content covers theoretical foundations, practical implementations, and real-world applications.

Key concepts include advanced techniques, optimization strategies, and best practices 
for implementing {topic.lower()} in production environments. Organizations can leverage 
these approaches to build scalable, efficient, and reliable systems.

Recent developments in the field have shown significant improvements in performance 
and accuracy. Research indicates that modern approaches can achieve up to 10x better 
results compared to traditional methods.

Implementation considerations include resource requirements, scalability factors, 
and integration patterns. Proper planning and architecture design are essential 
for successful deployment.
"""
        kb_texts.append(text.strip())
        
        kb_item = {
            "id": f"knowledge_base_chunk_{i:04d}",
            "text": text.strip(),
            "document_type": ["article", "tutorial", "reference", "guide", "whitepaper"][i % 5],
            "source": ["docs", "blog", "paper", "wiki", "forum"][i % 5],
            "chunk_index": i % 16,
            "document_id": doc_id,
            "language": "en",
            "confidence_score": 0.7 + (i % 30) / 100.0,
            "author": ["AI Research Team", "Data Science Group", "Engineering Team", "Product Team"][i % 4],
            "last_updated": f"2024-01-{(i % 30) + 1:02d}",
            "tags": ["beginner", "intermediate", "advanced"][i % 3]
        }
        kb_data.append(kb_item)

    # Generate embeddings
    print("Generating embeddings for knowledge base...")
    kb_embeddings = model.encode(kb_texts, show_progress_bar=True)

    for i, item in enumerate(kb_data):
        item["vector"] = kb_embeddings[i].tolist()

    # Save knowledge base data
    with open(OUTPUT_DIR / "knowledge_base_data.json", "w") as f:
        json.dump(kb_data, f)
    print(f"✅ Saved {len(kb_data)} knowledge base chunks")

    print("\n✅ All datasets generated successfully!")
    print(f"📁 Output directory: {OUTPUT_DIR.absolute()}")
    print(f"   - E-commerce: {len(ecommerce_data)} products")
    print(f"   - SEC EDGAR: {len(sec_data)} chunks")
    print(f"   - Knowledge Base: {len(kb_data)} chunks")


if __name__ == "__main__":
    main()