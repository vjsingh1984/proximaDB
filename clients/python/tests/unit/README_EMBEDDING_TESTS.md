# Embedding Provider Tests

This directory contains comprehensive tests for all embedding providers in the ProximaDB Python SDK.

## Test Files

### `test_embedding_providers.py`
**Base tests** - No model downloads required
- Tests provider interfaces and configurations
- Tests SimulatedEmbeddingProvider (no external dependencies)
- Tests EmbeddingProviderFactory
- **27 tests** - All can run without downloading models

### `test_real_embedding_providers.py`
**Real model tests** - Requires model downloads
- Tests actual embedding models from HuggingFace
- Tests all 5 provider implementations (gte-Qwen, SFR, BGE, E5, Sentence-Transformers)
- **50+ tests** covering:
  - Model initialization
  - Embedding generation
  - Query vs document embeddings
  - Multilingual support
  - Edge cases
  - Cross-provider comparisons

## Running Tests

### Quick Start (No Downloads)
```bash
# Run base tests only (no model downloads)
pytest tests/unit/test_embedding_providers.py -v
```

### Full Test Suite (With Model Downloads)
```bash
# First time: Download all models (~21GB)
python3 scripts/download_and_verify_models.py

# Run all tests including real models
pytest tests/unit/test_real_embedding_providers.py -v -m "requires_models"
```

### Selective Testing

#### Skip Model-Required Tests
```bash
# Skip tests that require downloaded models
pytest tests/unit/ -m "not requires_models"
```

#### Run Only Fast Tests
```bash
# Skip slow tests (large models like SFR)
pytest tests/unit/test_real_embedding_providers.py -m "not slow"
```

#### Run Specific Provider Tests
```bash
# Test only gte-Qwen provider
pytest tests/unit/test_real_embedding_providers.py::TestGTEQwenProvider -v

# Test only BGE provider
pytest tests/unit/test_real_embedding_providers.py::TestBGEProvider -v

# Test only E5 provider
pytest tests/unit/test_real_embedding_providers.py::TestE5Provider -v
```

#### Run Edge Case Tests
```bash
# Test edge cases (uses fast small model)
pytest tests/unit/test_real_embedding_providers.py::TestEdgeCases -v
```

## Test Markers

The test suite uses pytest markers for flexible test selection:

- `@pytest.mark.requires_models` - Requires downloaded models (skip in CI)
- `@pytest.mark.slow` - Slow tests (large models like SFR-4096)
- Default tests - Fast, no downloads required

## Models Tested

### 1. gte-Qwen (Alibaba NLP)
- **Model**: Alibaba-NLP/gte-Qwen2-1.5B-instruct
- **Dimensions**: 1536
- **Size**: ~3GB
- **MTEB Score**: 68.0+ (English), 69.0+ (Chinese)
- **Specialty**: #1 multilingual, 100+ languages

### 2. SFR (Salesforce Research)
- **Model**: Salesforce/SFR-Embedding-2_R
- **Dimensions**: 4096
- **Size**: ~14GB
- **MTEB Score**: 66.4
- **Specialty**: Top English accuracy
- **Note**: Marked as `@pytest.mark.slow` due to size

### 3. BGE (BAAI)
- **Models**:
  - bge-large-en-v1.5 (1024 dims, ~1.3GB)
  - bge-base-en-v1.5 (768 dims, ~438MB)
  - bge-small-en-v1.5 (384 dims, ~134MB)
- **MTEB Score**: 62-64+
- **Specialty**: Best retrieval performance

### 4. E5 (Microsoft)
- **Models**:
  - e5-large-v2 (1024 dims, ~1.3GB)
  - e5-base-v2 (768 dims, ~438MB)
- **MTEB Score**: 65+
- **Specialty**: Excellent general purpose

### 5. Sentence-Transformers
- **Models**:
  - all-mpnet-base-v2 (768 dims, ~438MB)
  - all-MiniLM-L6-v2 (384 dims, ~91MB)
- **MTEB Score**: 59-63+
- **Specialty**: Most versatile, fastest inference

## Test Coverage

### Provider Features Tested
✅ Initialization and configuration
✅ Single text embedding
✅ Batch text embedding
✅ Document embedding with metadata
✅ Query embedding with instructions
✅ Dimension validation
✅ Normalization verification
✅ Model info retrieval
✅ Availability checking

### Edge Cases Tested
✅ Empty input
✅ Single text input
✅ Very long text (truncation)
✅ Special characters
✅ Unicode and emoji
✅ Batch processing (100-500 texts)
✅ Multilingual text

### Cross-Provider Tests
✅ Semantic similarity preservation
✅ Dimension consistency
✅ Instruction prefix effects

## Performance Notes

### Model Loading Times (First Run)
- **gte-Qwen 1.5B**: ~10-15 seconds
- **SFR 4096**: ~30-60 seconds (large model)
- **BGE Large**: ~5-10 seconds
- **BGE Small**: ~2-5 seconds
- **E5 Base**: ~5-10 seconds
- **MiniLM**: ~2-5 seconds (fastest)

### Subsequent Runs
Models are cached in `~/.cache/huggingface/` and load much faster:
- **Small models (<500MB)**: ~1-2 seconds
- **Large models (1-3GB)**: ~3-5 seconds
- **Very large (10GB+)**: ~10-15 seconds

### Test Execution Time
- **Base tests** (`test_embedding_providers.py`): ~2 seconds
- **Edge case tests**: ~1-2 minutes (includes model loading)
- **Single provider test**: ~2-5 minutes
- **Full test suite** (all providers, all models): ~15-30 minutes

## CI/CD Integration

### Skip Model Tests in CI
```yaml
# In your CI configuration
- name: Run tests
  run: |
    pytest tests/unit/ -m "not requires_models" -v
```

### Run Full Tests (with caching)
```yaml
- name: Cache HuggingFace models
  uses: actions/cache@v3
  with:
    path: ~/.cache/huggingface
    key: huggingface-models-${{ hashFiles('scripts/download_and_verify_models.py') }}

- name: Download models
  run: |
    python3 scripts/download_and_verify_models.py

- name: Run all tests
  run: |
    pytest tests/unit/ -v
```

## Debugging Failed Tests

### Check Model Availability
```python
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(model_name="BAAI/bge-small-en-v1.5", dimension=384)
provider = BGEEmbeddingProvider(config)

print(f"Available: {provider.is_available()}")
print(f"Model info: {provider.get_model_info()}")
```

### Verify Model Download
```bash
# Check if models are cached
ls -lh ~/.cache/huggingface/hub/

# Re-download specific model
python3 -c "
from sentence_transformers import SentenceTransformer
model = SentenceTransformer('BAAI/bge-small-en-v1.5')
print('Model loaded successfully')
"
```

### Test Individual Functions
```python
import pytest
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(model_name="BAAI/bge-small-en-v1.5", dimension=384, normalize=True)
provider = BGEEmbeddingProvider(config)

# Test embedding generation
texts = ["test 1", "test 2"]
embeddings = provider.embed_texts(texts)

print(f"Shape: {embeddings.shape}")
print(f"Norms: {np.linalg.norm(embeddings, axis=1)}")
```

## Contributing

When adding new providers:

1. Add provider implementation in `src/proximadb/embedding_providers/`
2. Register in `factory.py`
3. Export in `__init__.py`
4. Add tests to `test_real_embedding_providers.py`:
   ```python
   class TestNewProvider:
       @pytest.fixture
       def config(self):
           return EmbeddingConfig(model_name="...", dimension=...)

       def test_initialization(self, config):
           provider = NewProvider(config)
           assert provider.is_available()

       def test_embed_texts(self, config, sample_texts):
           provider = NewProvider(config)
           embeddings = provider.embed_texts(sample_texts)
           assert embeddings.shape == (len(sample_texts), expected_dim)
   ```
5. Update `scripts/download_and_verify_models.py`
6. Run full test suite

## References

- [MTEB Leaderboard](https://huggingface.co/spaces/mteb/leaderboard)
- [Sentence-Transformers Documentation](https://www.sbert.net/)
- [HuggingFace Model Hub](https://huggingface.co/models)
- [ProximaDB Embedding Providers Guide](../../EMBEDDING_PROVIDERS.md)
