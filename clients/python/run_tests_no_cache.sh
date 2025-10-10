#!/bin/bash
# Run Python tests without any caching

# Prevent Python from writing bytecode
export PYTHONDONTWRITEBYTECODE=1

# Set PYTHONPATH
export PYTHONPATH=src

# Clean any existing cache
echo "🧹 Cleaning cache files..."
find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
find . -name "*.pyc" -delete 2>/dev/null || true
find . -name "*.pyo" -delete 2>/dev/null || true
rm -rf .pytest_cache 2>/dev/null || true

echo "🧪 Running tests without cache..."
python -m pytest "$@"
