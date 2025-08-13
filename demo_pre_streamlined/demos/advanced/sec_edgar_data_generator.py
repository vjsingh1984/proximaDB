#!/usr/bin/env python3
"""
SEC EDGAR Data Generator - Creates realistic SEC filing documents for demos
Generates ~30MB of SEC filing data across multiple companies for testing
This script runs during Docker container startup to prepare demo data
"""

import asyncio
import aiohttp
from datetime import datetime
import json
import random
from typing import List, Dict, Any

# Realistic SEC filing section templates with substantial content
FILING_TEMPLATES = {
    "business_overview": """
# BUSINESS OVERVIEW

## Company Description
{company_name} is a leading {industry} company that operates globally with a focus on {focus_areas}. Founded in {founded_year}, we have grown to become one of the most recognized brands in our industry. Our mission is to {mission_statement}.

## Products and Services
Our product portfolio includes {product_list}. These products serve millions of customers worldwide and generate significant revenue streams. In fiscal year {fiscal_year}, our top-performing products were:
{detailed_product_analysis}

## Market Position
We maintain a {market_position} position in the {industry} industry with approximately {market_share}% market share. Our competitive advantages include:
- {competitive_advantage_1}
- {competitive_advantage_2}
- {competitive_advantage_3}

## Geographic Presence
We operate in {num_countries} countries across {num_continents} continents. Our largest markets by revenue are:
1. {market_1}: ${revenue_1}B ({percent_1}% of total revenue)
2. {market_2}: ${revenue_2}B ({percent_2}% of total revenue)
3. {market_3}: ${revenue_3}B ({percent_3}% of total revenue)

## Research and Development
R&D is critical to our long-term success. In {fiscal_year}, we invested ${rd_investment}B in R&D, representing {rd_percent}% of our total revenue. Our R&D efforts focus on:
{rd_focus_areas}

## Supply Chain and Manufacturing
{supply_chain_description}

## Human Capital
As of {date}, we employ approximately {num_employees} full-time employees worldwide. Our workforce is distributed as follows:
- Engineering and Technical: {eng_percent}%
- Sales and Marketing: {sales_percent}%
- Operations: {ops_percent}%
- General and Administrative: {admin_percent}%

{additional_business_content}
""",
    
    "risk_factors": """
# RISK FACTORS

Investing in our securities involves a high degree of risk. You should carefully consider the risks and uncertainties described below, together with all of the other information in this report.

## Cybersecurity and Data Protection Risks

### Data Breach and Security Incidents
We face significant risks related to cybersecurity threats and data breaches. Our systems process and store vast amounts of sensitive data, including:
- Customer personal information ({customer_records} million records)
- Financial transaction data (${transaction_volume}B annually)
- Proprietary technology and trade secrets
- Employee information ({employee_records} records)

Recent high-profile breaches in our industry have resulted in:
- Average costs of ${breach_cost}M per incident
- Regulatory fines up to ${max_fine}M
- Loss of customer trust and market share
- Class action lawsuits with settlements exceeding ${settlement_amount}M

### Evolving Threat Landscape
The cybersecurity threat landscape continues to evolve rapidly. We face threats from:
- Nation-state actors targeting {target_systems}
- Organized cybercrime groups using ransomware
- Supply chain attacks affecting {num_suppliers} key suppliers
- Zero-day exploits in {num_products} of our products
- AI-powered attacks that can bypass traditional defenses

### Regulatory Compliance Risks
We are subject to numerous data protection regulations including:
- GDPR (potential fines up to 4% of global revenue)
- CCPA (fines up to $7,500 per violation)
- {num_countries} different national privacy laws
- Industry-specific regulations ({industry_regs})

Non-compliance could result in:
- Regulatory fines totaling ${potential_fines}M
- Suspension of operations in key markets
- Criminal liability for executives
- Reputational damage affecting {percent_customers}% of our customer base

### Third-Party Risks
We rely on {num_vendors} third-party vendors and partners who have access to our systems. Risks include:
- Vendor breaches affecting our data
- Supply chain compromises
- Inadequate security practices by partners
- Lack of visibility into vendor security posture

### Technology Infrastructure Risks
Our technology infrastructure faces risks including:
- Legacy systems with known vulnerabilities
- Complex integration points creating attack surfaces
- Cloud service dependencies on {num_providers} providers
- IoT device vulnerabilities in {num_devices} connected devices

{additional_risk_content}
""",
    
    "financial_performance": """
# FINANCIAL PERFORMANCE AND ANALYSIS

## Revenue Analysis
For the fiscal year ended {fiscal_year_end}, we reported total revenue of ${total_revenue}B, representing a {revenue_growth}% increase compared to the prior year.

### Revenue by Segment
{segment_analysis}

### Geographic Revenue Distribution
{geographic_revenue}

## Operating Expenses
Total operating expenses for {fiscal_year} were ${operating_expenses}B, broken down as follows:
- Cost of Revenue: ${cost_of_revenue}B ({cor_percent}%)
- Research and Development: ${rd_expense}B ({rd_percent}%)
- Sales and Marketing: ${sales_expense}B ({sales_percent}%)
- General and Administrative: ${ga_expense}B ({ga_percent}%)

### Year-over-Year Analysis
{yoy_analysis}

## Profitability Metrics
- Gross Margin: {gross_margin}% (vs {prior_gross_margin}% prior year)
- Operating Margin: {operating_margin}% (vs {prior_operating_margin}% prior year)
- Net Margin: {net_margin}% (vs {prior_net_margin}% prior year)
- EBITDA: ${ebitda}B
- Free Cash Flow: ${fcf}B

## Balance Sheet Highlights
As of {balance_sheet_date}:
- Total Assets: ${total_assets}B
- Total Liabilities: ${total_liabilities}B
- Shareholders' Equity: ${shareholders_equity}B
- Cash and Equivalents: ${cash}B
- Long-term Debt: ${debt}B

## Cash Flow Analysis
{cash_flow_analysis}

## Capital Allocation
{capital_allocation}

## Financial Outlook
{financial_outlook}

{additional_financial_content}
""",
    
    "legal_proceedings": """
# LEGAL PROCEEDINGS

We are involved in various legal proceedings, claims, and litigation arising in the ordinary course of business.

## Major Litigation

### Patent Infringement Cases
We are currently defending against {num_patent_cases} patent infringement lawsuits:
{patent_case_details}

### Antitrust Investigations
We are subject to antitrust investigations in {num_jurisdictions} jurisdictions:
{antitrust_details}

### Consumer Class Actions
We face {num_class_actions} consumer class action lawsuits:
{class_action_details}

### Securities Litigation
{securities_litigation}

### Employment Matters
{employment_litigation}

### Environmental Proceedings
{environmental_proceedings}

### Government Investigations
{government_investigations}

## Contingent Liabilities
We have accrued ${litigation_reserve}M for probable losses related to legal proceedings. However, the ultimate outcome of these matters could result in losses substantially in excess of our reserves.

{additional_legal_content}
"""
}

def generate_large_content(template: str, params: Dict[str, Any], target_size_kb: int) -> str:
    """Generate large content by expanding template with parameters and padding"""
    # Fill in the template
    content = template.format(**params)
    
    # Calculate current size
    current_size_kb = len(content.encode('utf-8')) / 1024
    
    # Add padding content to reach target size
    if current_size_kb < target_size_kb:
        padding_needed_kb = target_size_kb - current_size_kb
        
        # Generate relevant padding content
        padding_sections = []
        
        # Add detailed subsections
        for i in range(int(padding_needed_kb / 10)):  # Each subsection ~10KB
            subsection = f"""

## Additional Detail Section {i+1}

This section provides supplementary information and analysis related to the topics discussed above. 
It includes detailed breakdowns, historical context, and forward-looking statements that help 
investors understand the full scope of our operations and risks.

### Detailed Analysis {i+1}.1
{generate_analysis_paragraph()}

### Historical Context {i+1}.2
{generate_historical_paragraph()}

### Market Dynamics {i+1}.3
{generate_market_paragraph()}

### Strategic Initiatives {i+1}.4
{generate_strategy_paragraph()}

### Performance Metrics {i+1}.5
{generate_metrics_paragraph()}
"""
            padding_sections.append(subsection)
        
        content += "\n".join(padding_sections)
    
    return content

def generate_analysis_paragraph() -> str:
    """Generate a realistic analysis paragraph"""
    templates = [
        "Our analysis indicates that market conditions have shifted significantly due to technological disruption, "
        "regulatory changes, and evolving consumer preferences. We have adapted our strategy by investing in "
        "digital transformation initiatives, expanding our product portfolio, and entering new geographic markets. "
        "These efforts have resulted in improved operational efficiency and market position.",
        
        "The competitive landscape continues to evolve with new entrants leveraging advanced technologies. "
        "We maintain our competitive edge through continuous innovation, strategic partnerships, and operational "
        "excellence. Our investments in artificial intelligence and machine learning have enhanced our products "
        "and improved customer satisfaction scores by significant margins.",
        
        "Financial performance analysis reveals strong underlying fundamentals despite macroeconomic headwinds. "
        "Revenue growth has been driven by successful product launches, market share gains, and pricing optimization. "
        "Cost management initiatives have improved margins while maintaining investment in growth areas.",
    ]
    return random.choice(templates) + " " + generate_filler_text(500)

def generate_historical_paragraph() -> str:
    """Generate a historical context paragraph"""
    templates = [
        "Over the past decade, our company has undergone significant transformation. From our origins as a "
        "traditional manufacturer, we have evolved into a technology-driven enterprise. Key milestones include "
        "the launch of our digital platform, expansion into emerging markets, and strategic acquisitions that "
        "have strengthened our market position.",
        
        "Historical performance demonstrates our resilience through economic cycles. During the 2008 financial "
        "crisis, we maintained profitability through disciplined cost management. The COVID-19 pandemic "
        "accelerated our digital transformation, resulting in new revenue streams and improved operational efficiency.",
    ]
    return random.choice(templates) + " " + generate_filler_text(500)

def generate_market_paragraph() -> str:
    """Generate market dynamics paragraph"""
    return (
        "Market dynamics are influenced by technological advancement, regulatory evolution, and changing consumer "
        "behavior. Industry consolidation continues as companies seek scale advantages. Digital disruption creates "
        "both opportunities and challenges. We monitor these trends closely and adjust our strategy accordingly. "
        + generate_filler_text(500)
    )

def generate_strategy_paragraph() -> str:
    """Generate strategy paragraph"""
    return (
        "Our strategic initiatives focus on sustainable growth through innovation, operational excellence, and "
        "market expansion. Key priorities include digital transformation, sustainability initiatives, and talent "
        "development. We allocate resources based on expected returns and strategic importance. "
        + generate_filler_text(500)
    )

def generate_metrics_paragraph() -> str:
    """Generate metrics paragraph"""
    return (
        "Performance metrics demonstrate progress against strategic objectives. Key performance indicators include "
        "revenue growth, margin expansion, market share, customer satisfaction, and employee engagement. We use "
        "balanced scorecards to track performance across financial and non-financial dimensions. "
        + generate_filler_text(500)
    )

def generate_filler_text(words: int) -> str:
    """Generate realistic filler text"""
    topics = [
        "operational efficiency", "market opportunities", "strategic partnerships", "technological innovation",
        "customer satisfaction", "regulatory compliance", "risk management", "financial performance",
        "competitive positioning", "growth initiatives", "cost optimization", "digital transformation",
        "sustainability efforts", "talent acquisition", "product development", "market expansion"
    ]
    
    sentences = []
    word_count = 0
    
    while word_count < words:
        topic1, topic2 = random.sample(topics, 2)
        sentence = f"Our focus on {topic1} continues to drive improvements in {topic2}. "
        sentences.append(sentence)
        word_count += len(sentence.split())
    
    return " ".join(sentences)

def generate_filing_params(company: str, ticker: str) -> Dict[str, Any]:
    """Generate realistic parameters for filing templates"""
    base_params = {
        "company_name": company,
        "ticker": ticker,
        "fiscal_year": 2023,
        "fiscal_year_end": "September 30, 2023",
        "date": datetime.now().strftime("%B %d, %Y"),
        "balance_sheet_date": "September 30, 2023",
    }
    
    # Company-specific parameters
    if ticker == "AAPL":
        base_params.update({
            "industry": "technology and consumer electronics",
            "focus_areas": "innovative hardware, software, and services",
            "founded_year": 1976,
            "mission_statement": "bring the best user experience to customers through innovative hardware, software, and services",
            "product_list": "iPhone, Mac, iPad, Apple Watch, Apple TV, AirPods, HomePod, and various software and services",
            "market_position": "leading",
            "market_share": 15.8,
            "num_countries": 175,
            "num_continents": 6,
            "market_1": "Americas", "revenue_1": 169.7, "percent_1": 42,
            "market_2": "Europe", "revenue_2": 98.3, "percent_2": 25,
            "market_3": "Greater China", "revenue_3": 72.6, "percent_3": 18,
            "num_employees": 164000,
            "total_revenue": 383.3,
            "revenue_growth": 7.8,
            "customer_records": 1850,
            "transaction_volume": 1200,
            "num_suppliers": 200,
            "num_vendors": 500,
        })
    elif ticker == "MSFT":
        base_params.update({
            "industry": "technology and cloud computing",
            "focus_areas": "cloud computing, productivity software, and artificial intelligence",
            "founded_year": 1975,
            "mission_statement": "empower every person and every organization on the planet to achieve more",
            "product_list": "Azure, Office 365, Windows, Surface devices, Xbox, LinkedIn, and Dynamics 365",
            "market_position": "dominant",
            "market_share": 21.5,
            "num_countries": 190,
            "num_continents": 6,
            "market_1": "United States", "revenue_1": 106.5, "percent_1": 51,
            "market_2": "Europe", "revenue_2": 65.4, "percent_2": 31,
            "market_3": "Asia Pacific", "revenue_3": 37.6, "percent_3": 18,
            "num_employees": 221000,
            "total_revenue": 211.9,
            "revenue_growth": 11.5,
            "customer_records": 1200,
            "transaction_volume": 890,
            "num_suppliers": 150,
            "num_vendors": 400,
        })
    elif ticker == "GOOGL":
        base_params.update({
            "industry": "technology and digital advertising",
            "focus_areas": "search, advertising, cloud computing, and artificial intelligence",
            "founded_year": 1998,
            "mission_statement": "organize the world's information and make it universally accessible and useful",
            "product_list": "Google Search, YouTube, Google Cloud, Android, Chrome, Google Workspace, and Google Play",
            "market_position": "dominant",
            "market_share": 28.6,
            "num_countries": 200,
            "num_continents": 6,
            "market_1": "United States", "revenue_1": 146.8, "percent_1": 48,
            "market_2": "EMEA", "revenue_2": 90.3, "percent_2": 30,
            "market_3": "APAC", "revenue_3": 66.9, "percent_3": 22,
            "num_employees": 182000,
            "total_revenue": 307.4,
            "revenue_growth": 9.8,
            "customer_records": 2400,
            "transaction_volume": 1500,
            "num_suppliers": 300,
            "num_vendors": 600,
        })
    elif ticker == "AMZN":
        base_params.update({
            "industry": "e-commerce and cloud computing",
            "focus_areas": "e-commerce, cloud infrastructure, digital streaming, and artificial intelligence",
            "founded_year": 1994,
            "mission_statement": "be Earth's most customer-centric company",
            "product_list": "Amazon.com, AWS, Prime Video, Alexa, Kindle, Whole Foods, and Amazon Logistics",
            "market_position": "leading",
            "market_share": 38.7,
            "num_countries": 185,
            "num_continents": 6,
            "market_1": "North America", "revenue_1": 372.4, "percent_1": 69,
            "market_2": "International", "revenue_2": 118.0, "percent_2": 22,
            "market_3": "AWS", "revenue_3": 48.3, "percent_3": 9,
            "num_employees": 1525000,
            "total_revenue": 538.7,
            "revenue_growth": 12.3,
            "customer_records": 3100,
            "transaction_volume": 2800,
            "num_suppliers": 400,
            "num_vendors": 800,
        })
    elif ticker == "META":
        base_params.update({
            "industry": "social media and virtual reality",
            "focus_areas": "social networking, messaging, virtual reality, and the metaverse",
            "founded_year": 2004,
            "mission_statement": "give people the power to build community and bring the world closer together",
            "product_list": "Facebook, Instagram, WhatsApp, Messenger, Meta Quest, and Workplace",
            "market_position": "leading",
            "market_share": 22.3,
            "num_countries": 190,
            "num_continents": 6,
            "market_1": "US & Canada", "revenue_1": 60.0, "percent_1": 45,
            "market_2": "Europe", "revenue_2": 35.8, "percent_2": 27,
            "market_3": "Asia-Pacific", "revenue_3": 37.5, "percent_3": 28,
            "num_employees": 86000,
            "total_revenue": 134.9,
            "revenue_growth": 16.2,
            "customer_records": 3700,
            "transaction_volume": 680,
            "num_suppliers": 250,
            "num_vendors": 450,
        })
    elif ticker == "TSLA":
        base_params.update({
            "industry": "electric vehicles and clean energy",
            "focus_areas": "electric vehicles, energy storage, and solar panels",
            "founded_year": 2003,
            "mission_statement": "accelerate the world's transition to sustainable energy",
            "product_list": "Model S, Model 3, Model X, Model Y, Cybertruck, Semi, Powerwall, and Solar Roof",
            "market_position": "pioneering",
            "market_share": 19.3,
            "num_countries": 40,
            "num_continents": 5,
            "market_1": "United States", "revenue_1": 47.9, "percent_1": 50,
            "market_2": "China", "revenue_2": 22.5, "percent_2": 23,
            "market_3": "Europe", "revenue_3": 25.8, "percent_3": 27,
            "num_employees": 127000,
            "total_revenue": 96.8,
            "revenue_growth": 23.5,
            "customer_records": 580,
            "transaction_volume": 320,
            "num_suppliers": 350,
            "num_vendors": 500,
        })
    else:  # NVDA
        base_params.update({
            "industry": "semiconductors and artificial intelligence computing",
            "focus_areas": "GPUs, AI computing, data center acceleration, and autonomous vehicles",
            "founded_year": 1993,
            "mission_statement": "pioneering accelerated computing to help solve the world's most challenging problems",
            "product_list": "GeForce GPUs, Quadro GPUs, Tesla GPUs, DGX systems, CUDA, and DRIVE platform",
            "market_position": "dominant",
            "market_share": 82.4,
            "num_countries": 120,
            "num_continents": 6,
            "market_1": "United States", "revenue_1": 15.4, "percent_1": 44,
            "market_2": "China/Taiwan", "revenue_2": 9.5, "percent_2": 27,
            "market_3": "Other Asia", "revenue_3": 10.2, "percent_3": 29,
            "num_employees": 26000,
            "total_revenue": 35.1,
            "revenue_growth": 125.8,
            "customer_records": 420,
            "transaction_volume": 280,
            "num_suppliers": 180,
            "num_vendors": 320,
        })
    
    # Add common financial parameters
    base_params.update({
        "operating_expenses": round(base_params["total_revenue"] * 0.75, 1),
        "cost_of_revenue": round(base_params["total_revenue"] * 0.45, 1),
        "rd_expense": round(base_params["total_revenue"] * 0.15, 1),
        "sales_expense": round(base_params["total_revenue"] * 0.10, 1),
        "ga_expense": round(base_params["total_revenue"] * 0.05, 1),
        "rd_investment": round(base_params["total_revenue"] * 0.15, 1),
        "rd_percent": 15,
        "cor_percent": 45,
        "sales_percent": 10,
        "ga_percent": 5,
        "gross_margin": 55,
        "prior_gross_margin": 53,
        "operating_margin": 25,
        "prior_operating_margin": 23,
        "net_margin": 20,
        "prior_net_margin": 19,
        "ebitda": round(base_params["total_revenue"] * 0.30, 1),
        "fcf": round(base_params["total_revenue"] * 0.25, 1),
        "total_assets": round(base_params["total_revenue"] * 1.5, 1),
        "total_liabilities": round(base_params["total_revenue"] * 0.6, 1),
        "shareholders_equity": round(base_params["total_revenue"] * 0.9, 1),
        "cash": round(base_params["total_revenue"] * 0.3, 1),
        "debt": round(base_params["total_revenue"] * 0.2, 1),
        "breach_cost": random.randint(50, 150),
        "max_fine": random.randint(100, 500),
        "settlement_amount": random.randint(200, 800),
        "potential_fines": random.randint(500, 2000),
        "num_products": random.randint(5, 20),
        "percent_customers": random.randint(20, 40),
        "num_devices": random.randint(10000, 100000),
        "num_providers": random.randint(3, 8),
        "eng_percent": random.randint(40, 60),
        "ops_percent": random.randint(20, 30),
        "admin_percent": random.randint(5, 15),
        "employee_records": base_params["num_employees"],
        "target_systems": "intellectual property and customer data",
        "industry_regs": "SOX, PCI-DSS, HIPAA where applicable",
        "litigation_reserve": random.randint(100, 500),
        "num_patent_cases": random.randint(5, 20),
        "num_jurisdictions": random.randint(3, 10),
        "num_class_actions": random.randint(2, 8),
    })
    
    # Add detailed content placeholders
    base_params.update({
        "detailed_product_analysis": generate_filler_text(200),
        "competitive_advantage_1": "Strong brand recognition and customer loyalty",
        "competitive_advantage_2": "Extensive patent portfolio and technological innovation",
        "competitive_advantage_3": "Efficient supply chain and operational excellence",
        "rd_focus_areas": generate_filler_text(150),
        "supply_chain_description": generate_filler_text(200),
        "additional_business_content": generate_filler_text(500),
        "additional_risk_content": generate_filler_text(500),
        "segment_analysis": generate_filler_text(300),
        "geographic_revenue": generate_filler_text(200),
        "yoy_analysis": generate_filler_text(250),
        "cash_flow_analysis": generate_filler_text(200),
        "capital_allocation": generate_filler_text(200),
        "financial_outlook": generate_filler_text(300),
        "additional_financial_content": generate_filler_text(500),
        "patent_case_details": generate_filler_text(300),
        "antitrust_details": generate_filler_text(250),
        "class_action_details": generate_filler_text(250),
        "securities_litigation": generate_filler_text(200),
        "employment_litigation": generate_filler_text(200),
        "environmental_proceedings": generate_filler_text(200),
        "government_investigations": generate_filler_text(250),
        "additional_legal_content": generate_filler_text(400),
    })
    
    return base_params

async def generate_sec_edgar_data():
    """Generate SEC EDGAR demo data"""
    print("🏛️ SEC EDGAR Data Generator")
    print("=" * 80)
    print("📊 Generating realistic SEC filing documents for demo...")
    print()
    
    # S&P 50 top companies for faster demo generation
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
        
        # Top Industrial & Energy (5 companies)
        ("Exxon Mobil Corporation", "XOM"),
        ("Chevron Corporation", "CVX"),
        ("Boeing Company", "BA"),
        ("Caterpillar Inc.", "CAT"),
        ("United Parcel Service", "UPS"),
    ]
    
    # Document size targets (in KB) for each section
    # 32KB per section for fast generation with meaningful chunks
    section_sizes = {
        "business_overview": 32,        # 32KB
        "risk_factors": 32,             # 32KB
        "financial_performance": 32,    # 32KB
        "legal_proceedings": 32,        # 32KB
    }
    
    base_url = "http://localhost:8080"
    total_size = 0
    documents_created = []
    
    async with aiohttp.ClientSession() as session:
        # Create collection
        print("📁 Creating large SEC filings collection...")
        collection_data = {
            "operation": "create",
            "config": {
                "name": "sec_edgar_large_filings",
                "dimension": 768,
                "distance_metric": "COSINE",
                "storage_engine": "VIPER"
            }
        }
        
        async with session.post(f"{base_url}/api/v1/collection", json=collection_data) as resp:
            if resp.status != 200:
                text = await resp.text()
                print(f"❌ Failed to create collection: {text}")
                return
        
        print("✅ Collection created successfully")
        print()
        
        # Generate and ingest documents
        print("📥 Generating and ingesting large SEC filings...")
        print()
        
        for company_name, ticker in companies:
            print(f"🏢 Processing {company_name} ({ticker})...")
            params = generate_filing_params(company_name, ticker)
            
            for section_name, template in FILING_TEMPLATES.items():
                target_size_kb = section_sizes.get(section_name, 1000)
                
                # Generate large content
                content = generate_large_content(template, params, target_size_kb)
                content_size_mb = len(content.encode('utf-8')) / (1024 * 1024)
                total_size += content_size_mb
                
                print(f"  📄 {section_name}: {content_size_mb:.1f}MB")
                
                # Prepare document for ingestion
                document = {
                    "text": content,
                    "collection_id": "sec_edgar_large_filings",
                    "document_id": f"{ticker}_{section_name}",
                    "metadata": {
                        "company": company_name,
                        "ticker": ticker,
                        "filing_type": "10-K" if section_name != "legal_proceedings" else "8-K",
                        "section": section_name,
                        "filing_date": datetime.now().strftime("%Y-%m-%d"),
                        "fiscal_year": 2023,
                        "document_size_mb": round(content_size_mb, 2)
                    },
                    "chunk_size": 512,   # 512 chars per chunk  
                    "overlap": 64        # 64 char overlap
                }
                
                # Split large documents into smaller parts to avoid HTTP limits
                # Each part should be ~500KB max
                content_parts = []
                part_size_kb = 500  # 500KB per part
                part_size_chars = part_size_kb * 1024
                
                for i in range(0, len(content), part_size_chars):
                    part_content = content[i:i + part_size_chars]
                    content_parts.append(part_content)
                
                print(f"    📦 Splitting into {len(content_parts)} parts for ingestion...")
                
                total_chunks = 0
                for part_idx, part_content in enumerate(content_parts):
                    part_document = {
                        "text": part_content,
                        "collection_id": "sec_edgar_large_filings",
                        "document_id": f"{ticker}_{section_name}_part{part_idx}",
                        "metadata": {
                            "company": company_name,
                            "ticker": ticker,
                            "filing_type": "10-K" if section_name != "legal_proceedings" else "8-K",
                            "section": section_name,
                            "filing_date": datetime.now().strftime("%Y-%m-%d"),
                            "fiscal_year": 2023,
                            "document_size_mb": round(len(part_content) / (1024 * 1024), 2),
                            "part_index": part_idx,
                            "total_parts": len(content_parts)
                        },
                        "chunk_size": 512,   # 512 chars per chunk
                        "overlap": 64        # 64 char overlap
                    }
                    
                    # Ingest document part
                    async with session.post(
                        f"{base_url}/api/embeddings/ingest-document",
                        json=part_document
                    ) as resp:
                        if resp.status == 200:
                            result = await resp.json()
                            chunks = result.get("chunks", [])
                            total_chunks += len(chunks)
                        else:
                            error = await resp.text()
                            print(f"      ❌ Failed to ingest part {part_idx}: {error}")
                
                if total_chunks > 0:
                    documents_created.append({
                        "company": company_name,
                        "ticker": ticker,
                        "section": section_name,
                        "size_mb": content_size_mb,
                        "chunks": total_chunks,
                        "parts": len(content_parts)
                    })
                    print(f"    ✅ Successfully ingested {total_chunks} chunks across {len(content_parts)} parts")
            
            print()
        
        print(f"📊 Total data generated: {total_size:.1f}MB")
        print(f"📄 Total documents: {len(documents_created)}")
        print(f"🧩 Total chunks: {sum(doc['chunks'] for doc in documents_created)}")
        print()
        
        # Wait for indexing
        print("⏳ Waiting for indexing to complete...")
        await asyncio.sleep(5)
        
        # Perform searches
        print()
        print("🔍 Performing searches on large dataset...")
        print("=" * 80)
        
        queries = [
            {
                "query": "artificial intelligence machine learning AI investments R&D",
                "description": "AI and ML investments"
            },
            {
                "query": "cybersecurity data breach security threats ransomware attacks",
                "description": "Cybersecurity risks"
            },
            {
                "query": "revenue growth financial performance profitability margins",
                "description": "Financial performance"
            },
            {
                "query": "patent litigation antitrust investigation legal proceedings",
                "description": "Legal matters"
            },
            {
                "query": "cloud computing infrastructure AWS Azure Google Cloud",
                "description": "Cloud services"
            }
        ]
        
        for search_info in queries:
            print(f"\n🔍 Search: {search_info['description']}")
            print(f"   Query: '{search_info['query']}'")
            print("-" * 80)
            
            search_data = {
                "query": search_info["query"],
                "collection_id": "sec_edgar_large_filings",
                "top_k": 10
            }
            
            async with session.post(
                f"{base_url}/api/embeddings/search-text",
                json=search_data
            ) as resp:
                if resp.status == 200:
                    results = await resp.json()
                    
                    # Handle both list and dict response formats
                    if isinstance(results, dict) and "results" in results:
                        results_list = results["results"]
                    else:
                        results_list = results if isinstance(results, list) else []
                    
                    for i, result in enumerate(results_list[:5]):  # Show top 5
                        metadata = result.get("metadata", {})
                        score = result.get("score", 0)
                        
                        print(f"\n{i+1}. {metadata.get('company', 'Unknown')} - {metadata.get('section', 'Unknown')}")
                        print(f"   Ticker: {metadata.get('ticker', 'N/A')}")
                        print(f"   Document Size: {metadata.get('document_size_mb', 0):.1f}MB")
                        print(f"   Score: {score:.4f}")
                        
                        # Show snippet of matched content
                        chunk_id = result.get("id", "")
                        print(f"   Chunk: {chunk_id}")
                else:
                    error = await resp.text()
                    print(f"❌ Search failed: {error}")
        
        print("\n" + "=" * 80)
        print("✅ SEC EDGAR data generation completed!")
        print(f"📊 Summary:")
        print(f"   - Total data generated: {total_size:.1f}MB")
        print(f"   - Companies: {len(companies)}")
        print(f"   - Documents per company: {len(FILING_TEMPLATES)}")
        print(f"   - Total documents: {len(documents_created)}")
        print(f"   - Total chunks: {sum(doc['chunks'] for doc in documents_created)}")
        print(f"   - Collection: sec_edgar_large_filings")
        print()
        
        # Document breakdown
        print("📄 Document Breakdown:")
        for doc in documents_created[:10]:  # Show first 10
            parts_info = f" in {doc.get('parts', 1)} parts" if doc.get('parts', 1) > 1 else ""
            print(f"   - {doc['company']} {doc['section']}: {doc['size_mb']:.1f}MB ({doc['chunks']} chunks{parts_info})")
        if len(documents_created) > 10:
            print(f"   ... and {len(documents_created) - 10} more documents")
        
        return True  # Return success status

if __name__ == "__main__":
    asyncio.run(generate_sec_edgar_data())