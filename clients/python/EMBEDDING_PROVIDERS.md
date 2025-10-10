# ProximaDB Embedding Providers

Comprehensive guide to using open-source embedding models with ProximaDB Python SDK.

## 🌟 Top Open-Source Models (Ranked by MTEB Performance)

All models automatically download from HuggingFace on first use.

### 1. SFR - Salesforce Research (Best Accuracy)
**Provider:** `SFREmbeddingProvider`
**Top Model:** `Salesforce/SFR-Embedding-2_R`
- **Dimensions:** 4096
- **MTEB Score:** 66.4 (Top performer)
- **Best For:** Maximum accuracy, research, when quality is paramount
- **Speed:** Slower (large dimensions)

```python
from proximadb.embedding_providers import SFREmbeddingProvider, EmbeddingConfig

# Best quality configuration
config = EmbeddingConfig(
    model_name="Salesforce/SFR-Embedding-2_R",
    dimension=4096,
    batch_size=16,  # Smaller batch due to large dims
    normalize=True
)

provider = SFREmbeddingProvider(config)

# For queries
query_emb = provider.embed_query("What is machine learning?")

# For passages/documents
doc_embs = provider.embed_documents([
    {"text": "Machine learning is a subset of AI..."}
])
```

### 2. BGE - Beijing Academy of AI (Top MTEB Retrieval)
**Provider:** `BGEEmbeddingProvider`
**Top Model:** `BAAI/bge-large-en-v1.5`
- **Dimensions:** 1024
- **MTEB Retrieval:** Top-3 performer
- **Best For:** Production retrieval, semantic search
- **Speed:** Fast (optimized architecture)

```python
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

# Production-ready configuration
config = EmbeddingConfig(
    model_name="BAAI/bge-large-en-v1.5",
    dimension=1024,
    normalize=True
)

provider = BGEEmbeddingProvider(config)

# BGE uses special instruction for queries
query_emb = provider.embed_query("machine learning tutorial")

# Documents don't need instruction
doc_embs = provider.embed_documents([
    {"text": "This is a tutorial about ML"}
])
```

**Available BGE Models:**
- `BAAI/bge-large-en-v1.5` - Best quality (1024 dims)
- `BAAI/bge-base-en-v1.5` - Balanced (768 dims)
- `BAAI/bge-small-en-v1.5` - Fast (384 dims)
- `BAAI/bge-m3` - Multilingual (1024 dims, 100+ languages)

### 3. E5 - Microsoft (Excellent General Purpose)
**Provider:** `E5EmbeddingProvider`
**Top Model:** `intfloat/e5-large-v2`
- **Dimensions:** 1024
- **MTEB Score:** 65+ (Top-5 performer)
- **Best For:** General purpose, production use
- **Speed:** Fast

```python
from proximadb.embedding_providers import E5EmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(
    model_name="intfloat/e5-large-v2",
    dimension=1024,
    normalize=True  # Required for E5
)

provider = E5EmbeddingProvider(config)

# E5 requires "query: " prefix for queries
query_emb = provider.embed_query("python tutorial")  # Auto-prefixed

# E5 requires "passage: " prefix for documents
doc_embs = provider.embed_passages([
    "Python is a programming language"
])  # Auto-prefixed
```

**Available E5 Models:**
- `intfloat/e5-large-v2` - Best quality (1024 dims)
- `intfloat/e5-base-v2` - Balanced (768 dims)
- `intfloat/e5-small-v2` - Fast (384 dims)
- `intfloat/multilingual-e5-large` - Multilingual (1024 dims)

### 4. Sentence-Transformers (Most Versatile)
**Provider:** `SentenceTransformerProvider`
**Top Models:** `all-mpnet-base-v2`, `all-MiniLM-L6-v2`
- **Dimensions:** 768 (mpnet) or 384 (MiniLM)
- **Best For:** Quick start, wide model selection
- **Speed:** Very fast (MiniLM)

```python
from proximadb.embedding_providers import SentenceTransformerProvider, EmbeddingConfig

# Best quality option
config_quality = EmbeddingConfig(
    model_name="all-mpnet-base-v2",
    dimension=768
)

# Fastest option
config_fast = EmbeddingConfig(
    model_name="all-MiniLM-L6-v2",
    dimension=384
)

provider = SentenceTransformerProvider(config_quality)
embeddings = provider.embed_texts(["your text here"])
```

## 🚀 Quick Start Examples

### Factory Pattern (Recommended)

```python
from proximadb.embedding_providers import EmbeddingProviderFactory, EmbeddingConfig

# Create provider using factory
config = EmbeddingConfig(
    model_name="BAAI/bge-large-en-v1.5",
    dimension=1024
)

provider = EmbeddingProviderFactory.create_provider(
    "bge",  # Provider type
    config=config
)

# Generate embeddings
embeddings = provider.embed_texts([
    "artificial intelligence",
    "machine learning",
    "deep learning"
])
```

### Complete ProximaDB Integration

```python
from proximadb import ProximaDB
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

# Initialize ProximaDB client
client = ProximaDB(url="http://localhost:5678")

# Create embedding provider
embedding_config = EmbeddingConfig(
    model_name="BAAI/bge-base-en-v1.5",
    dimension=768,
    batch_size=32
)
embedding_provider = BGEEmbeddingProvider(embedding_config)

# Create collection
collection = client.create_collection(
    name="my_documents",
    dimension=768  # Must match embedding dimension
)

# Embed and insert documents
documents = [
    {"text": "AI is transforming technology", "category": "tech"},
    {"text": "Machine learning enables predictions", "category": "tech"},
    {"text": "Deep learning uses neural networks", "category": "tech"}
]

# Generate embeddings
doc_embeddings = embedding_provider.embed_documents(documents)

# Insert into ProximaDB
for i, (doc, embedding) in enumerate(zip(documents, doc_embeddings)):
    collection.insert({
        "id": f"doc_{i}",
        "vector": embedding.tolist(),
        "metadata": doc
    })

# Search with query
query = "what is artificial intelligence?"
query_embedding = embedding_provider.embed_query(query)

results = collection.search(
    query_vector=query_embedding.tolist(),
    top_k=5
)

for result in results:
    print(f"Score: {result.score}, Text: {result.metadata['text']}")
```

## 📊 Model Comparison

| Model | Provider | Dimensions | MTEB Score | Speed | Use Case |
|-------|----------|------------|------------|-------|----------|
| **SFR-Embedding-2_R** | SFR | 4096 | 66.4 | Slow | Best accuracy |
| **bge-large-en-v1.5** | BGE | 1024 | 64+ | Fast | Production retrieval |
| **e5-large-v2** | E5 | 1024 | 65+ | Fast | General purpose |
| **all-mpnet-base-v2** | ST | 768 | 63+ | Fast | Balanced quality |
| **bge-base-en-v1.5** | BGE | 768 | 63+ | Fast | Production balanced |
| **all-MiniLM-L6-v2** | ST | 384 | 59+ | Very Fast | High throughput |
| **bge-small-en-v1.5** | BGE | 384 | 62+ | Very Fast | Latency-sensitive |

**Legend:**
- SFR: Salesforce Research
- BGE: Beijing Academy of AI
- E5: Microsoft
- ST: Sentence-Transformers

## 🎯 Choosing the Right Model

### Maximum Accuracy
```python
# Use SFR-Embedding-2_R
from proximadb.embedding_providers import SFREmbeddingProvider

provider = SFREmbeddingProvider()  # Uses SFR-Embedding-2_R by default
```

### Production Balance (Quality + Speed)
```python
# Use BGE-large or E5-large
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(
    model_name="BAAI/bge-large-en-v1.5",
    dimension=1024
)
provider = BGEEmbeddingProvider(config)
```

### High Throughput / Low Latency
```python
# Use BGE-small or MiniLM
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(
    model_name="BAAI/bge-small-en-v1.5",
    dimension=384,
    batch_size=64  # Larger batches with smaller dims
)
provider = BGEEmbeddingProvider(config)
```

### Multilingual Support
```python
# Use BGE-m3 or multilingual-e5
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(
    model_name="BAAI/bge-m3",
    dimension=1024
)
provider = BGEEmbeddingProvider(config)

# Works with 100+ languages
texts_multi = [
    "Hello world",  # English
    "Bonjour le monde",  # French
    "Hola mundo",  # Spanish
    "你好世界"  # Chinese
]
embeddings = provider.embed_texts(texts_multi)
```

## 💾 Model Caching

Models are automatically cached by HuggingFace:
- **Default location:** `~/.cache/huggingface/`
- **Custom cache:**
```python
config = EmbeddingConfig(
    model_name="BAAI/bge-large-en-v1.5",
    dimension=1024,
    cache_dir="/custom/cache/path"  # Custom cache location
)
```

## ⚡ Performance Optimization

### GPU Acceleration
```python
config = EmbeddingConfig(
    model_name="BAAI/bge-large-en-v1.5",
    dimension=1024,
    device="cuda",  # Use GPU
    batch_size=64  # Larger batches on GPU
)
provider = BGEEmbeddingProvider(config)
```

### Batch Processing
```python
# Process large document sets efficiently
texts = [f"Document {i}" for i in range(10000)]

# Automatic batching
embeddings = provider.embed_texts(texts)  # Uses config.batch_size
```

### Query vs Passage Optimization

#### BGE Models
```python
# Queries: Use instruction prefix
query_emb = provider.embed_query("search query")

# Passages: No instruction
passage_embs = provider.embed_documents(documents)
```

#### E5 Models
```python
# Queries: "query: " prefix (automatic)
query_emb = provider.embed_query("search query")

# Passages: "passage: " prefix (automatic)
passage_embs = provider.embed_passages(passage_texts)
```

## 🔧 Installation

```bash
# Install ProximaDB with embedding support
pip install proximadb[embeddings]

# Or install specific dependencies
pip install sentence-transformers torch
```

## 📚 Additional Resources

- [MTEB Leaderboard](https://huggingface.co/spaces/mteb/leaderboard)
- [BGE Models](https://huggingface.co/BAAI)
- [E5 Models](https://huggingface.co/intfloat)
- [SFR Models](https://huggingface.co/Salesforce)
- [Sentence-Transformers](https://www.sbert.net/)

## 🏢 Domain-Specific Recommendations

### Finance & SEC Filings

For financial documents, SEC filings, earnings calls, and financial reports:

**Top Choice: BGE or E5 Large Models**
```python
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

# Best for finance/SEC filings
config = EmbeddingConfig(
    model_name="BAAI/bge-large-en-v1.5",
    dimension=1024,
    batch_size=32,
    normalize=True
)
provider = BGEEmbeddingProvider(config)

# For SEC filing paragraphs
sec_paragraphs = [
    {"text": "Risk Factors: The Company's business is subject to substantial regulatory oversight..."},
    {"text": "Management's Discussion and Analysis: Net revenue increased 23% year-over-year..."}
]

embeddings = provider.embed_documents(sec_paragraphs)
```

**Why BGE/E5 for Finance:**
- Trained on diverse text including financial content
- Excellent performance on technical vocabulary (EBITDA, diluted EPS, etc.)
- Strong retrieval performance for long-form documents
- Handles both narrative (MD&A) and tabular data contexts

**Alternative for Maximum Accuracy:**
```python
from proximadb.embedding_providers import SFREmbeddingProvider

# Highest quality for financial research
provider = SFREmbeddingProvider()  # Uses SFR-Embedding-2_R
```

### Legal Documents & Case Law

For legal contracts, case law, statutes, and legal opinions:

**Top Choice: SFR or BGE-m3 (for multilingual)**
```python
from proximadb.embedding_providers import SFREmbeddingProvider, EmbeddingConfig

# Best for legal documents
config = EmbeddingConfig(
    model_name="Salesforce/SFR-Embedding-2_R",
    dimension=4096,
    batch_size=16,
    normalize=True
)
provider = SFREmbeddingProvider(config)

# For legal case paragraphs
legal_docs = [
    {"text": "The Court finds that the defendant's motion for summary judgment..."},
    {"text": "Pursuant to 28 U.S.C. § 1331, this Court has federal question jurisdiction..."}
]

embeddings = provider.embed_documents(legal_docs)
```

**Why SFR/BGE for Legal:**
- Handles complex sentence structures and legal terminology
- Excellent for case citations and cross-references
- Strong performance on precedent-based retrieval
- Works with both statutes and case opinions

**For Multilingual Legal (International Law):**
```python
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

# Multilingual legal documents
config = EmbeddingConfig(
    model_name="BAAI/bge-m3",
    dimension=1024,
    batch_size=32
)
provider = BGEEmbeddingProvider(config)

# Handles 100+ languages
legal_multi = [
    {"text": "Article 6.1 of the European Convention on Human Rights..."},
    {"text": "Code Civil Article 1134: Les conventions légalement formées..."}
]

embeddings = provider.embed_documents(legal_multi)
```

### Medical & Healthcare

For medical records, clinical notes, and research papers:

```python
# Use E5 or BGE large models
from proximadb.embedding_providers import E5EmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(
    model_name="intfloat/e5-large-v2",
    dimension=1024,
    normalize=True
)
provider = E5EmbeddingProvider(config)
```

### Scientific Research & Academic Papers

For arXiv papers, research articles, and technical documentation:

```python
# SFR for maximum accuracy
from proximadb.embedding_providers import SFREmbeddingProvider

provider = SFREmbeddingProvider()  # 4096 dims, best accuracy
```

### Domain-Specific Model Comparison

| Domain | Best Model | Dimension | Why? |
|--------|-----------|-----------|------|
| **Finance/SEC** | bge-large-en-v1.5 | 1024 | Excellent retrieval, handles financial terminology |
| **Finance (Research)** | SFR-Embedding-2_R | 4096 | Maximum accuracy for analysis |
| **Legal (English)** | SFR-Embedding-2_R | 4096 | Best for complex legal reasoning |
| **Legal (Multi)** | bge-m3 | 1024 | 100+ languages, international law |
| **Medical** | e5-large-v2 | 1024 | Great for clinical terminology |
| **Scientific** | SFR-Embedding-2_R | 4096 | Handles technical jargon excellently |
| **Code/Tech Docs** | bge-large-en-v1.5 | 1024 | Good for technical documentation |

### Fine-Tuning for Your Domain

All providers support using custom-trained models:

```python
from proximadb.embedding_providers import SentenceTransformerProvider, EmbeddingConfig

# Use your own fine-tuned model
config = EmbeddingConfig(
    model_name="your-org/finance-tuned-bge-large",  # Your custom model
    dimension=1024
)
provider = SentenceTransformerProvider(config)
```

**Fine-tuning tips:**
1. Start with BGE or E5 base models
2. Use domain-specific training pairs (e.g., question-SEC filing paragraph)
3. Typical training: 10K-100K domain-specific pairs
4. Upload to HuggingFace and use model name directly

## 🤝 Contributing

To add a new embedding provider:
1. Extend `EmbeddingProvider` base class
2. Implement all abstract methods
3. Add to `EmbeddingProviderFactory`
4. Add comprehensive tests
5. Update documentation
