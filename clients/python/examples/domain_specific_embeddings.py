#!/usr/bin/env python3
"""
Domain-Specific Embedding Examples for ProximaDB

This script demonstrates best practices for embedding domain-specific documents:
- Finance & SEC Filings
- Legal Documents & Case Law
- Medical & Healthcare
- Scientific Research

Each example shows recommended models and configuration for optimal results.
"""

import numpy as np
from typing import List, Dict, Any

from proximadb.embedding_providers import (
    BGEEmbeddingProvider,
    E5EmbeddingProvider,
    SFREmbeddingProvider,
    SentenceTransformerProvider,
    EmbeddingConfig
)


def demo_finance_sec_filings():
    """
    Demo: Finance & SEC Filings Embedding

    Best practices for:
    - SEC 10-K/10-Q filings
    - Earnings call transcripts
    - Financial research reports
    - Risk factor disclosures
    """
    print("\n" + "="*80)
    print("FINANCE & SEC FILINGS - BGE LARGE MODEL")
    print("="*80)

    # Recommended: BGE Large for finance
    config = EmbeddingConfig(
        model_name="BAAI/bge-large-en-v1.5",
        dimension=1024,
        batch_size=32,
        normalize=True
    )

    print(f"Model: {config.model_name}")
    print(f"Dimensions: {config.dimension}")
    print(f"Best for: Financial terminology, SEC filings, earnings reports")

    # Example SEC filing paragraphs
    sec_examples = [
        {
            "text": "Risk Factors: The Company's business is subject to substantial regulatory "
                   "oversight from the SEC, FINRA, and other regulatory bodies. Changes in "
                   "regulations could materially adversely affect our financial condition.",
            "section": "Risk Factors",
            "filing": "10-K"
        },
        {
            "text": "Management's Discussion and Analysis: Net revenue increased 23% year-over-year "
                   "to $4.8 billion, driven primarily by growth in our cloud infrastructure "
                   "business and improved operating leverage across the organization.",
            "section": "MD&A",
            "filing": "10-Q"
        },
        {
            "text": "For the fiscal year ended December 31, 2024, diluted earnings per share (EPS) "
                   "was $12.45, representing a 34% increase compared to $9.28 in the prior year. "
                   "Adjusted EBITDA margin expanded to 38.2% from 35.1%.",
            "section": "Financial Results",
            "filing": "10-K"
        }
    ]

    print("\nExample documents:")
    for i, doc in enumerate(sec_examples, 1):
        print(f"  {i}. [{doc['filing']} - {doc['section']}]")
        print(f"     {doc['text'][:100]}...")

    # Simulated usage (actual embedding commented out)
    print("\nUsage example:")
    print("  provider = BGEEmbeddingProvider(config)")
    print("  embeddings = provider.embed_documents(sec_examples)")
    print("  # Returns 1024-dimensional embeddings optimized for finance")

    # For research/analysis requiring maximum accuracy
    print("\nFor financial research (maximum accuracy):")
    print("  sfr_config = EmbeddingConfig(")
    print("      model_name='Salesforce/SFR-Embedding-2_R',")
    print("      dimension=4096")
    print("  )")
    print("  provider = SFREmbeddingProvider(sfr_config)")


def demo_legal_documents():
    """
    Demo: Legal Documents & Case Law Embedding

    Best practices for:
    - Court opinions and case law
    - Legal contracts
    - Statutes and regulations
    - Legal briefs
    """
    print("\n" + "="*80)
    print("LEGAL DOCUMENTS & CASE LAW - SFR MODEL")
    print("="*80)

    # Recommended: SFR for legal (complex reasoning)
    config = EmbeddingConfig(
        model_name="Salesforce/SFR-Embedding-2_R",
        dimension=4096,
        batch_size=16,  # Smaller batch for 4096 dims
        normalize=True
    )

    print(f"Model: {config.model_name}")
    print(f"Dimensions: {config.dimension}")
    print(f"Best for: Legal reasoning, case citations, complex legal language")

    # Example legal document paragraphs
    legal_examples = [
        {
            "text": "The Court finds that the defendant's motion for summary judgment must be "
                   "DENIED. Viewing the evidence in the light most favorable to the plaintiff, "
                   "a reasonable jury could find that the defendant breached its fiduciary duty.",
            "type": "Court Opinion",
            "citation": "Smith v. Jones, 123 F.3d 456 (9th Cir. 2024)"
        },
        {
            "text": "Pursuant to 28 U.S.C. § 1331, this Court has federal question jurisdiction "
                   "over this matter. The plaintiff's complaint raises substantial questions "
                   "of federal law under the Securities Exchange Act of 1934.",
            "type": "Jurisdictional Statement",
            "citation": "Doe v. Corp, No. CV-24-1234 (D.Del. 2024)"
        },
        {
            "text": "The parties hereby agree that any dispute arising out of or relating to this "
                   "Agreement shall be resolved through binding arbitration in accordance with "
                   "the Commercial Arbitration Rules of the American Arbitration Association.",
            "type": "Contract Provision",
            "citation": "Services Agreement § 12.3"
        }
    ]

    print("\nExample documents:")
    for i, doc in enumerate(legal_examples, 1):
        print(f"  {i}. [{doc['type']}]")
        print(f"     {doc['text'][:100]}...")
        print(f"     Citation: {doc['citation']}")

    print("\nUsage example:")
    print("  provider = SFREmbeddingProvider(config)")
    print("  embeddings = provider.embed_documents(legal_examples)")
    print("  # Returns 4096-dimensional embeddings for complex legal text")

    # Multilingual legal
    print("\nFor international/multilingual legal documents:")
    print("  bge_config = EmbeddingConfig(")
    print("      model_name='BAAI/bge-m3',")
    print("      dimension=1024")
    print("  )")
    print("  provider = BGEEmbeddingProvider(bge_config)")
    print("  # Supports 100+ languages for international law")


def demo_medical_healthcare():
    """
    Demo: Medical & Healthcare Document Embedding

    Best practices for:
    - Clinical notes
    - Medical research papers
    - Patient records
    - Drug documentation
    """
    print("\n" + "="*80)
    print("MEDICAL & HEALTHCARE - E5 LARGE MODEL")
    print("="*80)

    # Recommended: E5 Large for medical
    config = EmbeddingConfig(
        model_name="intfloat/e5-large-v2",
        dimension=1024,
        batch_size=32,
        normalize=True  # Required for E5
    )

    print(f"Model: {config.model_name}")
    print(f"Dimensions: {config.dimension}")
    print(f"Best for: Clinical terminology, medical research, healthcare documents")

    # Example medical document snippets
    medical_examples = [
        {
            "text": "Patient presents with acute myocardial infarction (STEMI). Troponin I "
                   "elevated at 12.4 ng/mL. ECG shows ST-segment elevation in leads II, III, "
                   "and aVF. Emergent cardiac catheterization recommended.",
            "type": "Clinical Note",
            "specialty": "Cardiology"
        },
        {
            "text": "Randomized controlled trial of 1,247 patients demonstrated that the novel "
                   "PCSK9 inhibitor reduced LDL cholesterol by 58% compared to placebo "
                   "(p<0.001), with significant reduction in major adverse cardiovascular events.",
            "type": "Research Abstract",
            "specialty": "Clinical Research"
        },
        {
            "text": "Indications and Usage: This medication is indicated for the treatment of "
                   "moderate to severe plaque psoriasis in adults who are candidates for "
                   "systemic therapy or phototherapy.",
            "type": "Drug Documentation",
            "specialty": "Dermatology"
        }
    ]

    print("\nExample documents:")
    for i, doc in enumerate(medical_examples, 1):
        print(f"  {i}. [{doc['type']} - {doc['specialty']}]")
        print(f"     {doc['text'][:100]}...")

    print("\nUsage example:")
    print("  provider = E5EmbeddingProvider(config)")
    print("  embeddings = provider.embed_documents(medical_examples)")
    print("  # E5 handles medical terminology excellently")


def demo_scientific_research():
    """
    Demo: Scientific Research & Academic Papers

    Best practices for:
    - arXiv papers
    - Research articles
    - Technical documentation
    - Academic abstracts
    """
    print("\n" + "="*80)
    print("SCIENTIFIC RESEARCH - SFR MODEL (MAX ACCURACY)")
    print("="*80)

    # Recommended: SFR for maximum accuracy in research
    config = EmbeddingConfig(
        model_name="Salesforce/SFR-Embedding-2_R",
        dimension=4096,
        batch_size=16,
        normalize=True
    )

    print(f"Model: {config.model_name}")
    print(f"Dimensions: {config.dimension}")
    print(f"Best for: Academic papers, technical jargon, research retrieval")

    # Example research paper snippets
    research_examples = [
        {
            "text": "We propose a novel attention mechanism that achieves O(n log n) complexity "
                   "while maintaining the representational power of standard self-attention. "
                   "Empirical results on BERT pretraining show 2.3x speedup with minimal "
                   "performance degradation.",
            "type": "Abstract",
            "field": "Machine Learning",
            "venue": "NeurIPS 2024"
        },
        {
            "text": "The photocatalytic reduction of CO2 to methanol was achieved using a novel "
                   "titanium-based metal-organic framework (Ti-MOF) under visible light "
                   "irradiation, with quantum efficiency of 12.4% and selectivity of 89%.",
            "type": "Results",
            "field": "Chemistry",
            "venue": "Nature Chemistry"
        },
        {
            "text": "Cryo-EM structure of the SARS-CoV-2 spike protein in complex with ACE2 "
                   "receptor reveals critical binding interactions at 2.9 Å resolution. "
                   "The RBD adopts a previously undescribed 'up' conformation.",
            "type": "Findings",
            "field": "Structural Biology",
            "venue": "Science"
        }
    ]

    print("\nExample documents:")
    for i, doc in enumerate(research_examples, 1):
        print(f"  {i}. [{doc['field']} - {doc['venue']}]")
        print(f"     {doc['text'][:100]}...")

    print("\nUsage example:")
    print("  provider = SFREmbeddingProvider(config)")
    print("  embeddings = provider.embed_documents(research_examples)")
    print("  # 4096 dimensions provide maximum accuracy for technical content")


def demo_complete_workflow():
    """
    Demo: Complete ProximaDB Integration with Domain-Specific Embeddings
    """
    print("\n" + "="*80)
    print("COMPLETE WORKFLOW: SEC FILINGS + PROXIMADB")
    print("="*80)

    print("""
# Complete workflow for SEC filings search system

from proximadb import ProximaDB
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

# 1. Initialize ProximaDB
client = ProximaDB(url="http://localhost:5678")

# 2. Create embedding provider for finance
config = EmbeddingConfig(
    model_name="BAAI/bge-large-en-v1.5",
    dimension=1024,
    batch_size=32,
    normalize=True
)
provider = BGEEmbeddingProvider(config)

# 3. Create collection for SEC filings
collection = client.create_collection(
    name="sec_filings",
    dimension=1024  # Must match embedding dimension
)

# 4. Prepare SEC filing documents
sec_docs = [
    {
        "text": "Risk Factors: Market volatility could impact revenue...",
        "company": "ACME Corp",
        "filing_type": "10-K",
        "section": "Risk Factors",
        "year": 2024
    },
    {
        "text": "MD&A: Revenue increased 23% due to cloud growth...",
        "company": "ACME Corp",
        "filing_type": "10-Q",
        "section": "MD&A",
        "quarter": "Q3 2024"
    }
]

# 5. Generate embeddings using BGE (optimized for finance)
embeddings = provider.embed_documents(sec_docs)

# 6. Insert into ProximaDB
for i, (doc, embedding) in enumerate(zip(sec_docs, embeddings)):
    collection.insert({
        "id": f"filing_{i}",
        "vector": embedding.tolist(),
        "metadata": doc
    })

# 7. Search with financial queries
query = "What are the main revenue growth drivers?"
query_embedding = provider.embed_query(query)

results = collection.search(
    query_vector=query_embedding.tolist(),
    top_k=5,
    filter={"filing_type": "10-Q"}  # Only quarterly reports
)

# 8. Process results
for result in results:
    print(f"Company: {result.metadata['company']}")
    print(f"Section: {result.metadata['section']}")
    print(f"Relevance: {result.score:.3f}")
    print(f"Text: {result.metadata['text'][:200]}...")
    print()
    """)


def demo_model_selection_guide():
    """
    Quick reference guide for domain-specific model selection
    """
    print("\n" + "="*80)
    print("DOMAIN-SPECIFIC MODEL SELECTION GUIDE")
    print("="*80)

    print("\n📊 Quick Reference:")
    print("┌─────────────────────────┬──────────────────────────┬────────┬─────────────────────┐")
    print("│ Domain                  │ Recommended Model        │ Dims   │ Why?                │")
    print("├─────────────────────────┼──────────────────────────┼────────┼─────────────────────┤")
    print("│ Finance/SEC             │ bge-large-en-v1.5        │ 1024   │ Financial vocab     │")
    print("│ Finance (Research)      │ SFR-Embedding-2_R        │ 4096   │ Max accuracy        │")
    print("│ Legal (English)         │ SFR-Embedding-2_R        │ 4096   │ Complex reasoning   │")
    print("│ Legal (Multilingual)    │ bge-m3                   │ 1024   │ 100+ languages      │")
    print("│ Medical/Healthcare      │ e5-large-v2              │ 1024   │ Clinical terms      │")
    print("│ Scientific Research     │ SFR-Embedding-2_R        │ 4096   │ Technical jargon    │")
    print("│ Code/Tech Docs          │ bge-large-en-v1.5        │ 1024   │ Technical docs      │")
    print("│ General Purpose         │ bge-large-en-v1.5        │ 1024   │ Best balance        │")
    print("└─────────────────────────┴──────────────────────────┴────────┴─────────────────────┘")

    print("\n💡 Key Insights:")
    print("  • Finance: BGE handles financial terminology (EBITDA, EPS, etc.) excellently")
    print("  • Legal: SFR's 4096 dimensions capture complex legal reasoning")
    print("  • Medical: E5 works well with clinical and pharmaceutical terms")
    print("  • Research: SFR maximum accuracy for technical papers")
    print("  • Multilingual: BGE-m3 for international documents (100+ languages)")

    print("\n⚡ Performance Tips:")
    print("  • Use batch_size=32 for 1024-dim models, batch_size=16 for 4096-dim")
    print("  • Always set normalize=True for cosine similarity")
    print("  • For production: Start with BGE or E5, upgrade to SFR for max accuracy")
    print("  • Consider fine-tuning on domain-specific data for best results")


def main():
    """Run all domain-specific demonstrations"""
    print("\n" + "="*80)
    print(" " * 15 + "DOMAIN-SPECIFIC EMBEDDING GUIDE FOR PROXIMADB")
    print("="*80)
    print("\nThis guide shows best practices for embedding domain-specific documents:")
    print("  • Finance & SEC Filings")
    print("  • Legal Documents & Case Law")
    print("  • Medical & Healthcare")
    print("  • Scientific Research")
    print("\nNote: Actual model downloads disabled to keep demo fast.")

    # Run all demos
    demo_finance_sec_filings()
    demo_legal_documents()
    demo_medical_healthcare()
    demo_scientific_research()
    demo_complete_workflow()
    demo_model_selection_guide()

    print("\n" + "="*80)
    print("GUIDE COMPLETE")
    print("="*80)
    print("\nFor more information:")
    print("  • EMBEDDING_PROVIDERS.md - Full documentation with domain recommendations")
    print("  • examples/embedding_providers_demo.py - General provider examples")
    print("  • https://huggingface.co/spaces/mteb/leaderboard - MTEB rankings")
    print("\n")


if __name__ == "__main__":
    main()
