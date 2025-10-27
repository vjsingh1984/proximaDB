# Test Embeddings and Caching

This test suite uses realistic text embeddings via SentenceTransformers instead of random vectors to better reflect production usage and reduce false positives in search behavior.

What’s used
- Model: `sentence-transformers/all-MiniLM-L6-v2` (384D base dimension)
- Helpers: `embedding_utils.py` provides
  - `embed_text(text, dim)`: encode text and resize to `dim` (truncate/pad)
  - `embed_seed(index, dim)`: deterministic seed texts → stable embeddings
  - `embed_many(count, dim)`: batch generator for common test sizes
  - Internal cache keyed by `(text, dim)` to avoid repeated encodes

Automatic warmup
- `clients/python/tests/conftest.py` warms the cache once per pytest session.
- Default warmed dimensions: `32, 64, 128, 256, 384, 512, 1536`.

Controls (env vars)
- `PROXIMADB_TEST_EMBED_WARMUP`: set to `0`/`false` to disable warmup.
- `PROXIMADB_TEST_EMBED_DIMS`: comma list of dims to warm, e.g. `32,64,128,384`.

Setup
- Install dependencies before running tests:
  - `cd clients/python && pip install -e .[dev] && pip install sentence-transformers`
- Run tests:
  - `pytest -q clients/python/tests`

Notes
- Some compression tests intentionally keep sparse Gaussian noise to simulate data shapes. Embedding helpers are used for query and dense sets where appropriate.
- If CI runtime is a concern, pin a Hugging Face cache or narrow `PROXIMADB_TEST_EMBED_DIMS`.

CI cache (GitHub Actions example)
- Pin a cache for Hugging Face and sentence-transformers to avoid repeated downloads:

```yaml
name: python-tests
on: [push, pull_request]
jobs:
  tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: '3.11'
      - name: Cache HF models
        uses: actions/cache@v4
        with:
          path: |
            ~/.cache/huggingface
            ~/.cache/torch/sentence_transformers
          key: hf-cache-${{ runner.os }}-all-minilm-l6-v2-v1
          restore-keys: |
            hf-cache-${{ runner.os }}-
      - name: Install deps
        run: |
          cd clients/python
          pip install -e .[dev]
          pip install sentence-transformers
      - name: Run tests (warm cache with fewer dims)
        env:
          PROXIMADB_TEST_EMBED_DIMS: '64,128,384'
        run: |
          pytest -q clients/python/tests
```

Notes on cache locations and overrides
- Hugging Face hub: `~/.cache/huggingface` (override with `HF_HOME`)
- Sentence-transformers: `~/.cache/torch/sentence_transformers`
- You may set `HF_HOME` or `TRANSFORMERS_CACHE` to a writable path in your CI if needed.
