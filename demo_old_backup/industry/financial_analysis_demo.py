#!/usr/bin/env python3
"""
ProximaDB Financial Document Analysis Demo - 13F Filings Intelligence

This demo showcases ProximaDB's capabilities for financial document analysis:
- Extract Apple (AAPL) data from SEC 13F filings
- Process documents with sliding window chunking
- Generate BERT embeddings for semantic search
- Search for key financial insights and trends
- Demonstrate real-world financial intelligence use cases
"""

import time
import logging
import numpy as np
import sys
import os
import json
import requests
from pathlib import Path
from typing import List, Dict, Any, Optional
from datetime import datetime, timedelta
import re

# Financial data processing
try:
    import pandas as pd
    import yfinance as yf
    from sentence_transformers import SentenceTransformer
    FINANCIAL_LIBS_AVAILABLE = True
except ImportError:
    FINANCIAL_LIBS_AVAILABLE = False
    print("⚠️ Optional financial libraries not available. Using mock data for demo.")

# Import ProximaDB SDK
from proximadb import (
    connect_rest, connect_grpc, CollectionConfig, DistanceMetric,
    TextChunker, ChunkingStrategy, ChunkingConfig,
    chunk_sliding_window, chunk_by_sentences
)

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Sample 13F filing data (mock data for demo purposes)
SAMPLE_13F_FILINGS = {
    "Q3_2024": {
        "filing_date": "2024-11-01",
        "period_end": "2024-09-30",
        "total_value": 29800000000,
        "aapl_holdings": {
            "shares": 915560000,
            "value": 20500000000,
            "percentage": 68.8
        },
        "document_text": """
        FORM 13F INFORMATION TABLE
        
        Investment Manager: Berkshire Hathaway Inc.
        Report Period: September 30, 2024
        
        APPLE INC (AAPL) - Common Stock Analysis:
        Berkshire Hathaway's position in Apple Inc represents our largest equity holding at 915.56 million shares, 
        valued at approximately $20.5 billion as of September 30, 2024. This represents 68.8% of our total 
        portfolio value and demonstrates our continued confidence in Apple's business model and competitive moat.
        
        Apple's strong fundamentals continue to support our investment thesis. The company maintains exceptional 
        cash generation capabilities, with free cash flow of $26.3 billion in Q3 2024. Management's capital 
        allocation strategy remains disciplined, balancing growth investments with shareholder returns through 
        dividends and share repurchases.
        
        Key Investment Highlights for AAPL:
        - Services revenue growth of 14% year-over-year, reaching $24.2 billion
        - iPhone revenue stability despite market saturation concerns
        - Expansion into AI and machine learning capabilities with Apple Intelligence
        - Strong ecosystem lock-in effects with 2+ billion active devices
        - Robust balance sheet with $162 billion in cash and investments
        
        Risk Factors:
        - China market dependency represents approximately 20% of total revenue
        - Regulatory scrutiny in Europe regarding App Store practices
        - Potential iPhone replacement cycle elongation
        - Competition in services from Google, Amazon, and Microsoft
        
        We view Apple as a long-term holding given its durable competitive advantages, exceptional management team,
        and ability to generate substantial free cash flow. The company's transition to a services-focused model
        provides more predictable revenue streams and higher margin opportunities.
        """,
        
        "market_commentary": """
        Market Conditions and AAPL Performance Analysis Q3 2024:
        
        Apple's stock performance during Q3 2024 reflected broader market volatility amid concerns about 
        interest rates and economic growth. The stock traded in a range of $180-$230, ending the quarter 
        at approximately $224 per share.
        
        Institutional investor sentiment remains positive, with 13F filings showing increased positions among 
        top asset managers. Notable developments include:
        
        Technology Sector Dynamics:
        - AI revolution benefiting Apple's hardware and services ecosystem
        - Increased demand for premium smartphones in emerging markets
        - Supply chain optimization reducing component costs
        
        Financial Performance Indicators:
        - Gross margin expansion to 45.2% driven by services mix
        - Operating leverage demonstrated through expense management
        - Return on invested capital exceeding 40% annually
        
        Future Outlook Considerations:
        - Vision Pro adoption and spatial computing market development
        - Healthcare technology integration opportunities
        - Autonomous vehicle technology partnerships and development
        - Sustainable energy initiatives and carbon neutrality goals
        
        The investment community continues to view Apple as a defensive growth stock with characteristics
        of both value and growth investing styles. The company's ability to maintain premium pricing
        while growing market share globally supports our long-term bullish outlook.
        """
    },
    
    "Q2_2024": {
        "filing_date": "2024-08-14",
        "period_end": "2024-06-30", 
        "total_value": 27500000000,
        "aapl_holdings": {
            "shares": 905240000,
            "value": 18900000000,
            "percentage": 68.7
        },
        "document_text": """
        FORM 13F INFORMATION TABLE - Q2 2024
        
        Investment Manager: Berkshire Hathaway Inc.
        Report Period: June 30, 2024
        
        APPLE INC (AAPL) Position Analysis:
        Our Apple holding of 905.24 million shares valued at $18.9 billion continues to represent approximately 
        68.7% of our equity portfolio. We maintained our position during Q2 2024 despite market volatility 
        driven by inflation concerns and Federal Reserve policy uncertainty.
        
        Q2 2024 Performance Assessment:
        Apple reported mixed results for Q2 2024, with revenue of $85.8 billion, a 5% decline year-over-year.
        However, this decline was primarily attributed to difficult year-over-year comparisons and timing of 
        product launches rather than fundamental business deterioration.
        
        Positive Developments:
        - Services revenue reached record $24.2 billion, up 14% year-over-year
        - iPad revenue surged 24% with new Pro models driving premium sales
        - Gross margin improvement to 46.3% exceeded analyst expectations
        - Share repurchase program continued with $110 billion authorization
        
        Management Commentary Highlights:
        CEO Tim Cook emphasized Apple's long-term growth strategy focusing on artificial intelligence integration
        across the product ecosystem. The company's investment in generative AI capabilities positions Apple
        to capture value from the next computing paradigm shift.
        
        Competitive Position Analysis:
        Apple maintains significant competitive advantages through:
        - Vertical integration of hardware, software, and services
        - Premium brand positioning supporting pricing power
        - Developer ecosystem with 2.2 million apps generating recurring revenue
        - Customer loyalty metrics exceeding 90% retention rates
        
        Capital Allocation Framework:
        Management's disciplined approach to capital allocation continues to create shareholder value through
        strategic investments in R&D, selective acquisitions, and consistent return of excess cash to 
        shareholders through dividends and buybacks.
        """,
        
        "analyst_notes": """
        Sell-Side Analyst Coverage Summary - Q2 2024:
        
        Consensus Rating: BUY (18 Buy, 6 Hold, 1 Sell)
        Price Target Range: $190 - $250 (Average: $220)
        
        Key Analyst Themes:
        
        Morgan Stanley (Overweight, $240 PT):
        "Apple's services transformation accelerating with AI integration creating new monetization opportunities.
        We expect Services revenue to reach $100 billion annually by 2026."
        
        Goldman Sachs (Buy, $230 PT):
        "Hardware replacement cycles extending but services attach rates increasing. Net effect positive for
        long-term revenue visibility and margin expansion."
        
        UBS (Neutral, $200 PT):
        "China exposure remains key risk factor. Geopolitical tensions could impact 20% of revenue base.
        Recommend reduced weight until visibility improves."
        
        JPMorgan (Overweight, $245 PT):
        "Apple Intelligence rollout positions company for next growth phase. Integration across device ecosystem
        creates sustainable competitive advantages in AI-driven applications."
        
        Investment Banking Activity:
        - No major M&A transactions announced during quarter
        - Increased focus on strategic partnerships in AI and automotive sectors
        - Supply chain diversification investments continuing in India and Vietnam
        
        The analyst community remains constructive on Apple's long-term prospects while acknowledging
        near-term headwinds from macroeconomic uncertainty and China market dynamics.
        """
    }
}

# Financial insights search queries
FINANCIAL_SEARCH_QUERIES = {
    "revenue_growth": {
        "query": "services revenue growth rate quarterly performance",
        "description": "Search for information about Apple's services revenue growth trends"
    },
    "competitive_advantages": {
        "query": "competitive moat ecosystem lock-in advantages differentiation",
        "description": "Find insights about Apple's competitive positioning and business moat"
    },
    "risk_factors": {
        "query": "china risk regulatory concerns supply chain dependencies",
        "description": "Identify key risk factors and challenges facing Apple"
    },
    "ai_strategy": {
        "query": "artificial intelligence machine learning Apple Intelligence strategy",
        "description": "Discover Apple's AI strategy and technology developments"
    },
    "financial_metrics": {
        "query": "cash flow margins return on capital financial performance",
        "description": "Extract key financial performance indicators and metrics"
    },
    "capital_allocation": {
        "query": "share buybacks dividends capital allocation strategy",
        "description": "Understand management's capital allocation decisions"
    },
    "market_outlook": {
        "query": "future outlook growth opportunities market expansion",
        "description": "Find forward-looking statements and growth prospects"
    }
}

class FinancialAnalysisDemo:
    """Demonstrates ProximaDB for financial document analysis and insight extraction"""
    
    def __init__(self, server_url="http://localhost:5678", grpc_url="localhost:5679"):
        self.server_url = server_url
        self.grpc_url = grpc_url
        self.rest_client = None
        self.grpc_client = None
        self.collection_name = f"financial_docs_{int(time.time())}"
        self.chunks_collection = f"financial_chunks_{int(time.time())}"
        
        # Initialize BERT model for embeddings
        self.model = None
        if FINANCIAL_LIBS_AVAILABLE:
            try:
                self.model = SentenceTransformer('sentence-transformers/all-mpnet-base-v2')
                logger.info("✅ Loaded BERT model for embeddings")
            except Exception as e:
                logger.warning(f"⚠️ Failed to load BERT model: {e}")
                self.model = None
        
        self.processed_documents = []
        self.document_chunks = []
        
    def setup(self):
        """Initialize ProximaDB connections and collections"""
        print("🏦 Setting up Financial Analysis Demo...")
        
        try:
            # Create REST and gRPC clients
            self.rest_client = connect_rest(self.server_url)
            logger.info("✅ Connected to ProximaDB via REST")
            
            try:
                self.grpc_client = connect_grpc(self.grpc_url)
                logger.info("✅ Connected to ProximaDB via gRPC")
            except Exception as e:
                logger.warning(f"⚠️ gRPC connection failed, using REST only: {e}")
                self.grpc_client = self.rest_client
            
            # Create collection for financial documents
            config = CollectionConfig(
                dimension=768,  # BERT embedding dimension
                distance_metric=DistanceMetric.COSINE,
                description="Financial documents and 13F filings analysis",
                index_config={
                    "algorithm": "hnsw",
                    "parameters": {"m": 16, "ef_construction": 200}
                }
            )
            collection = self.rest_client.create_collection(self.collection_name, config)
            logger.info(f"✅ Created financial collection: {collection.name}")
            
            # Create collection for document chunks
            chunks_config = CollectionConfig(
                dimension=768,
                distance_metric=DistanceMetric.COSINE,
                description="Financial document chunks with sliding window embeddings"
            )
            chunks_collection = self.rest_client.create_collection(self.chunks_collection, chunks_config)
            logger.info(f"✅ Created chunks collection: {chunks_collection.name}")
            
            return True
        except Exception as e:
            logger.error(f"❌ Setup failed: {e}")
            return False
    
    def extract_13f_data(self):
        """Extract and process 13F filing data"""
        print("\\n📄 Extracting 13F Filing Data...")
        print("=" * 60)
        
        if FINANCIAL_LIBS_AVAILABLE:
            print("📊 Loading real AAPL financial data...")
            try:
                # Get real AAPL data from yfinance
                aapl = yf.Ticker("AAPL")
                info = aapl.info
                history = aapl.history(period="1y")
                
                print(f"✅ Current AAPL Price: ${info.get('currentPrice', 'N/A')}")
                print(f"✅ Market Cap: ${info.get('marketCap', 'N/A'):,}")
                print(f"✅ 52 Week Range: ${info.get('fiftyTwoWeekLow', 'N/A')} - ${info.get('fiftyTwoWeekHigh', 'N/A')}")
                
            except Exception as e:
                logger.warning(f"⚠️ Failed to fetch real data: {e}")
                print("📝 Using sample 13F filing data for demo...")
        else:
            print("📝 Using sample 13F filing data for demo...")
        
        # Process sample 13F filings
        for period, filing_data in SAMPLE_13F_FILINGS.items():
            print(f"\\n📋 Processing {period} Filing:")
            print(f"   Filing Date: {filing_data['filing_date']}")
            print(f"   AAPL Shares: {filing_data['aapl_holdings']['shares']:,}")
            print(f"   AAPL Value: ${filing_data['aapl_holdings']['value']:,}")
            print(f"   Portfolio %: {filing_data['aapl_holdings']['percentage']}%")
            
            # Create document record
            document = {
                "period": period,
                "filing_date": filing_data['filing_date'],
                "document_type": "13F",
                "company": "AAPL",
                "full_text": filing_data['document_text'],
                "market_commentary": filing_data.get('market_commentary', ''),
                "analyst_notes": filing_data.get('analyst_notes', ''),
                "holdings_data": filing_data['aapl_holdings'],
                "total_portfolio_value": filing_data['total_value']
            }
            
            self.processed_documents.append(document)
        
        print(f"\\n✅ Extracted {len(self.processed_documents)} financial documents")
        return True
    
    def process_sliding_window_chunking(self):
        """Apply sliding window chunking to financial documents"""
        print("\\n🔄 Processing with Sliding Window Chunking...")
        print("=" * 60)
        
        for doc in self.processed_documents:
            print(f"\\n📄 Chunking {doc['period']} filing...")
            
            # Combine all text content for comprehensive analysis
            full_text = f"""
            {doc['full_text']}
            
            Market Commentary:
            {doc['market_commentary']}
            
            Analyst Notes:
            {doc.get('analyst_notes', '')}
            """
            
            # Apply sliding window chunking
            window_size = 300  # tokens per window
            overlap = 100      # overlapping tokens
            
            window_chunks = chunk_sliding_window(
                full_text,
                window_size=window_size,
                overlap=overlap,
                document_id=f"{doc['company']}_{doc['period']}_sliding"
            )
            
            print(f"   ✅ Created {len(window_chunks)} sliding window chunks")
            
            # Also create sentence-based chunks for comparison
            sentence_chunks = chunk_by_sentences(
                full_text,
                chunk_size=250,
                document_id=f"{doc['company']}_{doc['period']}_sentences"
            )
            
            print(f"   ✅ Created {len(sentence_chunks)} sentence-based chunks")
            
            # Store chunks with metadata
            for i, chunk in enumerate(window_chunks[:20]):  # Limit for demo
                chunk_data = {
                    "chunk": chunk,
                    "document_period": doc['period'],
                    "document_type": doc['document_type'],
                    "company": doc['company'],
                    "filing_date": doc['filing_date'],
                    "chunk_type": "sliding_window",
                    "chunk_index": i,
                    "window_size": window_size,
                    "overlap": overlap
                }
                self.document_chunks.append(chunk_data)
            
            # Add some sentence chunks too
            for i, chunk in enumerate(sentence_chunks[:10]):  # Limit for demo
                chunk_data = {
                    "chunk": chunk,
                    "document_period": doc['period'],
                    "document_type": doc['document_type'],
                    "company": doc['company'],
                    "filing_date": doc['filing_date'],
                    "chunk_type": "sentence_based",
                    "chunk_index": i,
                    "avg_sentence_length": len(chunk.text) // max(1, chunk.text.count('.'))
                }
                self.document_chunks.append(chunk_data)
        
        print(f"\\n✅ Total chunks created: {len(self.document_chunks)}")
        return True
    
    def generate_bert_embeddings(self):
        """Generate BERT embeddings for document chunks"""
        print("\\n🧠 Generating BERT Embeddings...")
        print("=" * 60)
        
        vectors = []
        ids = []
        metadata = []
        
        for i, chunk_data in enumerate(self.document_chunks):
            chunk = chunk_data['chunk']
            
            # Generate embeddings
            if self.model:
                # Use real BERT embeddings
                embedding = self.model.encode(chunk.text).astype(np.float32)
                print(f"✅ Generated BERT embedding for chunk {i+1} (dim: {len(embedding)})")
            else:
                # Generate mock embeddings with financial domain bias
                embedding = np.random.randn(768).astype(np.float32)
                
                # Add domain-specific biases based on content
                text_lower = chunk.text.lower()
                if 'revenue' in text_lower or 'growth' in text_lower:
                    embedding[:50] += 0.3  # Revenue/growth bias
                if 'risk' in text_lower or 'concern' in text_lower:
                    embedding[50:100] += 0.3  # Risk bias
                if 'apple' in text_lower or 'aapl' in text_lower:
                    embedding[100:150] += 0.3  # Apple-specific bias
                if 'ai' in text_lower or 'intelligence' in text_lower:
                    embedding[150:200] += 0.3  # AI bias
                
                embedding = embedding / np.linalg.norm(embedding)
                print(f"✅ Generated mock embedding for chunk {i+1} (with financial bias)")
            
            vectors.append(embedding.tolist())
            ids.append(f"chunk_{i:03d}")
            
            # Prepare metadata
            meta = {
                "text": chunk.text[:500],  # Truncate for storage
                "period": chunk_data['document_period'],
                "company": chunk_data['company'],
                "document_type": chunk_data['document_type'],
                "filing_date": chunk_data['filing_date'],
                "chunk_type": chunk_data['chunk_type'],
                "chunk_index": chunk_data['chunk_index'],
                "text_length": len(chunk.text),
                "start_pos": chunk.start_pos,
                "end_pos": chunk.end_pos
            }
            
            # Add chunk-type specific metadata
            if chunk_data['chunk_type'] == 'sliding_window':
                meta.update({
                    "window_size": chunk_data['window_size'],
                    "overlap": chunk_data['overlap']
                })
            elif chunk_data['chunk_type'] == 'sentence_based':
                meta.update({
                    "avg_sentence_length": chunk_data['avg_sentence_length']
                })
            
            metadata.append(meta)
        
        # Store embeddings in ProximaDB
        try:
            start_time = time.time()
            result = self.grpc_client.insert_vectors(
                self.chunks_collection,
                vectors,
                ids,
                metadata=metadata
            )
            duration = time.time() - start_time
            
            logger.info(f"✅ Stored {result.successful_count} financial chunk embeddings")
            logger.info(f"⚡ Embedding storage throughput: {result.successful_count/duration:.0f} chunks/second")
            
            return True
        except Exception as e:
            logger.error(f"❌ Failed to store embeddings: {e}")
            return False
    
    def search_financial_insights(self):
        """Search for key financial insights using semantic search"""
        print("\\n🔍 Searching for Financial Insights...")
        print("=" * 60)
        
        for query_name, query_data in FINANCIAL_SEARCH_QUERIES.items():
            print(f"\\n💡 {query_data['description']}")
            print(f"   Query: \"{query_data['query']}\"")
            print("-" * 50)
            
            # Generate query embedding
            if self.model:
                query_embedding = self.model.encode(query_data['query']).astype(np.float32)
            else:
                # Generate biased query vector
                query_embedding = np.random.randn(768).astype(np.float32)
                
                # Add query-specific biases
                query_lower = query_data['query'].lower()
                if 'revenue' in query_lower or 'growth' in query_lower:
                    query_embedding[:50] += 0.5
                if 'risk' in query_lower:
                    query_embedding[50:100] += 0.5
                if 'ai' in query_lower or 'intelligence' in query_lower:
                    query_embedding[150:200] += 0.5
                if 'competitive' in query_lower or 'moat' in query_lower:
                    query_embedding[200:250] += 0.5
                    
                query_embedding = query_embedding / np.linalg.norm(query_embedding)
            
            try:
                # Perform semantic search
                results = self.rest_client.search(
                    self.chunks_collection,
                    query_embedding.tolist(),
                    k=5
                )
                
                print(f"   📊 Found {len(results)} relevant insights:")
                
                for i, result in enumerate(results):
                    metadata = getattr(result, 'metadata', {})
                    
                    print(f"\\n   {i+1}. Score: {result.score:.3f}")
                    print(f"      Period: {metadata.get('period', 'N/A')}")
                    print(f"      Type: {metadata.get('chunk_type', 'N/A')}")
                    print(f"      Filing: {metadata.get('filing_date', 'N/A')}")
                    print(f"      Text: {metadata.get('text', 'N/A')[:200]}...")
                
                # Analyze results by period
                period_distribution = {}
                for result in results:
                    metadata = getattr(result, 'metadata', {})
                    period = metadata.get('period', 'Unknown')
                    period_distribution[period] = period_distribution.get(period, 0) + 1
                
                print(f"\\n   📈 Results by period: {dict(period_distribution)}")
                
            except Exception as e:
                logger.error(f"❌ Search failed for {query_name}: {e}")
    
    def analyze_financial_trends(self):
        """Analyze trends across different filing periods"""
        print("\\n📈 Analyzing Financial Trends...")
        print("=" * 60)
        
        # Trend analysis queries
        trend_queries = [
            "services revenue quarterly growth trends",
            "portfolio allocation changes over time",
            "risk factors evolution quarterly filings",
            "cash flow generation trends analysis"
        ]
        
        for query in trend_queries:
            print(f"\\n🔍 Trend Analysis: {query}")
            print("-" * 40)
            
            if self.model:
                query_embedding = self.model.encode(query).astype(np.float32)
            else:
                query_embedding = np.random.randn(768).astype(np.float32)
                query_embedding = query_embedding / np.linalg.norm(query_embedding)
            
            try:
                results = self.rest_client.search(
                    self.chunks_collection,
                    query_embedding.tolist(),
                    k=10
                )
                
                # Group results by period for trend analysis
                period_results = {}
                for result in results:
                    metadata = getattr(result, 'metadata', {})
                    period = metadata.get('period', 'Unknown')
                    
                    if period not in period_results:
                        period_results[period] = []
                    period_results[period].append({
                        'score': result.score,
                        'text': metadata.get('text', '')[:150],
                        'chunk_type': metadata.get('chunk_type', 'N/A')
                    })
                
                print(f"   📊 Trend insights across {len(period_results)} periods:")
                
                for period in sorted(period_results.keys()):
                    results_for_period = period_results[period]
                    avg_score = np.mean([r['score'] for r in results_for_period])
                    
                    print(f"\\n   📅 {period}:")
                    print(f"      Relevance Score: {avg_score:.3f}")
                    print(f"      Top Insight: {results_for_period[0]['text']}...")
                
            except Exception as e:
                logger.error(f"❌ Trend analysis failed: {e}")
    
    def demonstrate_chunking_strategies(self):
        """Compare different chunking strategies for financial documents"""
        print("\\n🔄 Comparing Chunking Strategies...")
        print("=" * 60)
        
        # Analyze chunks by strategy
        strategy_stats = {}
        
        try:
            # Get sample of all chunks
            query_vector = np.random.randn(768).astype(np.float32)
            query_vector = query_vector / np.linalg.norm(query_vector)
            
            results = self.rest_client.search(
                self.chunks_collection,
                query_vector.tolist(),
                k=50
            )
            
            for result in results:
                metadata = getattr(result, 'metadata', {})
                chunk_type = metadata.get('chunk_type', 'unknown')
                
                if chunk_type not in strategy_stats:
                    strategy_stats[chunk_type] = {
                        'count': 0,
                        'avg_length': 0,
                        'total_length': 0,
                        'periods': set()
                    }
                
                strategy_stats[chunk_type]['count'] += 1
                strategy_stats[chunk_type]['total_length'] += metadata.get('text_length', 0)
                strategy_stats[chunk_type]['periods'].add(metadata.get('period', 'Unknown'))
            
            print("📊 Chunking Strategy Performance Analysis:")
            print(f"{'Strategy':<20} {'Count':<8} {'Avg Length':<12} {'Periods':<15}")
            print("-" * 60)
            
            for strategy, stats in strategy_stats.items():
                if stats['count'] > 0:
                    avg_length = stats['total_length'] / stats['count']
                    periods_covered = len(stats['periods'])
                    print(f"{strategy:<20} {stats['count']:<8} {avg_length:<12.0f} {periods_covered:<15}")
            
            print("\\n💡 Strategy Recommendations:")
            print("   • Sliding Window: Best for continuous context preservation")
            print("   • Sentence-based: Optimal for semantic coherence")
            print("   • Both strategies provide complementary insights")
            
        except Exception as e:
            logger.error(f"❌ Chunking analysis failed: {e}")
    
    def generate_financial_report(self):
        """Generate comprehensive financial analysis report"""
        print("\\n📋 Generating Financial Analysis Report...")
        print("=" * 60)
        
        report = {
            "analysis_date": datetime.now().isoformat(),
            "company": "AAPL",
            "periods_analyzed": len(SAMPLE_13F_FILINGS),
            "total_chunks": len(self.document_chunks),
            "embedding_model": "sentence-transformers/all-mpnet-base-v2" if self.model else "mock_embeddings",
            "key_insights": [],
            "risk_factors": [],
            "growth_opportunities": []
        }
        
        # Perform targeted searches for report sections
        report_queries = {
            "key_insights": "competitive advantages market position strengths",
            "risk_factors": "risks concerns challenges regulatory",
            "growth_opportunities": "growth opportunities future outlook expansion"
        }
        
        for section, query in report_queries.items():
            try:
                if self.model:
                    query_embedding = self.model.encode(query).astype(np.float32)
                else:
                    query_embedding = np.random.randn(768).astype(np.float32)
                    query_embedding = query_embedding / np.linalg.norm(query_embedding)
                
                results = self.rest_client.search(
                    self.chunks_collection,
                    query_embedding.tolist(),
                    k=3
                )
                
                section_insights = []
                for result in results:
                    metadata = getattr(result, 'metadata', {})
                    section_insights.append({
                        "text": metadata.get('text', '')[:200],
                        "relevance_score": result.score,
                        "period": metadata.get('period', 'N/A'),
                        "source": metadata.get('chunk_type', 'N/A')
                    })
                
                report[section] = section_insights
                
            except Exception as e:
                logger.error(f"❌ Failed to generate {section}: {e}")
        
        # Save report
        report_path = "/tmp/financial_analysis_report.json"
        with open(report_path, 'w') as f:
            json.dump(report, f, indent=2)
        
        print(f"✅ Financial analysis report saved to {report_path}")
        
        # Print summary
        print("\\n📊 Analysis Summary:")
        print(f"   • Analyzed {report['periods_analyzed']} quarterly filings")
        print(f"   • Processed {report['total_chunks']} document chunks")
        print(f"   • Generated semantic embeddings with {report['embedding_model']}")
        print(f"   • Identified {len(report['key_insights'])} key insights")
        print(f"   • Catalogued {len(report['risk_factors'])} risk factors")
        print(f"   • Found {len(report['growth_opportunities'])} growth opportunities")
        
        return report
    
    def cleanup(self):
        """Clean up demo resources"""
        print("\\n🧹 Cleaning up...")
        
        try:
            self.rest_client.delete_collection(self.collection_name)
            self.rest_client.delete_collection(self.chunks_collection)
            logger.info("✅ Deleted demo collections")
        except Exception as e:
            logger.warning(f"⚠️ Cleanup failed: {e}")
    
    def run_full_demo(self):
        """Run the complete financial analysis demonstration"""
        print("🏦 ProximaDB Financial Document Analysis Demo")
        print("=" * 60)
        print("This demo showcases:")
        print("• 13F filing data extraction and processing")
        print("• Sliding window chunking with BERT embeddings")
        print("• Semantic search for financial insights")
        print("• Trend analysis across filing periods")
        print("• Financial intelligence report generation")
        print("=" * 60)
        
        if not self.setup():
            return False
        
        try:
            # Run all demo components
            self.extract_13f_data()
            self.process_sliding_window_chunking()
            self.generate_bert_embeddings()
            self.search_financial_insights()
            self.analyze_financial_trends()
            self.demonstrate_chunking_strategies()
            self.generate_financial_report()
            
            print("\\n✅ Financial analysis demonstration completed successfully!")
            print("\\n💡 Key Takeaways:")
            print("• ProximaDB excels at financial document intelligence")
            print("• Sliding window chunking preserves context for complex analysis")
            print("• BERT embeddings enable sophisticated semantic search")
            print("• Multi-period trend analysis reveals investment insights")
            print("• Real-time search capabilities support dynamic research workflows")
            
            return True
            
        except Exception as e:
            logger.error(f"❌ Demo failed: {e}")
            return False
        finally:
            self.cleanup()


def main():
    """Main entry point"""
    print("🚀 Starting ProximaDB Financial Analysis Demo...")
    
    if not FINANCIAL_LIBS_AVAILABLE:
        print("\\n📦 Optional Dependencies Note:")
        print("For enhanced functionality, install: pip install pandas yfinance sentence-transformers")
        print("Demo will continue with mock data and embeddings.\\n")
    
    demo = FinancialAnalysisDemo()
    success = demo.run_full_demo()
    
    print(f"\\n{'='*60}")
    if success:
        print("🎊 Financial analysis demo completed successfully!")
        print("✨ ProximaDB demonstrated powerful financial intelligence capabilities!")
    else:
        print("😞 Demo encountered issues")
    
    return success


if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)