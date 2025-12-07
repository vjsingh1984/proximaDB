//! PyO3 Python Bindings for ProximaDB Embedded Mode
//!
//! This module provides the Python interface for ProximaDB embedded mode.
//! It uses PyO3 for zero-copy NumPy array handling and efficient data transfer.

use pyo3::prelude::*;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::types::{PyDict, PyList};
use std::collections::HashMap;
use std::sync::Arc;

use super::{EmbeddedConfig, EmbeddedProximaDB, StorageLocationConfig};

/// Python wrapper for disk configuration
#[pyclass(name = "DiskConfig")]
#[derive(Clone)]
pub struct PyDiskConfig {
    /// Path to storage directory
    #[pyo3(get, set)]
    pub path: String,
    /// Weight for data distribution (higher = more data)
    #[pyo3(get, set)]
    pub weight: u32,
    /// Tags for storage tier identification
    #[pyo3(get, set)]
    pub tags: Vec<String>,
}

#[pymethods]
impl PyDiskConfig {
    /// Create a new disk configuration
    ///
    /// Args:
    ///     path: Path to the storage directory
    ///     weight: Weight for data distribution (default: 1)
    ///     tags: Optional list of tags (e.g., ["hot", "ssd"])
    #[new]
    #[pyo3(signature = (path, weight=1, tags=None))]
    fn new(path: String, weight: u32, tags: Option<Vec<String>>) -> Self {
        PyDiskConfig {
            path,
            weight,
            tags: tags.unwrap_or_default(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "DiskConfig(path='{}', weight={}, tags={:?})",
            self.path, self.weight, self.tags
        )
    }
}

impl From<PyDiskConfig> for StorageLocationConfig {
    fn from(config: PyDiskConfig) -> Self {
        StorageLocationConfig {
            path: config.path,
            weight: config.weight,
            tags: config.tags,
        }
    }
}

/// Python wrapper for search results
#[pyclass(name = "SearchResult")]
pub struct PySearchResult {
    /// Vector ID
    #[pyo3(get)]
    pub id: String,
    /// Similarity score
    #[pyo3(get)]
    pub score: f32,
    /// Associated metadata as dictionary
    metadata_map: HashMap<String, String>,
}

#[pymethods]
impl PySearchResult {
    /// Get metadata as a Python dictionary
    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new(py);
        for (k, v) in &self.metadata_map {
            dict.set_item(k, v)?;
        }
        Ok(dict.into())
    }

    fn __repr__(&self) -> String {
        format!(
            "SearchResult(id='{}', score={:.4}, metadata={{...}})",
            self.id, self.score
        )
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }
}

/// Python wrapper for collection information
#[pyclass(name = "CollectionInfo")]
pub struct PyCollectionInfo {
    /// Collection name
    #[pyo3(get)]
    pub name: String,
    /// Vector dimension
    #[pyo3(get)]
    pub dimension: u32,
    /// Number of vectors in the collection
    #[pyo3(get)]
    pub vector_count: u64,
    /// Storage engine type
    #[pyo3(get)]
    pub engine: String,
}

#[pymethods]
impl PyCollectionInfo {
    fn __repr__(&self) -> String {
        format!(
            "CollectionInfo(name='{}', dimension={}, vector_count={}, engine='{}')",
            self.name, self.dimension, self.vector_count, self.engine
        )
    }
}

/// Python wrapper for storage statistics
#[pyclass(name = "StorageStats")]
pub struct PyStorageStats {
    /// Total vectors across all collections
    #[pyo3(get)]
    pub total_vectors: u64,
    /// Total number of collections
    #[pyo3(get)]
    pub total_collections: u64,
    /// Disk usage in bytes
    #[pyo3(get)]
    pub disk_usage_bytes: u64,
    /// Cache hit rate (0.0 to 1.0)
    #[pyo3(get)]
    pub cache_hit_rate: f64,
}

#[pymethods]
impl PyStorageStats {
    /// Get disk usage in human-readable format
    fn disk_usage_human(&self) -> String {
        let bytes = self.disk_usage_bytes as f64;
        if bytes >= 1e12 {
            format!("{:.2} TB", bytes / 1e12)
        } else if bytes >= 1e9 {
            format!("{:.2} GB", bytes / 1e9)
        } else if bytes >= 1e6 {
            format!("{:.2} MB", bytes / 1e6)
        } else if bytes >= 1e3 {
            format!("{:.2} KB", bytes / 1e3)
        } else {
            format!("{} B", self.disk_usage_bytes)
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "StorageStats(vectors={}, collections={}, disk={}, cache_hit={:.1}%)",
            self.total_vectors,
            self.total_collections,
            self.disk_usage_human(),
            self.cache_hit_rate * 100.0
        )
    }
}

/// ProximaDB embedded database instance
///
/// This class provides a Python interface to ProximaDB's embedded mode,
/// allowing direct in-process access to the vector database without
/// network overhead.
///
/// Example:
///     ```python
///     from proximadb_embedded import ProximaDB, DiskConfig
///
///     # Multi-disk configuration
///     db = ProximaDB(
///         data_dirs=[
///             DiskConfig("/nvme/data", weight=2),  # Fast SSD
///             DiskConfig("/hdd/data", weight=1),   # Slower HDD
///         ],
///         metadata_dir="/nvme/metadata",
///         cache_size_mb=2048
///     )
///
///     # Create collection
///     db.create_collection("embeddings", dimension=768)
///
///     # Insert vectors (supports NumPy arrays)
///     import numpy as np
///     vectors = np.random.rand(1000, 768).astype(np.float32)
///     db.insert("embeddings", ids=[f"v{i}" for i in range(1000)], vectors=vectors)
///
///     # Search
///     results = db.search("embeddings", query=vectors[0], top_k=10)
///     ```
#[pyclass(name = "ProximaDB")]
pub struct PyProximaDB {
    inner: Arc<EmbeddedProximaDB>,
}

#[pymethods]
impl PyProximaDB {
    /// Create a new embedded ProximaDB instance
    ///
    /// Args:
    ///     data_dirs: List of DiskConfig for multi-disk storage, or a single path string
    ///     metadata_dir: Path to metadata storage (should be on fast disk)
    ///     cache_size_mb: Cache size in megabytes (default: 512)
    ///     default_engine: Default storage engine ("sst", "viper", "nova", etc.)
    ///     enable_wal: Enable write-ahead logging for durability (default: True)
    ///     wal_sync_mode: WAL sync mode - "immediate", "batch", or "async" (default: "batch")
    #[new]
    #[pyo3(signature = (
        data_dirs=None,
        metadata_dir=None,
        cache_size_mb=512,
        default_engine="sst",
        enable_wal=true,
        wal_sync_mode="batch"
    ))]
    fn new(
        data_dirs: Option<&PyAny>,
        metadata_dir: Option<String>,
        cache_size_mb: usize,
        default_engine: &str,
        enable_wal: bool,
        wal_sync_mode: &str,
    ) -> PyResult<Self> {
        // Parse data directories
        let storage_locations = if let Some(dirs) = data_dirs {
            if let Ok(path) = dirs.extract::<String>() {
                // Single path string
                vec![StorageLocationConfig::new(path)]
            } else if let Ok(configs) = dirs.extract::<Vec<PyDiskConfig>>() {
                // List of DiskConfig
                configs.into_iter().map(|c| c.into()).collect()
            } else if let Ok(paths) = dirs.extract::<Vec<String>>() {
                // List of path strings
                paths
                    .into_iter()
                    .map(StorageLocationConfig::new)
                    .collect()
            } else {
                return Err(PyValueError::new_err(
                    "data_dirs must be a path string, list of paths, or list of DiskConfig",
                ));
            }
        } else {
            vec![StorageLocationConfig::new("./data")]
        };

        // Determine metadata path
        let metadata_path = metadata_dir.unwrap_or_else(|| {
            let first_dir = &storage_locations[0].path;
            if first_dir.starts_with("file://") {
                format!("{}/metadata", first_dir)
            } else {
                format!("{}/metadata", first_dir)
            }
        });

        let config = EmbeddedConfig {
            storage_locations,
            metadata_path,
            cache_size_mb,
            default_engine: default_engine.to_string(),
            enable_wal,
            wal_sync_mode: wal_sync_mode.to_string(),
        };

        let db = EmbeddedProximaDB::new(config)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create database: {}", e)))?;

        Ok(PyProximaDB {
            inner: Arc::new(db),
        })
    }

    /// Create a new collection
    ///
    /// Args:
    ///     name: Collection name
    ///     dimension: Vector dimension
    ///     engine: Storage engine type (optional, uses default if not specified)
    ///
    /// Raises:
    ///     RuntimeError: If collection creation fails
    #[pyo3(signature = (name, dimension, engine=None))]
    fn create_collection(
        &self,
        name: &str,
        dimension: u32,
        engine: Option<&str>,
    ) -> PyResult<()> {
        self.inner
            .create_collection(name, dimension, engine)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create collection: {}", e)))
    }

    /// Delete a collection
    ///
    /// Args:
    ///     name: Collection name to delete
    ///
    /// Raises:
    ///     RuntimeError: If collection deletion fails
    fn delete_collection(&self, name: &str) -> PyResult<()> {
        self.inner
            .delete_collection(name)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to delete collection: {}", e)))
    }

    /// Get collection information
    ///
    /// Args:
    ///     name: Collection name
    ///
    /// Returns:
    ///     CollectionInfo or None if collection doesn't exist
    fn get_collection(&self, name: &str) -> PyResult<Option<PyCollectionInfo>> {
        self.inner
            .get_collection(name)
            .map(|opt| {
                opt.map(|info| PyCollectionInfo {
                    name: info.name,
                    dimension: info.dimension,
                    vector_count: info.vector_count,
                    engine: info.engine,
                })
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to get collection: {}", e)))
    }

    /// List all collections
    ///
    /// Returns:
    ///     List of CollectionInfo
    fn list_collections(&self) -> PyResult<Vec<PyCollectionInfo>> {
        self.inner
            .list_collections()
            .map(|collections| {
                collections
                    .into_iter()
                    .map(|info| PyCollectionInfo {
                        name: info.name,
                        dimension: info.dimension,
                        vector_count: info.vector_count,
                        engine: info.engine,
                    })
                    .collect()
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to list collections: {}", e)))
    }

    /// Insert vectors into a collection
    ///
    /// Args:
    ///     collection: Collection name
    ///     ids: List of vector IDs
    ///     vectors: 2D array of vectors (can be NumPy array or list of lists)
    ///     metadata: Optional list of metadata dictionaries
    ///
    /// Returns:
    ///     Number of vectors inserted
    ///
    /// Example:
    ///     ```python
    ///     import numpy as np
    ///     vectors = np.random.rand(100, 768).astype(np.float32)
    ///     ids = [f"vec_{i}" for i in range(100)]
    ///     metadata = [{"category": "A", "score": 0.9} for _ in range(100)]
    ///     count = db.insert("my_collection", ids, vectors, metadata)
    ///     ```
    #[pyo3(signature = (collection, ids, vectors, metadata=None))]
    fn insert(
        &self,
        collection: &str,
        ids: Vec<String>,
        vectors: &PyAny,
        metadata: Option<&PyList>,
    ) -> PyResult<usize> {
        // Convert vectors from Python to Rust
        let rust_vectors: Vec<Vec<f32>> = if vectors.hasattr("tolist")? {
            // NumPy array - call tolist() to convert
            vectors.call_method0("tolist")?.extract()?
        } else {
            // Already a Python list
            vectors.extract()?
        };

        // Convert metadata
        let rust_metadata: Option<Vec<HashMap<String, serde_json::Value>>> =
            if let Some(meta_list) = metadata {
                let mut result = Vec::with_capacity(meta_list.len());
                for item in meta_list.iter() {
                    let dict: &PyDict = item.downcast()?;
                    let mut map = HashMap::new();
                    for (k, v) in dict.iter() {
                        let key: String = k.extract()?;
                        let value = python_to_json(v)?;
                        map.insert(key, value);
                    }
                    result.push(map);
                }
                Some(result)
            } else {
                None
            };

        self.inner
            .insert(collection, ids, rust_vectors, rust_metadata)
            .map_err(|e| PyRuntimeError::new_err(format!("Insert failed: {}", e)))
    }

    /// Search for similar vectors
    ///
    /// Args:
    ///     collection: Collection name
    ///     query: Query vector (can be NumPy array or list)
    ///     top_k: Number of results to return (default: 10)
    ///     filter: Optional filter expression
    ///
    /// Returns:
    ///     List of SearchResult objects
    ///
    /// Example:
    ///     ```python
    ///     import numpy as np
    ///     query = np.random.rand(768).astype(np.float32)
    ///     results = db.search("my_collection", query, top_k=10)
    ///     for r in results:
    ///         print(f"{r.id}: {r.score}")
    ///     ```
    #[pyo3(signature = (collection, query, top_k=10, filter=None))]
    fn search(
        &self,
        collection: &str,
        query: &PyAny,
        top_k: usize,
        filter: Option<&str>,
    ) -> PyResult<Vec<PySearchResult>> {
        // Convert query vector
        let query_vec: Vec<f32> = if query.hasattr("tolist")? {
            query.call_method0("tolist")?.extract()?
        } else {
            query.extract()?
        };

        self.inner
            .search(collection, query_vec, top_k, filter)
            .map(|results| {
                results
                    .into_iter()
                    .map(|r| PySearchResult {
                        id: r.id,
                        score: r.score,
                        metadata_map: r.metadata,
                    })
                    .collect()
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Search failed: {}", e)))
    }

    /// Flush all pending writes to disk
    ///
    /// This ensures all data is persisted to disk. Called automatically
    /// when the database is closed.
    fn flush(&self) -> PyResult<()> {
        self.inner
            .flush()
            .map_err(|e| PyRuntimeError::new_err(format!("Flush failed: {}", e)))
    }

    /// Get storage statistics
    ///
    /// Returns:
    ///     StorageStats with information about disk usage, vector counts, etc.
    fn stats(&self) -> PyResult<PyStorageStats> {
        self.inner
            .stats()
            .map(|s| PyStorageStats {
                total_vectors: s.total_vectors,
                total_collections: s.total_collections,
                disk_usage_bytes: s.disk_usage_bytes,
                cache_hit_rate: s.cache_hit_rate,
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to get stats: {}", e)))
    }

    /// Context manager entry
    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Context manager exit - ensures flush on exit
    fn __exit__(
        &self,
        _exc_type: Option<&PyAny>,
        _exc_val: Option<&PyAny>,
        _exc_tb: Option<&PyAny>,
    ) -> PyResult<bool> {
        self.flush()?;
        Ok(false) // Don't suppress exceptions
    }

    fn __repr__(&self) -> PyResult<String> {
        let stats = self.stats()?;
        Ok(format!(
            "ProximaDB(collections={}, vectors={}, disk={})",
            stats.total_collections,
            stats.total_vectors,
            stats.disk_usage_human()
        ))
    }
}

/// Convert Python value to serde_json::Value
fn python_to_json(value: &PyAny) -> PyResult<serde_json::Value> {
    if value.is_none() {
        Ok(serde_json::Value::Null)
    } else if let Ok(b) = value.extract::<bool>() {
        Ok(serde_json::Value::Bool(b))
    } else if let Ok(i) = value.extract::<i64>() {
        Ok(serde_json::Value::Number(i.into()))
    } else if let Ok(f) = value.extract::<f64>() {
        Ok(serde_json::json!(f))
    } else if let Ok(s) = value.extract::<String>() {
        Ok(serde_json::Value::String(s))
    } else if let Ok(list) = value.downcast::<PyList>() {
        let arr: Vec<serde_json::Value> = list
            .iter()
            .map(|item| python_to_json(item))
            .collect::<PyResult<_>>()?;
        Ok(serde_json::Value::Array(arr))
    } else if let Ok(dict) = value.downcast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            map.insert(key, python_to_json(v)?);
        }
        Ok(serde_json::Value::Object(map))
    } else {
        // Fallback: convert to string representation
        Ok(serde_json::Value::String(value.str()?.to_string()))
    }
}

/// Python module definition
#[pymodule]
fn proximadb_embedded(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyProximaDB>()?;
    m.add_class::<PyDiskConfig>()?;
    m.add_class::<PySearchResult>()?;
    m.add_class::<PyCollectionInfo>()?;
    m.add_class::<PyStorageStats>()?;

    // Add version info
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}
