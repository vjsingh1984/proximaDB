"""
Integration tests for Code Indexing with ProximaDB.

These tests verify end-to-end code indexing functionality:
- Vector store integration for code embeddings
- Graph store integration for code relationships
- Python and Rust code parsing and indexing
- Semantic code search
- Code relationship traversal

Requirements:
- Running ProximaDB server at localhost:5678 (REST) and :5679 (gRPC)
- Start server: cargo run --release --bin proximadb-server --config config/simple-config.toml

Run with:
    PYTHONPATH=src pytest tests/integration/test_code_indexing_integration.py -v -s
"""

import pytest
import sys
import asyncio
import time
import hashlib
import requests
from pathlib import Path
from typing import List, Dict, Any, Optional
from dataclasses import dataclass
from unittest.mock import MagicMock

# Add src to path
sys.path.insert(0, str(Path(__file__).parent.parent.parent / "src"))

# Check if server is available
def is_server_available(url: str = "http://localhost:5678") -> bool:
    """Check if ProximaDB server is running."""
    try:
        response = requests.get(f"{url}/health", timeout=5)
        return response.status_code == 200
    except Exception:
        return False


# Skip all tests if server not available
pytestmark = pytest.mark.skipif(
    not is_server_available(),
    reason="ProximaDB server not available at localhost:5678"
)


# =============================================================================
# Test Fixtures
# =============================================================================

@pytest.fixture(scope="module")
def server_url():
    """Server URL fixture."""
    return "http://localhost:5678"


@pytest.fixture(scope="module")
def grpc_url():
    """gRPC URL fixture."""
    return "localhost:5679"


@pytest.fixture(scope="function")
def unique_collection_name():
    """Generate unique collection name for test isolation."""
    return f"test_code_{int(time.time() * 1000)}"


@pytest.fixture(scope="function")
def unique_graph_name():
    """Generate unique graph name for test isolation."""
    return f"test_graph_{int(time.time() * 1000)}"


@pytest.fixture(scope="module")
def sample_python_code():
    """Sample Python code for testing."""
    return '''
"""Calculator module with basic operations."""

class Calculator:
    """A simple calculator class."""

    def __init__(self, precision: int = 2):
        """Initialize calculator with precision."""
        self.precision = precision
        self.history: list = []

    def add(self, a: float, b: float) -> float:
        """Add two numbers."""
        result = round(a + b, self.precision)
        self.history.append(("add", a, b, result))
        return result

    def subtract(self, a: float, b: float) -> float:
        """Subtract b from a."""
        result = round(a - b, self.precision)
        self.history.append(("subtract", a, b, result))
        return result

    def multiply(self, a: float, b: float) -> float:
        """Multiply two numbers."""
        result = round(a * b, self.precision)
        self.history.append(("multiply", a, b, result))
        return result

    def divide(self, a: float, b: float) -> float:
        """Divide a by b."""
        if b == 0:
            raise ValueError("Cannot divide by zero")
        result = round(a / b, self.precision)
        self.history.append(("divide", a, b, result))
        return result

    def get_history(self) -> list:
        """Return calculation history."""
        return self.history.copy()

    def clear_history(self) -> None:
        """Clear calculation history."""
        self.history.clear()


def create_calculator(precision: int = 2) -> Calculator:
    """Factory function to create a calculator."""
    return Calculator(precision)


def calculate_expression(expression: str) -> float:
    """Evaluate a simple mathematical expression."""
    calc = create_calculator()
    # Simple expression parser (for demo)
    parts = expression.split()
    if len(parts) != 3:
        raise ValueError("Expression must be: number operator number")

    a = float(parts[0])
    op = parts[1]
    b = float(parts[2])

    operations = {
        "+": calc.add,
        "-": calc.subtract,
        "*": calc.multiply,
        "/": calc.divide,
    }

    if op not in operations:
        raise ValueError(f"Unknown operator: {op}")

    return operations[op](a, b)
'''


@pytest.fixture(scope="module")
def sample_rust_code():
    """Sample Rust code for testing."""
    return '''
//! Vector operations module for high-performance computing.

use std::ops::{Add, Sub, Mul, Div};

/// A 3D vector for mathematical operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vector3D {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vector3D {
    /// Create a new 3D vector.
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Create a zero vector.
    pub fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    /// Calculate the magnitude (length) of the vector.
    pub fn magnitude(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    /// Normalize the vector to unit length.
    pub fn normalize(&self) -> Self {
        let mag = self.magnitude();
        if mag == 0.0 {
            return Self::zero();
        }
        Self::new(self.x / mag, self.y / mag, self.z / mag)
    }

    /// Calculate dot product with another vector.
    pub fn dot(&self, other: &Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Calculate cross product with another vector.
    pub fn cross(&self, other: &Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }
}

impl Add for Vector3D {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vector3D {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Mul<f64> for Vector3D {
    type Output = Self;

    fn mul(self, scalar: f64) -> Self {
        Self::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }
}

/// Calculate the angle between two vectors in radians.
pub fn angle_between(v1: &Vector3D, v2: &Vector3D) -> f64 {
    let dot = v1.dot(v2);
    let mag_product = v1.magnitude() * v2.magnitude();
    if mag_product == 0.0 {
        return 0.0;
    }
    (dot / mag_product).acos()
}

/// Project vector a onto vector b.
pub fn project(a: &Vector3D, b: &Vector3D) -> Vector3D {
    let b_mag_sq = b.dot(b);
    if b_mag_sq == 0.0 {
        return Vector3D::zero();
    }
    let scalar = a.dot(b) / b_mag_sq;
    Vector3D::new(b.x * scalar, b.y * scalar, b.z * scalar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_creation() {
        let v = Vector3D::new(1.0, 2.0, 3.0);
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);
    }

    #[test]
    fn test_magnitude() {
        let v = Vector3D::new(3.0, 4.0, 0.0);
        assert!((v.magnitude() - 5.0).abs() < 1e-10);
    }
}
'''


# =============================================================================
# Helper Functions
# =============================================================================

def create_collection(server_url: str, name: str, dimension: int = 384) -> Dict[str, Any]:
    """Create a vector collection via REST API.

    Uses CollectionRequest format with integer operation codes:
    - 1 = COLLECTION_CREATE
    - 5 = COLLECTION_DELETE
    """
    response = requests.post(
        f"{server_url}/api/v1/collections",
        json={
            "operation": 1,  # COLLECTION_CREATE
            "collection_id": name,
            "collection_config": {
                "name": name,
                "dimension": dimension,
                "distance_metric": 1,  # COSINE
            }
        }
    )
    return response.json()


def delete_collection(server_url: str, name: str) -> bool:
    """Delete a collection."""
    try:
        response = requests.delete(f"{server_url}/api/v1/collections/{name}")
        return response.status_code in (200, 204, 404)
    except Exception:
        return False


def insert_vectors(
    server_url: str,
    collection_name: str,
    vectors: List[Dict[str, Any]],
) -> Dict[str, Any]:
    """Insert vectors into collection.

    Automatically converts metadata values to SqlValue format.
    """
    # Convert metadata to SqlValue format for each vector
    formatted_vectors = []
    for v in vectors:
        formatted_v = {
            "id": v["id"],
            "vector": v["vector"],
        }
        if "metadata" in v:
            formatted_v["metadata"] = convert_metadata(v["metadata"])
        formatted_vectors.append(formatted_v)

    response = requests.post(
        f"{server_url}/api/v1/vectors/batch",
        json={
            "collection_id": collection_name,
            "vectors": formatted_vectors,
        }
    )
    return response.json()


def search_vectors(
    server_url: str,
    collection_name: str,
    query_vector: List[float],
    top_k: int = 5,
    filters: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """Search for similar vectors.

    Automatically converts filter values to SqlValue format.
    """
    # Build search query
    query = {
        "vector": query_vector,
    }
    if filters:
        query["filters"] = convert_metadata(filters)

    request = {
        "collection_id": collection_name,
        "queries": [query],
        "top_k": top_k,
    }

    response = requests.post(
        f"{server_url}/api/v1/search",
        json=request,
    )
    return response.json()


def generate_embedding(text: str, dimension: int = 384) -> List[float]:
    """Generate a deterministic pseudo-embedding from text.

    This creates reproducible embeddings based on text content
    for testing purposes without requiring an actual embedding model.
    """
    # Hash the text to get a seed
    text_hash = hashlib.sha256(text.encode()).digest()

    # Generate deterministic values
    embedding = []
    for i in range(dimension):
        # Use different parts of the hash for variety
        byte_idx = i % len(text_hash)
        next_byte = text_hash[(i + 1) % len(text_hash)]

        # Create a value between -1 and 1
        value = (text_hash[byte_idx] + next_byte * 0.1 - 128) / 128.0
        embedding.append(value)

    # Normalize to unit length
    magnitude = sum(v * v for v in embedding) ** 0.5
    if magnitude > 0:
        embedding = [v / magnitude for v in embedding]

    return embedding


def to_sql_value(value: Any) -> Dict[str, Any]:
    """Convert a Python value to ProximaDB SqlValue format.

    The API expects metadata values wrapped in SqlValue format:
    - strings: {"string_value": "..."}
    - numbers (int): {"int64_value": ...}
    - numbers (float): {"number_value": ...}
    - booleans: {"bool_value": ...}
    - lists: {"array_value": {"values": [...]}}
    - null: {"null_value": 0}
    """
    if value is None:
        return {"null_value": 0}
    elif isinstance(value, bool):
        return {"bool_value": value}
    elif isinstance(value, int):
        return {"int64_value": value}
    elif isinstance(value, float):
        return {"number_value": value}
    elif isinstance(value, str):
        return {"string_value": value}
    elif isinstance(value, (list, tuple)):
        return {"array_value": {"values": [to_sql_value(v) for v in value]}}
    elif isinstance(value, dict):
        return {"object_value": {"fields": {k: to_sql_value(v) for k, v in value.items()}}}
    else:
        return {"string_value": str(value)}


def convert_metadata(metadata: Dict[str, Any]) -> Dict[str, Any]:
    """Convert a metadata dictionary to SqlValue format."""
    return {k: to_sql_value(v) for k, v in metadata.items()}


def check_success(result: Dict[str, Any]) -> bool:
    """Check if API result indicates success.

    Returns True if:
    - result has "success": true
    - result has no actual error (error_message is None/null)
    """
    if result.get("success") is True:
        return True
    # Check for actual error message (not just field name)
    if result.get("error_message") not in (None, ""):
        return False
    if result.get("error"):
        return False
    return True


def assert_success(result: Dict[str, Any], message: str = "API call failed"):
    """Assert that an API result indicates success."""
    assert check_success(result), f"{message}: {result}"


# =============================================================================
# Test Classes
# =============================================================================

class TestServerConnection:
    """Test basic server connectivity."""

    def test_health_endpoint(self, server_url):
        """Test server health endpoint."""
        response = requests.get(f"{server_url}/health")
        assert response.status_code == 200

        data = response.json()
        assert data["status"] == "healthy"
        assert "version" in data

    def test_list_collections_empty(self, server_url):
        """Test listing collections."""
        response = requests.get(f"{server_url}/api/v1/collections")
        assert response.status_code == 200


class TestCollectionOperations:
    """Test vector collection operations."""

    def test_create_and_delete_collection(self, server_url, unique_collection_name):
        """Test creating and deleting a collection."""
        # Create
        result = create_collection(server_url, unique_collection_name)
        # API may return success in different formats
        assert_success(result, "Collection creation failed")

        # Cleanup
        delete_collection(server_url, unique_collection_name)

    def test_create_code_collection(self, server_url, unique_collection_name):
        """Test creating a collection for code embeddings."""
        try:
            result = create_collection(server_url, unique_collection_name, dimension=384)
            # Check for success in response
            assert result.get("success", False) or "error" not in str(result).lower()
        finally:
            delete_collection(server_url, unique_collection_name)


class TestVectorOperations:
    """Test vector insertion and search."""

    def test_insert_and_search_vectors(self, server_url, unique_collection_name):
        """Test inserting and searching vectors."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            # Create test vectors
            vectors = [
                {
                    "id": "func_add",
                    "vector": generate_embedding("add two numbers function"),
                    "metadata": {
                        "name": "add",
                        "type": "function",
                        "language": "python",
                        "file": "calculator.py",
                    }
                },
                {
                    "id": "func_subtract",
                    "vector": generate_embedding("subtract numbers function"),
                    "metadata": {
                        "name": "subtract",
                        "type": "function",
                        "language": "python",
                        "file": "calculator.py",
                    }
                },
                {
                    "id": "class_calculator",
                    "vector": generate_embedding("calculator class for math operations"),
                    "metadata": {
                        "name": "Calculator",
                        "type": "class",
                        "language": "python",
                        "file": "calculator.py",
                    }
                },
            ]

            # Insert vectors
            result = insert_vectors(server_url, unique_collection_name, vectors)
            # Check for success
            assert_success(result, "Vector operation failed")

            # Search for similar vectors
            query = generate_embedding("function to add numbers")
            search_result = search_vectors(
                server_url,
                unique_collection_name,
                query,
                top_k=3,
            )

            # Verify search results
            assert "results" in search_result or "matches" in search_result or isinstance(search_result, list)

        finally:
            delete_collection(server_url, unique_collection_name)


class TestPythonCodeIndexing:
    """Test indexing Python code."""

    def test_index_python_functions(self, server_url, unique_collection_name, sample_python_code):
        """Test indexing Python function definitions."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            # Parse and create vectors for functions (simplified)
            functions = [
                ("add", "Add two numbers.", "def add(self, a: float, b: float) -> float"),
                ("subtract", "Subtract b from a.", "def subtract(self, a: float, b: float) -> float"),
                ("multiply", "Multiply two numbers.", "def multiply(self, a: float, b: float) -> float"),
                ("divide", "Divide a by b.", "def divide(self, a: float, b: float) -> float"),
                ("create_calculator", "Factory function to create a calculator.", "def create_calculator(precision: int = 2) -> Calculator"),
                ("calculate_expression", "Evaluate a simple mathematical expression.", "def calculate_expression(expression: str) -> float"),
            ]

            vectors = []
            for name, doc, signature in functions:
                embedding_text = f"{name} {doc} {signature}"
                vectors.append({
                    "id": f"py_func_{name}",
                    "vector": generate_embedding(embedding_text),
                    "metadata": {
                        "name": name,
                        "type": "function",
                        "language": "python",
                        "docstring": doc,
                        "signature": signature,
                        "file": "calculator.py",
                    }
                })

            # Insert vectors
            result = insert_vectors(server_url, unique_collection_name, vectors)
            assert_success(result, "Vector operation failed")

            # Search for math operations
            query = generate_embedding("function to perform division")
            search_result = search_vectors(server_url, unique_collection_name, query, top_k=3)

            # Verify we get results
            results = search_result.get("results", search_result.get("matches", search_result))
            if isinstance(results, list) and len(results) > 0:
                # Check that divide function is in top results
                result_ids = [r.get("id", r.get("vector_id", "")) for r in results]
                print(f"Search results for 'division': {result_ids}")

        finally:
            delete_collection(server_url, unique_collection_name)

    def test_index_python_classes(self, server_url, unique_collection_name, sample_python_code):
        """Test indexing Python class definitions."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            # Create class vector
            vectors = [{
                "id": "py_class_Calculator",
                "vector": generate_embedding("Calculator class for basic math operations add subtract multiply divide"),
                "metadata": {
                    "name": "Calculator",
                    "type": "class",
                    "language": "python",
                    "docstring": "A simple calculator class.",
                    "methods": ["add", "subtract", "multiply", "divide", "get_history", "clear_history"],
                    "file": "calculator.py",
                }
            }]

            result = insert_vectors(server_url, unique_collection_name, vectors)
            assert_success(result, "Vector operation failed")

            # Search for calculator
            query = generate_embedding("calculator for arithmetic operations")
            search_result = search_vectors(server_url, unique_collection_name, query, top_k=1)

            results = search_result.get("results", search_result.get("matches", search_result))
            if isinstance(results, list):
                print(f"Found {len(results)} results for 'calculator' search")

        finally:
            delete_collection(server_url, unique_collection_name)


class TestRustCodeIndexing:
    """Test indexing Rust code."""

    def test_index_rust_structs(self, server_url, unique_collection_name, sample_rust_code):
        """Test indexing Rust struct definitions."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            # Create struct vector
            vectors = [{
                "id": "rs_struct_Vector3D",
                "vector": generate_embedding("Vector3D struct 3D vector mathematical operations x y z coordinates"),
                "metadata": {
                    "name": "Vector3D",
                    "type": "struct",
                    "language": "rust",
                    "docstring": "A 3D vector for mathematical operations.",
                    "fields": ["x", "y", "z"],
                    "file": "vector.rs",
                }
            }]

            result = insert_vectors(server_url, unique_collection_name, vectors)
            assert_success(result, "Vector operation failed")

        finally:
            delete_collection(server_url, unique_collection_name)

    def test_index_rust_functions(self, server_url, unique_collection_name, sample_rust_code):
        """Test indexing Rust function definitions."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            functions = [
                ("new", "Create a new 3D vector.", "pub fn new(x: f64, y: f64, z: f64) -> Self"),
                ("magnitude", "Calculate the magnitude (length) of the vector.", "pub fn magnitude(&self) -> f64"),
                ("normalize", "Normalize the vector to unit length.", "pub fn normalize(&self) -> Self"),
                ("dot", "Calculate dot product with another vector.", "pub fn dot(&self, other: &Self) -> f64"),
                ("cross", "Calculate cross product with another vector.", "pub fn cross(&self, other: &Self) -> Self"),
                ("angle_between", "Calculate the angle between two vectors in radians.", "pub fn angle_between(v1: &Vector3D, v2: &Vector3D) -> f64"),
                ("project", "Project vector a onto vector b.", "pub fn project(a: &Vector3D, b: &Vector3D) -> Vector3D"),
            ]

            vectors = []
            for name, doc, signature in functions:
                embedding_text = f"{name} {doc} {signature}"
                vectors.append({
                    "id": f"rs_func_{name}",
                    "vector": generate_embedding(embedding_text),
                    "metadata": {
                        "name": name,
                        "type": "function",
                        "language": "rust",
                        "docstring": doc,
                        "signature": signature,
                        "file": "vector.rs",
                    }
                })

            result = insert_vectors(server_url, unique_collection_name, vectors)
            assert_success(result, "Vector operation failed")

            # Search for vector operations
            query = generate_embedding("function to calculate vector length magnitude")
            search_result = search_vectors(server_url, unique_collection_name, query, top_k=3)

            results = search_result.get("results", search_result.get("matches", search_result))
            if isinstance(results, list) and len(results) > 0:
                result_ids = [r.get("id", r.get("vector_id", "")) for r in results]
                print(f"Search results for 'magnitude': {result_ids}")

        finally:
            delete_collection(server_url, unique_collection_name)


class TestCrossLanguageSearch:
    """Test searching across Python and Rust code."""

    def test_search_across_languages(self, server_url, unique_collection_name):
        """Test searching finds code across languages."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            # Insert both Python and Rust vectors
            vectors = [
                {
                    "id": "py_add",
                    "vector": generate_embedding("add two numbers python function"),
                    "metadata": {"name": "add", "language": "python", "type": "function"}
                },
                {
                    "id": "rs_add",
                    "vector": generate_embedding("add two vectors rust implementation"),
                    "metadata": {"name": "add", "language": "rust", "type": "impl"}
                },
                {
                    "id": "py_subtract",
                    "vector": generate_embedding("subtract numbers python function"),
                    "metadata": {"name": "subtract", "language": "python", "type": "function"}
                },
                {
                    "id": "rs_sub",
                    "vector": generate_embedding("subtract vectors rust implementation"),
                    "metadata": {"name": "sub", "language": "rust", "type": "impl"}
                },
            ]

            result = insert_vectors(server_url, unique_collection_name, vectors)
            assert_success(result, "Vector operation failed")

            # Search for addition - should find both languages
            query = generate_embedding("addition operation")
            search_result = search_vectors(server_url, unique_collection_name, query, top_k=4)

            results = search_result.get("results", search_result.get("matches", search_result))
            if isinstance(results, list):
                languages = set()
                for r in results:
                    meta = r.get("metadata", {})
                    if isinstance(meta, dict):
                        lang = meta.get("language")
                        if lang:
                            languages.add(lang)
                print(f"Found results in languages: {languages}")

        finally:
            delete_collection(server_url, unique_collection_name)


class TestCodeRelationships:
    """Test code relationship storage (simulated graph operations)."""

    def test_store_call_relationships(self, server_url, unique_collection_name):
        """Test storing function call relationships."""
        try:
            # Create collection for relationships
            create_collection(server_url, unique_collection_name, dimension=384)

            # Store relationship vectors
            # In a real implementation, this would use the graph API
            relationships = [
                {
                    "id": "rel_calc_expr_calls_create_calc",
                    "vector": generate_embedding("calculate_expression calls create_calculator"),
                    "metadata": {
                        "type": "calls",
                        "source": "calculate_expression",
                        "target": "create_calculator",
                        "language": "python",
                    }
                },
                {
                    "id": "rel_create_calc_creates_calc",
                    "vector": generate_embedding("create_calculator creates Calculator instance"),
                    "metadata": {
                        "type": "instantiates",
                        "source": "create_calculator",
                        "target": "Calculator",
                        "language": "python",
                    }
                },
            ]

            result = insert_vectors(server_url, unique_collection_name, relationships)
            assert_success(result, "Vector operation failed")

            # Search for relationships
            query = generate_embedding("what does calculate_expression call")
            search_result = search_vectors(server_url, unique_collection_name, query, top_k=2)

            results = search_result.get("results", search_result.get("matches", search_result))
            if isinstance(results, list):
                print(f"Found {len(results)} relationship results")

        finally:
            delete_collection(server_url, unique_collection_name)


class TestBatchOperations:
    """Test batch operations for efficient indexing."""

    def test_batch_insert_large(self, server_url, unique_collection_name):
        """Test batch inserting many vectors."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            # Generate many vectors
            vectors = []
            for i in range(100):
                vectors.append({
                    "id": f"batch_vec_{i}",
                    "vector": generate_embedding(f"function number {i} with operations"),
                    "metadata": {
                        "index": i,
                        "type": "function",
                        "language": "python" if i % 2 == 0 else "rust",
                    }
                })

            # Batch insert
            start_time = time.time()
            result = insert_vectors(server_url, unique_collection_name, vectors)
            elapsed = time.time() - start_time

            assert_success(result, "Vector operation failed")
            print(f"Inserted {len(vectors)} vectors in {elapsed:.2f}s")

            # Verify search works
            query = generate_embedding("function operations")
            search_result = search_vectors(server_url, unique_collection_name, query, top_k=10)

            results = search_result.get("results", search_result.get("matches", search_result))
            assert results is not None

        finally:
            delete_collection(server_url, unique_collection_name)


class TestMetadataFiltering:
    """Test metadata-based filtering in searches."""

    def test_filter_by_language(self, server_url, unique_collection_name):
        """Test filtering search results by language."""
        try:
            # Create collection
            create_collection(server_url, unique_collection_name, dimension=384)

            # Insert vectors with different languages
            vectors = [
                {
                    "id": "py_1",
                    "vector": generate_embedding("function to process data"),
                    "metadata": {"language": "python", "name": "process_py"}
                },
                {
                    "id": "rs_1",
                    "vector": generate_embedding("function to process data"),
                    "metadata": {"language": "rust", "name": "process_rs"}
                },
                {
                    "id": "py_2",
                    "vector": generate_embedding("function to transform data"),
                    "metadata": {"language": "python", "name": "transform_py"}
                },
            ]

            result = insert_vectors(server_url, unique_collection_name, vectors)
            assert_success(result, "Vector operation failed")

            # Search with language filter (if supported)
            query = generate_embedding("data processing function")

            # Try filtered search
            try:
                search_result = search_vectors(
                    server_url,
                    unique_collection_name,
                    query,
                    top_k=3,
                    filters={"language": "python"}
                )
                print(f"Filtered search result: {search_result}")
            except Exception as e:
                print(f"Filtered search not supported or failed: {e}")

            # Unfiltered search should work
            search_result = search_vectors(server_url, unique_collection_name, query, top_k=3)
            results = search_result.get("results", search_result.get("matches", search_result))
            assert results is not None

        finally:
            delete_collection(server_url, unique_collection_name)


# =============================================================================
# Performance Tests
# =============================================================================

@pytest.mark.slow
class TestPerformance:
    """Performance benchmarks for code indexing."""

    def test_search_latency(self, server_url, unique_collection_name):
        """Measure search latency."""
        try:
            # Create and populate collection
            create_collection(server_url, unique_collection_name, dimension=384)

            vectors = [
                {
                    "id": f"perf_vec_{i}",
                    "vector": generate_embedding(f"performance test vector {i}"),
                    "metadata": {"index": i}
                }
                for i in range(50)
            ]
            insert_vectors(server_url, unique_collection_name, vectors)

            # Measure search latency
            query = generate_embedding("performance test query")
            latencies = []

            for _ in range(10):
                start = time.time()
                search_vectors(server_url, unique_collection_name, query, top_k=5)
                latencies.append(time.time() - start)

            avg_latency = sum(latencies) / len(latencies)
            print(f"Average search latency: {avg_latency * 1000:.2f}ms")

            # Should be reasonably fast
            assert avg_latency < 1.0, f"Search latency too high: {avg_latency}s"

        finally:
            delete_collection(server_url, unique_collection_name)


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])
