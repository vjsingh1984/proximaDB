# Chunking System Migration Guide

## Overview

The ProximaDB Python SDK chunking system has been refactored to provide better separation of concerns and a more modular architecture. This guide helps you migrate from the old combined chunking/embedding approach to the new clean architecture.

## Key Changes

### 1. Separation of Concerns

**Before**: Chunking and embedding were mixed together
```python
# Old approach - chunking knew about embeddings
chunks = chunker.chunk_text(text, enable_bert_embeddings=True)
```

**After**: Chunking and embedding are separate operations
```python
# New approach - clean separation
chunker = TextChunker(config)
chunks = chunker.chunk_text(text, source_id)

# Generate embeddings separately
embeddings = embedding_provider.embed_texts([c.text for c in chunks])

# Combine into vector records
records = create_vector_records(chunks, embeddings)
```

### 2. Pluggable Chunking Strategies

**Before**: Single monolithic chunking class
```python
chunker = TextChunker(ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC))
```

**After**: Each strategy in its own module
```python
# Direct strategy usage
from proximadb.chunking_strategies import SemanticStrategy
strategy = SemanticStrategy(config)
chunks = strategy.chunk(text, source_id)

# Or use the factory
from proximadb.chunking_strategies import get_chunking_strategy
strategy = get_chunking_strategy("semantic", chunk_size=1000)
```

### 3. Clean Metadata Structure

**Before**: Mixed chunking and embedding metadata
```python
chunk.metadata = {
    "chunk_type": "bert_semantic_enhanced",
    "embedding_model": "all-MiniLM-L6-v2",
    "coherence_score": 0.85
}
```

**After**: Pure chunking metadata only
```python
chunk.metadata = {
    "chunk_type": "semantic",
    "chunk_index": 0,
    "section_type": "content",
    "has_header": True,
    "header_title": "Introduction"
}
```

## Migration Steps

### Step 1: Update Imports

```python
# Old imports
from proximadb.chunking import TextChunker, ChunkingConfig, chunks_to_vector_records

# New imports
from proximadb.chunking_refactored import (
    TextChunker,
    ChunkingConfig,
    ChunkingStrategy,
    create_vector_records,
    chunk_and_embed_text
)
from proximadb.embedding_interface import get_default_embedding_provider
```

### Step 2: Separate Chunking and Embedding

#### Old Code:
```python
def process_document(text, doc_id):
    chunker = TextChunker(ChunkingConfig(
        strategy=ChunkingStrategy.SEMANTIC,
        enable_bert_embeddings=True
    ))
    
    # This did both chunking and embedding
    chunks = chunker.chunk_text(text, doc_id)
    
    # Convert to records
    records = chunks_to_vector_records(chunks, embeddings)
    return records
```

#### New Code:
```python
def process_document(text, doc_id):
    # 1. Chunk text
    chunker = TextChunker(ChunkingConfig(
        strategy=ChunkingStrategy.SEMANTIC,
        chunk_size=1000
    ))
    chunks = chunker.chunk_text(text, doc_id)
    
    # 2. Generate embeddings
    embedding_provider = get_default_embedding_provider()
    embeddings = embedding_provider.embed_texts([c.text for c in chunks])
    
    # 3. Create vector records
    records = create_vector_records(chunks, embeddings.tolist())
    return records
```

### Step 3: Use Convenience Function (Optional)

If you want to keep the combined approach:

```python
# Use the convenience function that maintains separation internally
records = chunk_and_embed_text(
    text=document_text,
    source_id="doc_123",
    embedding_provider=get_default_embedding_provider(),
    chunking_config=ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC),
    metadata={"author": "John Doe"},
    filterable_fields=["author", "source_id"]
)
```

### Step 4: Update Custom Strategies

If you have custom chunking strategies:

**Before**:
```python
class MyCustomChunker(TextChunker):
    def _chunk_custom(self, text, source_id, metadata):
        # Custom logic mixed with embedding concerns
        if self.config.enable_bert_embeddings:
            embeddings = self._get_embeddings(text)
        # ...
```

**After**:
```python
from proximadb.chunking_strategies.base import ChunkingStrategyInterface

class MyCustomStrategy(ChunkingStrategyInterface):
    def chunk(self, text, source_id, base_metadata=None):
        # Pure chunking logic only
        chunks = []
        # Your custom chunking logic here
        return chunks
        
# Register the strategy
from proximadb.chunking_strategies import ChunkingStrategyFactory
ChunkingStrategyFactory.register_strategy(
    ChunkingStrategy.CUSTOM,
    MyCustomStrategy
)
```

## Benefits of the New Architecture

1. **Cleaner Code**: Each concern is handled separately
2. **Better Testing**: Can test chunking without embedding dependencies
3. **More Flexible**: Easy to swap chunking strategies or embedding providers
4. **Performance**: Can optimize chunking and embedding independently
5. **Extensibility**: Easy to add new chunking strategies

## Common Patterns

### Pattern 1: Batch Processing
```python
def process_documents(documents):
    chunker = TextChunker(ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC))
    embedding_provider = get_default_embedding_provider()
    
    all_records = []
    
    for doc in documents:
        # Chunk
        chunks = chunker.chunk_text(doc.text, doc.id)
        
        # Batch embed for efficiency
        if chunks:
            embeddings = embedding_provider.embed_texts([c.text for c in chunks])
            records = create_vector_records(chunks, embeddings.tolist())
            all_records.extend(records)
    
    return all_records
```

### Pattern 2: Custom Metadata
```python
def process_with_metadata(text, doc_id, custom_metadata):
    # Chunk with base metadata
    chunker = TextChunker()
    chunks = chunker.chunk_text(text, doc_id, custom_metadata)
    
    # Add chunk-specific metadata
    for i, chunk in enumerate(chunks):
        chunk.metadata["processing_timestamp"] = time.time()
        chunk.metadata["chunk_hash"] = hashlib.md5(chunk.text.encode()).hexdigest()
    
    # Generate embeddings and create records
    embeddings = embedding_provider.embed_texts([c.text for c in chunks])
    records = create_vector_records(
        chunks, 
        embeddings.tolist(),
        filterable_fields=["source_id", "processing_timestamp"]
    )
    
    return records
```

### Pattern 3: Strategy Selection
```python
def smart_chunk(text, doc_type):
    # Choose strategy based on document type
    if doc_type == "code":
        strategy = ChunkingStrategy.SEMANTIC  # Preserves code blocks
    elif doc_type == "article":
        strategy = ChunkingStrategy.PARAGRAPH
    elif doc_type == "conversation":
        strategy = ChunkingStrategy.SENTENCE
    else:
        strategy = ChunkingStrategy.SLIDING_WINDOW
    
    chunker = TextChunker(ChunkingConfig(strategy=strategy))
    return chunker.chunk_text(text, f"{doc_type}_doc")
```

## Troubleshooting

### Issue: "bert_semantic_enhanced" chunk type not found
**Solution**: This chunk type has been removed. Use "semantic" instead and handle embeddings separately.

### Issue: Missing embedding_model in metadata
**Solution**: Embedding metadata is now added during the `create_vector_records` step, not during chunking.

### Issue: Custom chunking strategy not working
**Solution**: Ensure your custom strategy inherits from `ChunkingStrategyInterface` and implements the `chunk` method.

## Support

For questions or issues with migration:
- Check the test files for examples: `test_chunking_refactored.py`
- Review the API documentation in the module docstrings
- Open an issue on GitHub with the `chunking` label