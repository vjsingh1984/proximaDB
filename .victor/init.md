# Project Overview

ProximaDB is a multi-model vector database system with support for embeddings, graph, and full-text search. It is primarily implemented in Python, with components in Rust, Go, and JavaScript. Key capabilities include RAG enhancement, progressive search, and vector optimization.

# Package Layout

| Path       | Description                   |
|------------|-------------------------------|
| `src/`     | Core source code              |
| `docs/`    | Documentation                 |
| `examples/`| Sample workflows              |
| `scripts/` | Automation and helper scripts |
| `tests/`   | Unit and integration tests    |
| `demo/`    | Demo and showcase code        |
| `tools/`   | Utility tools                 |

# Key Entry Points

| Component                     | Path                                    | Description                                                  |
|------------------------------|-----------------------------------------|--------------------------------------------------------------|
| ProximaDBProgressiveClient   | `demo/progressive_search_demo.py:28`    | Client for progressive search support                        |
| BERTEmbeddingService         | `demo/utils/bert_embedding_service.py:24` | Service for generating BERT embeddings from text             |
| ChunkingService              | `demo/utils/chunking_utils.py:17`       | Service for chunking text and preparing vectors with metadata |
| LLMService                   | `demo/utils/llm_service.py:38`          | Lightweight LLM service for RAG enhancement                  |
| ProximaDBEmbeddingService    | `demo/showcases/advanced/embedding_service.py:72` | Comprehensive embedding service with real BERT embeddings    |
| PDFTranscriber               | `tools/pdf_transcriber.py:52`           | Class for PDF transcription                                  |
| SafePDFTranscriber           | `tools/safe_pdf_transcriber.py:20`      | Safe PDF transcription class                                 |
| ProximaDBFeatureShowcase     | `demo/quickstart/feature_showcase.py:48` | Class for showcasing core ProximaDB features                 |
| ProgressiveSearchConfig      | `demo/progressive_search_demo.py:20`    | Configuration for progressive search stages                  |
| DemoSetup                    | `demo/setup.py:46`                      | Setup class for demo environments                            |
| FilingType                   | `demo/showcases/advanced/sec_edgar_complete.py:76` | Enum-like class for SEC filing types                         |
| SectionType                  | `demo/showcases/advanced/sec_edgar_complete.py:88` | Enum-like class for SEC document sections                    |

# Development Commands

```bash
npm install
pytest
pip install -e ".[dev]"
```

# Dependencies

Core dependencies: numpy

# Configuration

Settings are loaded from `.env`, `~/.victor/profiles.yaml`, and CLI flags in that order.

# Architecture Notes

- Multi-model architecture with vector, graph, and full-text search support
- Embedding and chunking services are central to RAG workflows
- Progressive search client enables staged query execution
- Rust and Go components provide core performance-critical modules
- Demo and showcase classes act as entry points for system exploration

# Codebase Scale

1,488,398 lines of code across 2,677 files (1,291,024 LOC source, 197,374 LOC config)