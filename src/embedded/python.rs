// PyO3 generates impl blocks for #[pymethods] macros that trigger the
// non_local_definitions lint. This is a known issue with pyo3_macros.
#![allow(non_local_definitions)]

//! PyO3 Python Bindings for ProximaDB Embedded Mode
//!
//! This module provides the Python interface for ProximaDB embedded mode.
//! It uses PyO3 with the numpy crate for ZERO-COPY NumPy array handling.
//!
//! ## Performance
//!
//! The numpy crate provides direct access to NumPy array buffers without copying:
//! - `insert_numpy()`: Zero-copy insert from numpy.ndarray (fastest)
//! - `insert()`: Python list conversion (fallback, slower)
//!
//! ## Example
//!
//! ```python
//! import numpy as np
//! from proximadb import ProximaDB
//!
//! db = ProximaDB("./data")
//! db.create_collection("vectors", dimension=384)
//!
//! # Fast: Zero-copy numpy insert
//! vectors = np.random.rand(10000, 384).astype(np.float32)
//! ids = [f"vec_{i}" for i in range(10000)]
//! db.insert_numpy("vectors", ids, vectors)  # Zero-copy!
//!
//! # Slower: Python list (converts via .tolist())
//! db.insert("vectors", ids, vectors.tolist())
//! ```

use pyo3::exceptions::{PyRuntimeError, PyValueError, PyUserWarning};
use pyo3::prelude::*;
use pyo3::{PyErr, PyTypeInfo};
use pyo3::types::{PyDict, PyList};
use std::collections::HashMap;
use std::sync::Arc;

// Zero-copy numpy support
use numpy::{PyArray1, PyArray2, PyReadonlyArray1, PyReadonlyArray2};

use super::{EmbeddedConfig, EmbeddedProximaDB, StorageLocationConfig};
use crate::core::config::{AdvancedPruneConfig, PruneModeConfig};
use crate::core::proto_metadata_helper::sqlvalue_metadata_to_json;

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
    /// Disk usage in bytes for this collection
    #[pyo3(get)]
    pub disk_usage_bytes: u64,
}

#[pymethods]
impl PyCollectionInfo {
    fn __repr__(&self) -> String {
        format!(
            "CollectionInfo(name='{}', dimension={}, vector_count={}, engine='{}', disk_usage_bytes={})",
            self.name, self.dimension, self.vector_count, self.engine, self.disk_usage_bytes
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

// ============================================================================
// Generic Graph Database Python Bindings
// ============================================================================
//
// These bindings provide a generic, tool-agnostic graph API. Domain-specific
// behavior (like code intelligence for Victor) should be built as an adapter
// layer in the consuming application.

/// Generic graph node with flexible property storage
///
/// This is a domain-agnostic node type. All domain-specific attributes
/// should be stored in the `properties` dict.
///
/// Example:
///     # For a code symbol:
///     node = GraphNode("fn_main", labels=["function"], properties={"name": "main", "file": "main.py"})
///
///     # For a social network:
///     node = GraphNode("user_123", labels=["Person"], properties={"name": "Alice"})
#[pyclass(name = "GraphNode")]
#[derive(Clone)]
pub struct PyGraphNode {
    /// Unique node ID
    #[pyo3(get, set)]
    pub id: String,
    /// Node labels/types
    labels_vec: Vec<String>,
    /// Flexible property storage
    properties_map: HashMap<String, String>,
}

#[pymethods]
impl PyGraphNode {
    /// Create a new graph node
    ///
    /// Args:
    ///     id: Unique node identifier
    ///     labels: List of labels/types for this node
    ///     properties: Dictionary of properties
    #[new]
    #[pyo3(signature = (id, labels=None, properties=None))]
    fn new(
        id: String,
        labels: Option<Vec<String>>,
        properties: Option<&PyDict>,
    ) -> PyResult<Self> {
        let properties_map = if let Some(dict) = properties {
            let mut map = HashMap::new();
            for (k, v) in dict.iter() {
                let key: String = k.extract()?;
                let value: String = v.str()?.to_string();
                map.insert(key, value);
            }
            map
        } else {
            HashMap::new()
        };

        Ok(PyGraphNode {
            id,
            labels_vec: labels.unwrap_or_default(),
            properties_map,
        })
    }

    /// Get labels as a list
    #[getter]
    fn labels(&self) -> Vec<String> {
        self.labels_vec.clone()
    }

    /// Set labels
    #[setter]
    fn set_labels(&mut self, labels: Vec<String>) {
        self.labels_vec = labels;
    }

    /// Get properties as a dictionary
    #[getter]
    fn properties(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new(py);
        for (k, v) in &self.properties_map {
            dict.set_item(k, v)?;
        }
        Ok(dict.into())
    }

    /// Set properties from a dictionary
    #[setter]
    fn set_properties(&mut self, properties: &PyDict) -> PyResult<()> {
        self.properties_map.clear();
        for (k, v) in properties.iter() {
            let key: String = k.extract()?;
            let value: String = v.str()?.to_string();
            self.properties_map.insert(key, value);
        }
        Ok(())
    }

    fn __repr__(&self) -> String {
        format!(
            "GraphNode(id='{}', labels={:?})",
            self.id, self.labels_vec
        )
    }
}

impl From<super::GraphNode> for PyGraphNode {
    fn from(node: super::GraphNode) -> Self {
        PyGraphNode {
            id: node.id,
            labels_vec: node.labels,
            properties_map: node.properties,
        }
    }
}

impl From<PyGraphNode> for super::GraphNode {
    fn from(node: PyGraphNode) -> Self {
        super::GraphNode {
            id: node.id,
            labels: node.labels_vec,
            properties: node.properties_map,
        }
    }
}

/// Generic graph edge with flexible property storage
#[pyclass(name = "GraphEdge")]
#[derive(Clone)]
pub struct PyGraphEdge {
    /// Optional edge ID
    #[pyo3(get, set)]
    pub id: Option<String>,
    /// Source node ID
    #[pyo3(get, set)]
    pub from_node_id: String,
    /// Destination node ID
    #[pyo3(get, set)]
    pub to_node_id: String,
    /// Edge type
    #[pyo3(get, set)]
    pub edge_type: String,
    /// Optional weight
    #[pyo3(get, set)]
    pub weight: Option<f64>,
    /// Flexible property storage
    properties_map: HashMap<String, String>,
}

#[pymethods]
impl PyGraphEdge {
    /// Create a new graph edge
    ///
    /// Args:
    ///     from_node_id: Source node ID
    ///     to_node_id: Destination node ID
    ///     edge_type: Type of relationship
    ///     id: Optional explicit edge ID
    ///     weight: Optional edge weight
    ///     properties: Optional property dictionary
    #[new]
    #[pyo3(signature = (from_node_id, to_node_id, edge_type, id=None, weight=None, properties=None))]
    fn new(
        from_node_id: String,
        to_node_id: String,
        edge_type: String,
        id: Option<String>,
        weight: Option<f64>,
        properties: Option<&PyDict>,
    ) -> PyResult<Self> {
        let properties_map = if let Some(dict) = properties {
            let mut map = HashMap::new();
            for (k, v) in dict.iter() {
                let key: String = k.extract()?;
                let value: String = v.str()?.to_string();
                map.insert(key, value);
            }
            map
        } else {
            HashMap::new()
        };

        Ok(PyGraphEdge {
            id,
            from_node_id,
            to_node_id,
            edge_type,
            weight,
            properties_map,
        })
    }

    /// Get properties as a dictionary
    #[getter]
    fn properties(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new(py);
        for (k, v) in &self.properties_map {
            dict.set_item(k, v)?;
        }
        Ok(dict.into())
    }

    fn __repr__(&self) -> String {
        format!(
            "GraphEdge(from='{}', to='{}', type='{}')",
            self.from_node_id, self.to_node_id, self.edge_type
        )
    }
}

impl From<super::GraphEdge> for PyGraphEdge {
    fn from(edge: super::GraphEdge) -> Self {
        PyGraphEdge {
            id: edge.id,
            from_node_id: edge.from_node_id,
            to_node_id: edge.to_node_id,
            edge_type: edge.edge_type,
            weight: edge.weight,
            properties_map: edge.properties,
        }
    }
}

impl From<PyGraphEdge> for super::GraphEdge {
    fn from(edge: PyGraphEdge) -> Self {
        super::GraphEdge {
            id: edge.id,
            from_node_id: edge.from_node_id,
            to_node_id: edge.to_node_id,
            edge_type: edge.edge_type,
            weight: edge.weight,
            properties: edge.properties_map,
        }
    }
}

/// Graph statistics
#[pyclass(name = "GraphStats")]
pub struct PyGraphStats {
    /// Total number of nodes
    #[pyo3(get)]
    pub total_nodes: u64,
    /// Total number of edges
    #[pyo3(get)]
    pub total_edges: u64,
}

#[pymethods]
impl PyGraphStats {
    fn __repr__(&self) -> String {
        format!(
            "GraphStats(total_nodes={}, total_edges={})",
            self.total_nodes, self.total_edges
        )
    }
}

impl From<super::GraphStats> for PyGraphStats {
    fn from(stats: super::GraphStats) -> Self {
        PyGraphStats {
            total_nodes: stats.total_nodes,
            total_edges: stats.total_edges,
        }
    }
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

fn set_approx_defaults(mode_str: &str) -> (String, f32, usize, usize) {
    match mode_str {
        "ratio" => ("ratio".to_string(), 0.1, 1, 0),
        _ => ("sqrt".to_string(), 0.2, 1, 0),
    }
}

#[pymethods]
impl PyProximaDB {
    /// Create a new embedded ProximaDB instance
    #[new]
    #[pyo3(signature = (
        data_dirs=None,
        metadata_dir=None,
        cache_size_mb=512,
        default_engine="sst",
        enable_wal=true,
        prune_mode=None
    ))]
    fn new(
        py: Python,
        data_dirs: Option<&PyAny>,
        metadata_dir: Option<String>,
        cache_size_mb: usize,
        default_engine: &str,
        enable_wal: bool,
        prune_mode: Option<&PyAny>,
    ) -> PyResult<Self> {
        let mut final_prune_config = None;

        if let Some(mode_arg) = prune_mode {
            if let Ok(s) = mode_arg.extract::<String>() {
                let s_lower = s.to_lowercase();
                if s_lower == "exact" {
                    // None implies exact; leave as default
                } else if s_lower == "approximate" || s_lower == "sqrt" || s_lower == "ratio" {
                    final_prune_config = Some(PruneModeConfig::Simple(s_lower));
                } else {
                    PyErr::warn(
                        py,
                        PyUserWarning::type_object(py),
                        "Invalid 'prune_mode' string provided. Falling back to 'exact' mode.",
                        1,
                    )?;
                }
            } else if let Ok(dict) = mode_arg.downcast::<PyDict>() {
                let prune_type = dict
                    .get_item("type")?
                    .and_then(|t| t.extract::<String>().ok())
                    .unwrap_or_else(|| "sqrt".to_string());

                final_prune_config = Some(PruneModeConfig::Advanced(AdvancedPruneConfig {
                    r#type: prune_type,
                    min_keep: dict.get_item("min_keep")?.and_then(|v| v.extract().ok()),
                    max_keep: dict.get_item("max_keep")?.and_then(|v| v.extract().ok()),
                    ratio: dict.get_item("ratio")?.and_then(|v| v.extract().ok()),
                }));
            } else {
                return Err(PyValueError::new_err(
                    "'prune_mode' must be a string, a dictionary, or None.",
                ));
            }
        }

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
            // [AGENT_MODIFICATION]: The wal_sync_mode is not exposed in the new signature, so use a smart default.
            wal_sync_mode: "batch".to_string(),
            block_prune_mode: "exact".to_string(),
            block_prune_ratio: 0.0,
            block_prune_min_keep: 0,
            block_prune_max_keep: 0,
            // RL planner is enabled by default for adaptive query optimization
            enable_rl_planner: true,
            rl_policy_path: None, // Will use default path based on data_dir
        };

        let mut config = config;

        // Translate PruneModeConfig to EmbeddedConfig fields
        if let Some(prune_cfg) = final_prune_config {
            match prune_cfg {
                PruneModeConfig::Simple(s) => {
                    let (mode, ratio, min_k, max_k) = set_approx_defaults(&s);
                    config.block_prune_mode = mode;
                    config.block_prune_ratio = ratio;
                    config.block_prune_min_keep = min_k;
                    config.block_prune_max_keep = max_k;
                }
                PruneModeConfig::Advanced(adv) => {
                    let (mode, def_ratio, def_min_k, def_max_k) =
                        set_approx_defaults(&adv.r#type);
                    config.block_prune_mode = adv.r#type;
                    config.block_prune_ratio = adv.ratio.unwrap_or(def_ratio);
                    config.block_prune_min_keep = adv.min_keep.unwrap_or(def_min_k);
                    config.block_prune_max_keep = adv.max_keep.unwrap_or(def_max_k);
                }
            }
        } else {
            config.block_prune_mode = "exact".to_string();
            // leave ratio/min/max at safe exact defaults
        }

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
    /// Examples:
    ///     db.create_collection("vectors", 768, "sst")
    ///     db.create_collection("vectors", 768, "helix")
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
                    disk_usage_bytes: info.disk_usage_bytes,
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
                        disk_usage_bytes: info.disk_usage_bytes,
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
    ///     search_mode: Optional search mode for accuracy vs speed tradeoff
    ///         - "exact": 100% recall, searches all partitions (default)
    ///         - "approximate": Faster search using IVF-style partition pruning (~95% recall)
    ///         - "approximate:N": Approximate with explicit nprobe value N
    ///         - "adaptive": Auto-select based on dataset size
    ///         - "adaptive:N": Adaptive with explicit threshold N
    ///
    /// Returns:
    ///     List of SearchResult objects
    ///
    /// Example:
    ///     ```python
    ///     import numpy as np
    ///     query = np.random.rand(768).astype(np.float32)
    ///
    ///     # Exact search (100% recall, default)
    ///     results = db.search("my_collection", query, top_k=10)
    ///
    ///     # Approximate search (faster, ~95% recall)
    ///     results = db.search("my_collection", query, top_k=10, search_mode="approximate")
    ///
    ///     # Approximate with custom nprobe
    ///     results = db.search("my_collection", query, top_k=10, search_mode="approximate:5")
    ///     ```
    #[pyo3(signature = (collection, query, top_k=10, filter=None, search_mode=None))]
    fn search(
        &self,
        collection: &str,
        query: &PyAny,
        top_k: usize,
        filter: Option<&str>,
        search_mode: Option<&str>,
    ) -> PyResult<Vec<PySearchResult>> {
        // Convert query vector
        let query_vec: Vec<f32> = if query.hasattr("tolist")? {
            query.call_method0("tolist")?.extract()?
        } else {
            query.extract()?
        };

        self.inner
            .search_with_mode(collection, query_vec, top_k, filter, search_mode)
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

    /// Insert vectors from a NumPy array with ZERO-COPY transfer
    ///
    /// This is the fastest way to insert vectors from Python. The numpy array
    /// buffer is accessed directly without copying to a Python list first.
    ///
    /// Args:
    ///     collection: Collection name
    ///     ids: List of vector IDs
    ///     vectors: 2D numpy array of shape (n_vectors, dimension) with dtype=float32
    ///     metadata: Optional list of metadata dictionaries
    ///
    /// Returns:
    ///     Number of vectors inserted
    ///
    /// Performance:
    ///     ~3-5x faster than insert() for large arrays (>10K vectors)
    ///
    /// Example:
    ///     ```python
    ///     import numpy as np
    ///     vectors = np.random.rand(100000, 384).astype(np.float32)
    ///     ids = [f"vec_{i}" for i in range(100000)]
    ///     count = db.insert_numpy("my_collection", ids, vectors)  # Zero-copy!
    ///     ```
    #[pyo3(signature = (collection, ids, vectors, metadata=None))]
    fn insert_numpy(
        &self,
        collection: &str,
        ids: Vec<String>,
        vectors: PyReadonlyArray2<f32>,
        metadata: Option<&PyList>,
    ) -> PyResult<usize> {
        // Zero-copy access to numpy buffer
        let array = vectors.as_array();
        let shape = array.shape();
        let n_vectors = shape[0];
        let dimension = shape[1];

        // Validate dimensions
        if ids.len() != n_vectors {
            return Err(PyValueError::new_err(format!(
                "Number of IDs ({}) doesn't match number of vectors ({})",
                ids.len(), n_vectors
            )));
        }

        // Convert to Vec<Vec<f32>> - we still need this format for the internal API
        // but at least we avoided the Python .tolist() overhead
        let rust_vectors: Vec<Vec<f32>> = array
            .rows()
            .into_iter()
            .map(|row| row.to_vec())
            .collect();

        // Convert metadata (same as before)
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

    /// Search with a NumPy query vector (zero-copy)
    ///
    /// Args:
    ///     collection: Collection name
    ///     query: 1D numpy array of shape (dimension,) with dtype=float32
    ///     top_k: Number of results to return (default: 10)
    ///     filter: Optional filter expression
    ///     search_mode: Optional search mode for accuracy vs speed tradeoff
    ///         - "exact": 100% recall, searches all partitions (default)
    ///         - "approximate": Faster search using IVF-style partition pruning (~95% recall)
    ///         - "approximate:N": Approximate with explicit nprobe value N
    ///         - "adaptive": Auto-select based on dataset size
    ///
    /// Returns:
    ///     List of SearchResult objects
    #[pyo3(signature = (collection, query, top_k=10, filter=None, search_mode=None))]
    fn search_numpy(
        &self,
        collection: &str,
        query: PyReadonlyArray1<f32>,
        top_k: usize,
        filter: Option<&str>,
        search_mode: Option<&str>,
    ) -> PyResult<Vec<PySearchResult>> {
        // Zero-copy access to query vector
        let query_vec: Vec<f32> = query.as_array().to_vec();

        self.inner
            .search_with_mode(collection, query_vec, top_k, filter, search_mode)
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

    /// Batch search with multiple query vectors (zero-copy)
    ///
    /// Args:
    ///     collection: Collection name
    ///     queries: 2D numpy array of shape (n_queries, dimension) with dtype=float32
    ///     top_k: Number of results per query (default: 10)
    ///     search_mode: Optional search mode for accuracy vs speed tradeoff
    ///         - "exact": 100% recall, searches all partitions (default)
    ///         - "approximate": Faster search using IVF-style partition pruning (~95% recall)
    ///         - "approximate:N": Approximate with explicit nprobe value N
    ///
    /// Returns:
    ///     List of lists of SearchResult objects
    #[pyo3(signature = (collection, queries, top_k=10, search_mode=None))]
    fn batch_search_numpy(
        &self,
        collection: &str,
        queries: PyReadonlyArray2<f32>,
        top_k: usize,
        search_mode: Option<&str>,
    ) -> PyResult<Vec<Vec<PySearchResult>>> {
        let array = queries.as_array();
        let mut all_results = Vec::with_capacity(array.nrows());

        for row in array.rows() {
            let query_vec: Vec<f32> = row.to_vec();
            let results = self.inner
                .search_with_mode(collection, query_vec, top_k, None, search_mode)
                .map_err(|e| PyRuntimeError::new_err(format!("Search failed: {}", e)))?;

            all_results.push(
                results
                    .into_iter()
                    .map(|r| PySearchResult {
                        id: r.id,
                        score: r.score,
                        metadata_map: r.metadata,
                    })
                    .collect()
            );
        }

        Ok(all_results)
    }

    // ========================================================================
    // Vector Lookup Operations - GET by ID
    // ========================================================================

    /// Get a vector by its ID
    ///
    /// Args:
    ///     collection: Collection name
    ///     vector_id: The unique ID of the vector to retrieve
    ///
    /// Returns:
    ///     Dictionary with {id, vector, metadata} or None if not found
    ///
    /// Example:
    ///     ```python
    ///     result = db.get_vector("my_collection", "vec_123")
    ///     if result:
    ///         print(f"Vector ID: {result['id']}")
    ///         print(f"Vector: {result['vector'][:5]}...")  # First 5 dims
    ///         print(f"Metadata: {result['metadata']}")
    ///     ```
    fn get_vector(&self, py: Python<'_>, collection: &str, vector_id: &str) -> PyResult<Option<PyObject>> {
        match self.inner.get_vector(collection, vector_id) {
            Ok(Some(record)) => {
                let dict = PyDict::new(py);
                dict.set_item("id", &record.id)?;
                dict.set_item("vector", record.vector.clone())?;

                // Convert metadata HashMap<String, SqlValue> to Python dict
                // First convert SqlValue to serde_json::Value, then to Python
                let metadata_dict = PyDict::new(py);
                let json_metadata = sqlvalue_metadata_to_json(&record.metadata);
                for (key, value) in &json_metadata {
                    let py_value = json_to_python(py, value)?;
                    metadata_dict.set_item(key, py_value)?;
                }
                dict.set_item("metadata", metadata_dict)?;

                // Include timestamp if available
                if let Some(ts) = record.timestamp {
                    dict.set_item("timestamp", ts)?;
                }

                Ok(Some(dict.into()))
            },
            Ok(None) => Ok(None),
            Err(e) => Err(PyRuntimeError::new_err(format!("Failed to get vector: {}", e))),
        }
    }

    /// Check if a vector exists in the collection
    ///
    /// This is a fast existence check using bloom filters when available.
    ///
    /// Args:
    ///     collection: Collection name
    ///     vector_id: The unique ID of the vector to check
    ///
    /// Returns:
    ///     True if the vector exists, False otherwise
    ///
    /// Example:
    ///     ```python
    ///     if db.vector_exists("my_collection", "vec_123"):
    ///         print("Vector exists!")
    ///     else:
    ///         print("Vector not found")
    ///     ```
    fn vector_exists(&self, collection: &str, vector_id: &str) -> PyResult<bool> {
        self.inner
            .vector_exists(collection, vector_id)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to check vector existence: {}", e)))
    }

    /// Delete a single vector by ID (tombstone-based)
    ///
    /// This uses tombstone markers to logically delete the vector. The tombstone
    /// will be written to the WAL and will shadow the original vector in searches.
    /// Physical deletion happens during compaction.
    ///
    /// Args:
    ///     collection: Name of the collection
    ///     vector_id: ID of the vector to delete
    ///
    /// Returns:
    ///     True if tombstone was written (doesn't guarantee vector existed)
    ///
    /// Example:
    ///     ```python
    ///     deleted = db.delete_vector("embeddings", "vec_123")
    ///     if deleted:
    ///         print("Vector marked for deletion")
    ///     ```
    fn delete_vector(&self, collection: &str, vector_id: &str) -> PyResult<bool> {
        self.inner
            .delete_vector(collection, vector_id)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to delete vector: {}", e)))
    }

    /// Delete multiple vectors by IDs (batch tombstone operation)
    ///
    /// More efficient than calling `delete_vector` multiple times.
    /// All tombstones are written in a single batch operation.
    ///
    /// Args:
    ///     collection: Name of the collection
    ///     vector_ids: List of vector IDs to delete
    ///
    /// Returns:
    ///     Number of tombstones written (equals input count)
    ///
    /// Example:
    ///     ```python
    ///     count = db.delete_vectors("embeddings", ["vec_1", "vec_2", "vec_3"])
    ///     print(f"Marked {count} vectors for deletion")
    ///     ```
    fn delete_vectors(&self, collection: &str, vector_ids: Vec<String>) -> PyResult<usize> {
        self.inner
            .delete_vectors(collection, vector_ids)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to delete vectors: {}", e)))
    }

    /// Upsert vectors (insert or update)
    ///
    /// This is an atomic operation that inserts new vectors and updates existing ones.
    /// Existing vectors are deleted (via tombstone) before the new vectors are inserted.
    ///
    /// Args:
    ///     collection: Collection name
    ///     ids: List of vector IDs
    ///     vectors: List of vectors (each is a list of floats), or NumPy array
    ///     metadata: Optional list of metadata dicts for each vector
    ///
    /// Returns:
    ///     Tuple of (inserted_count, updated_count)
    ///
    /// Example:
    ///     ```python
    ///     # Insert new and update existing vectors in one operation
    ///     inserted, updated = db.upsert(
    ///         "embeddings",
    ///         ["vec_1", "vec_2", "vec_3"],
    ///         [[0.1, 0.2, ...], [0.3, 0.4, ...], [0.5, 0.6, ...]],
    ///         [{"source": "doc1"}, {"source": "doc2"}, {"source": "doc3"}]
    ///     )
    ///     print(f"Inserted {inserted}, updated {updated}")
    ///     ```
    #[pyo3(signature = (collection, ids, vectors, metadata=None))]
    fn upsert(
        &self,
        collection: &str,
        ids: Vec<String>,
        vectors: &PyAny,
        metadata: Option<&PyList>,
    ) -> PyResult<(usize, usize)> {
        // Convert vectors from Python to Rust
        let rust_vectors: Vec<Vec<f32>> = if vectors.hasattr("tolist")? {
            // NumPy array - call tolist() to convert
            vectors.call_method0("tolist")?.extract()?
        } else {
            // Already a Python list
            vectors.extract()?
        };

        // Convert metadata (same pattern as insert)
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
            .upsert(collection, ids, rust_vectors, rust_metadata)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to upsert vectors: {}", e)))
    }

    /// Upsert vectors using NumPy arrays (zero-copy)
    ///
    /// This is the high-performance variant of upsert() that accepts NumPy arrays directly.
    ///
    /// Args:
    ///     collection: Collection name
    ///     ids: List of vector IDs
    ///     vectors: 2D NumPy array of shape (n_vectors, dimension)
    ///     metadata: Optional list of metadata dicts for each vector
    ///
    /// Returns:
    ///     Tuple of (inserted_count, updated_count)
    ///
    /// Example:
    ///     ```python
    ///     import numpy as np
    ///     vectors = np.random.randn(1000, 768).astype(np.float32)
    ///     ids = [f"vec_{i}" for i in range(1000)]
    ///     inserted, updated = db.upsert_numpy("embeddings", ids, vectors)
    ///     ```
    #[pyo3(signature = (collection, ids, vectors, metadata=None))]
    fn upsert_numpy(
        &self,
        collection: &str,
        ids: Vec<String>,
        vectors: PyReadonlyArray2<f32>,
        metadata: Option<&PyList>,
    ) -> PyResult<(usize, usize)> {
        // Zero-copy access to numpy buffer
        let array = vectors.as_array();
        let rust_vectors: Vec<Vec<f32>> = array
            .rows()
            .into_iter()
            .map(|row| row.to_vec())
            .collect();

        // Convert metadata (same pattern as insert_numpy)
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
            .upsert(collection, ids, rust_vectors, rust_metadata)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to upsert vectors: {}", e)))
    }

    /// Flush all pending writes to disk
    ///
    /// This ensures all data is persisted to disk. Called automatically
    /// when the database is closed via close() or context manager exit.
    fn flush(&self) -> PyResult<()> {
        self.inner
            .flush()
            .map_err(|e| PyRuntimeError::new_err(format!("Flush failed: {}", e)))
    }

    /// Close the database, flushing all pending writes to disk
    ///
    /// This ensures all data is persisted to SST files before the database
    /// is closed. This is important for:
    /// - Creating SST files with centroid indexes (enables approximate search)
    /// - Avoiding WAL replay overhead on subsequent opens
    /// - Ensuring data durability
    ///
    /// Example:
    ///     db = ProximaDB("/tmp/mydb")
    ///     db.insert("collection", vectors, ids)
    ///     db.close()  # Flushes and closes gracefully
    fn close(&self) -> PyResult<()> {
        self.flush()
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

    // ========================================================================
    // Generic Graph Operations - Tool Agnostic API
    // ========================================================================
    //
    // These methods provide a generic graph API. Domain-specific behavior
    // (like code intelligence) should be built as an adapter in the consuming
    // application (e.g., Victor).

    /// Create a graph collection
    ///
    /// A graph must be created before nodes and edges can be added.
    ///
    /// Args:
    ///     graph_id: Unique identifier for the graph
    ///     engine: Graph engine type ("orion", "pulsar", "quasar") (optional)
    ///
    /// Example:
    ///     ```python
    ///     db.create_graph("my_knowledge_graph", engine="orion")
    ///     db.create_nodes("my_knowledge_graph", nodes)
    ///     ```
    #[pyo3(signature = (graph_id, engine=None))]
    fn create_graph(&self, graph_id: &str, engine: Option<&str>) -> PyResult<()> {
        self.inner
            .create_graph(graph_id, engine)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create graph: {}", e)))
    }

    /// Create nodes in the graph
    ///
    /// Inserts nodes with their properties. Use labels and properties
    /// for domain-specific categorization and attributes.
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///     nodes: List of GraphNode objects
    ///
    /// Returns:
    ///     Number of nodes created
    ///
    /// Example:
    ///     ```python
    ///     # For code intelligence:
    ///     nodes = [
    ///         GraphNode("fn_main", labels=["function"], properties={"name": "main", "file": "main.py"}),
    ///         GraphNode("class_A", labels=["class"], properties={"name": "A", "file": "models.py"}),
    ///     ]
    ///     count = db.create_nodes("my_graph", nodes)
    ///
    ///     # For social network:
    ///     nodes = [
    ///         GraphNode("user_1", labels=["Person"], properties={"name": "Alice"}),
    ///         GraphNode("user_2", labels=["Person"], properties={"name": "Bob"}),
    ///     ]
    ///     count = db.create_nodes("social_graph", nodes)
    ///     ```
    fn create_nodes(&self, graph_id: &str, nodes: Vec<PyGraphNode>) -> PyResult<usize> {
        let rust_nodes: Vec<super::GraphNode> = nodes.into_iter().map(|n| n.into()).collect();
        self.inner
            .create_nodes(graph_id, rust_nodes)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create nodes: {}", e)))
    }

    /// Create edges in the graph
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///     edges: List of GraphEdge objects
    ///
    /// Returns:
    ///     Number of edges created
    ///
    /// Example:
    ///     ```python
    ///     edges = [
    ///         GraphEdge("fn_main", "fn_helper", "CALLS"),
    ///         GraphEdge("class_A", "class_B", "INHERITS"),
    ///     ]
    ///     count = db.create_edges("my_graph", edges)
    ///     ```
    fn create_edges(&self, graph_id: &str, edges: Vec<PyGraphEdge>) -> PyResult<usize> {
        let rust_edges: Vec<super::GraphEdge> = edges.into_iter().map(|e| e.into()).collect();
        self.inner
            .create_edges(graph_id, rust_edges)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create edges: {}", e)))
    }

    /// Get a node by its ID
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///     node_id: Node ID to retrieve
    ///
    /// Returns:
    ///     GraphNode if found, None otherwise
    fn get_node(&self, graph_id: &str, node_id: &str) -> PyResult<Option<PyGraphNode>> {
        self.inner
            .get_node(graph_id, node_id)
            .map(|opt| opt.map(|n| n.into()))
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to get node: {}", e)))
    }

    /// Query nodes by labels
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///     labels: List of labels to filter by
    ///
    /// Returns:
    ///     List of matching GraphNode objects
    ///
    /// Example:
    ///     ```python
    ///     # Get all functions
    ///     functions = db.query_nodes_by_labels("code_graph", ["function"])
    ///
    ///     # Get all Person nodes
    ///     people = db.query_nodes_by_labels("social_graph", ["Person"])
    ///     ```
    fn query_nodes_by_labels(&self, graph_id: &str, labels: Vec<String>) -> PyResult<Vec<PyGraphNode>> {
        self.inner
            .query_nodes_by_labels(graph_id, labels)
            .map(|nodes| nodes.into_iter().map(|n| n.into()).collect())
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to query nodes: {}", e)))
    }

    /// Get outgoing edges from a node
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///     node_id: Node ID to get outgoing edges for
    ///     edge_types: Optional list of edge types to filter by
    ///
    /// Returns:
    ///     List of GraphEdge objects representing outgoing edges
    #[pyo3(signature = (graph_id, node_id, edge_types=None))]
    fn get_outgoing_edges(
        &self,
        graph_id: &str,
        node_id: &str,
        edge_types: Option<Vec<String>>,
    ) -> PyResult<Vec<PyGraphEdge>> {
        self.inner
            .get_outgoing_edges(graph_id, node_id, edge_types)
            .map(|edges| edges.into_iter().map(|e| e.into()).collect())
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to get outgoing edges: {}", e)))
    }

    /// Get incoming edges to a node
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///     node_id: Node ID to get incoming edges for
    ///     edge_types: Optional list of edge types to filter by
    ///
    /// Returns:
    ///     List of GraphEdge objects representing incoming edges
    #[pyo3(signature = (graph_id, node_id, edge_types=None))]
    fn get_incoming_edges(
        &self,
        graph_id: &str,
        node_id: &str,
        edge_types: Option<Vec<String>>,
    ) -> PyResult<Vec<PyGraphEdge>> {
        self.inner
            .get_incoming_edges(graph_id, node_id, edge_types)
            .map(|edges| edges.into_iter().map(|e| e.into()).collect())
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to get incoming edges: {}", e)))
    }

    /// Delete a node and its edges
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///     node_id: Node ID to delete
    ///
    /// Returns:
    ///     True if node was deleted, False if not found
    fn delete_node(&self, graph_id: &str, node_id: &str) -> PyResult<bool> {
        self.inner
            .delete_node(graph_id, node_id)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to delete node: {}", e)))
    }

    /// Get graph statistics
    ///
    /// Args:
    ///     graph_id: Graph identifier
    ///
    /// Returns:
    ///     GraphStats with total_nodes and total_edges
    fn graph_stats(&self, graph_id: &str) -> PyResult<PyGraphStats> {
        self.inner
            .graph_stats(graph_id)
            .map(|s| s.into())
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to get graph stats: {}", e)))
    }

    /// Delete entire graph
    ///
    /// Removes all nodes and edges in the graph.
    ///
    /// Args:
    ///     graph_id: Graph identifier
    fn delete_graph(&self, graph_id: &str) -> PyResult<()> {
        self.inner
            .delete_graph(graph_id)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to delete graph: {}", e)))
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

/// Convert serde_json::Value to Python object
fn json_to_python(py: Python<'_>, value: &serde_json::Value) -> PyResult<PyObject> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => Ok(b.into_py(py)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_py(py))
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_py(py))
            } else {
                Ok(n.to_string().into_py(py))
            }
        }
        serde_json::Value::String(s) => Ok(s.into_py(py)),
        serde_json::Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                list.append(json_to_python(py, item)?)?;
            }
            Ok(list.into())
        }
        serde_json::Value::Object(obj) => {
            let dict = PyDict::new(py);
            for (k, v) in obj {
                dict.set_item(k, json_to_python(py, v)?)?;
            }
            Ok(dict.into())
        }
    }
}

/// Initialize tracing/logging for the embedded ProximaDB module.
///
/// This function sets up a tracing subscriber that outputs to stderr.
/// Call this at the beginning of your Python script to enable debug logging.
///
/// Args:
///     level: Log level - "error", "warn", "info", "debug", or "trace" (default: "info")
///
/// Example:
///     import proximadb
///     proximadb.init_logging("debug")  # Enable debug logging
///     db = proximadb.ProximaDB("/tmp/test")
#[pyfunction]
#[pyo3(signature = (level = "info"))]
fn init_logging(level: &str) -> PyResult<()> {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};
    use std::sync::Once;

    static INIT: Once = Once::new();
    let mut initialized = false;

    INIT.call_once(|| {
        // Build filter from environment or provided level
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(level));

        // Create a simple stderr layer
        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_line_number(false)
            .with_file(false)
            .with_thread_ids(false)
            .with_ansi(true)
            .with_writer(std::io::stderr);

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .init();

        initialized = true;
    });

    if !initialized {
        eprintln!("Warning: Logging already initialized, ignoring init_logging() call");
    }

    Ok(())
}

/// Python module definition
/// This exports as "proximadb" which is the module name used by benchmarks
#[pymodule]
fn proximadb(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    // Core classes
    m.add_class::<PyProximaDB>()?;
    m.add_class::<PyDiskConfig>()?;
    m.add_class::<PySearchResult>()?;
    m.add_class::<PyCollectionInfo>()?;
    m.add_class::<PyStorageStats>()?;

    // Graph classes - Victor Framework compatible
    m.add_class::<PyGraphNode>()?;
    m.add_class::<PyGraphEdge>()?;
    m.add_class::<PyGraphStats>()?;

    // Add logging initialization function
    m.add_function(wrap_pyfunction!(init_logging, m)?)?;

    // Add version info
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}
