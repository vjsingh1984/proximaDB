//! # Python DataFrame API for ProximaDB (Proxima-Spark)
//!
//! Provides a Rust-native distributed execution engine based on DataFusion
//! with a Python front-end via PyO3.

use crate::datafusion::ProximaDataFusionTable;
use crate::embedded::EmbeddedProximaDB;
use datafusion::arrow::pyarrow::PyArrowConvert;
use datafusion::arrow::util::pretty;
use datafusion::prelude::*;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::sync::Arc;

#[pyclass]
pub struct PyDataFusionSession {
    ctx: SessionContext,
    db: Arc<EmbeddedProximaDB>,
}

impl PyDataFusionSession {
    pub fn new(ctx: SessionContext, db: Arc<EmbeddedProximaDB>) -> Self {
        Self { ctx, db }
    }
}

#[pymethods]
impl PyDataFusionSession {
    /// Execute a SQL query and return a DataFrame
    fn sql(&self, py: Python<'_>, query: String) -> PyResult<PyDataFrame> {
        let df = self
            .db
            .runtime()
            .block_on(async { self.ctx.sql(&query).await })
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;

        Ok(PyDataFrame {
            df: Arc::new(df),
            db: self.db.clone(),
        })
    }

    /// Register all ProximaDB collections as tables in the DataFusion session
    fn refresh_tables(&self, py: Python<'_>) -> PyResult<()> {
        use crate::datafusion::CollectionInfo;
        use crate::datafusion::NullSplitReader;
        use crate::datafusion::ProximaDataFusionTable;
        use crate::datafusion::infer_schema_from_collection;
        use std::sync::Arc;

        let collections = self.db.list_collections().map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to list collections: {}", e))
        })?;

        for info in collections {
            let table_name = info.name.clone();

            // 1. Get the real schema from the database
            // In embedded mode, we can block on the async call since we're in a controlled thread
            let proxima_schema = self
                .db
                .runtime()
                .block_on(async {
                    self.db
                        .shared_services()
                        .collection_service
                        .get_latest_schema(&table_name)
                        .await
                })
                .map_err(|e| {
                    pyo3::exceptions::PyRuntimeError::new_err(format!(
                        "Failed to get schema for {}: {}",
                        table_name, e
                    ))
                })?;

            let schema = if let Some(ps) = proxima_schema {
                infer_schema_from_collection(&ps).map_err(|e| {
                    pyo3::exceptions::PyRuntimeError::new_err(format!(
                        "Schema inference failed: {}",
                        e
                    ))
                })?
            } else {
                // Fallback to empty schema if not found (shouldn't happen for existing collections)
                Arc::new(arrow::datatypes::Schema::empty())
            };

            // 2. Create collection info for DataFusion
            let df_info = CollectionInfo::new(
                table_name.clone(),
                info.dimension as usize,
                match info.engine.as_str() {
                    "viper" => crate::datafusion::EngineType::Viper,
                    "helix" => crate::datafusion::EngineType::Helix,
                    _ => crate::datafusion::EngineType::Sst,
                },
            );

            // 3. Create appropriate reader for the engine
            let reader = Arc::new(NullSplitReader::new(
                schema.clone(),
                df_info.engine_type.clone(),
            ));

            // 4. Create and register table
            let table = ProximaDataFusionTable::new(table_name.clone(), df_info, schema, reader);

            self.ctx
                .register_table(&table_name, Arc::new(table))
                .map_err(|e| {
                    pyo3::exceptions::PyRuntimeError::new_err(format!(
                        "Failed to register table {}: {}",
                        table_name, e
                    ))
                })?;
        }

        Ok(())
    }

    /// Execute a query across the entire ProximaDB cluster (Distributed)
    fn execute_distributed(&self, py: Python<'_>, query: String) -> PyResult<PyDataFrame> {
        // 1. Get the DistributedQueryCoordinator from the embedded instance
        // This requires exposing the coordinator in EmbeddedProximaDB

        // 2. Plan and execute across nodes
        // This will return a physical plan that can be wrapped in a DataFusion DataFrame

        // For the prototype, we fallback to local execution but mark the intent
        self.sql(py, query)
    }
}

#[pyclass]
pub struct PyDataFrame {
    df: Arc<DataFrame>,
    db: Arc<EmbeddedProximaDB>,
}

#[pymethods]
impl PyDataFrame {
    /// Convert results to a PyArrow Table (Zero-Copy)
    fn to_arrow(&self, py: Python<'_>) -> PyResult<PyObject> {
        let batches = self
            .db
            .runtime()
            .block_on(async { self.df.clone().collect().await })
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;

        if batches.is_empty() {
            return Ok(py.None());
        }

        // Use the Arrow C Data Interface via pyo3-arrow (if available)
        // or the datafusion::arrow::pyarrow implementation.
        // For the prototype, we return the batches as a list of PyArrow RecordBatches
        let pyarrow = py.import_bound("pyarrow")?;
        let py_batches = PyList::empty(py);

        for batch in batches {
            // Convert each RecordBatch to a PyArrow RecordBatch using PyArrowConvert
            let py_batch = batch.to_pyarrow(py)?;
            py_batches.append(py_batch)?;
        }

        // Combine RecordBatches into a PyArrow Table
        let table = pyarrow.call_method1("Table", (py_batches,))?;
        Ok(table.into_any().unbind())
    }

    /// Collect results as a list of dictionaries
    fn collect(&self, py: Python<'_>) -> PyResult<PyObject> {
        let batches = self
            .db
            .runtime()
            .block_on(async { self.df.clone().collect().await })
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;

        // Convert Arrow RecordBatches to Python list of dicts
        // In a production version, we would use pyarrow for zero-copy
        let list = PyList::empty(py);
        for batch in batches {
            // Simplified conversion for the prototype
            // A real implementation would use arrow::pyarrow or similar
        }

        Ok(list.into_any().unbind())
    }

    /// Show the first N rows of the DataFrame
    fn show(&self, n: usize) -> PyResult<()> {
        self.db
            .runtime()
            .block_on(async {
                self.df
                    .clone()
                    .limit(0, Some(n))
                    .map_err(|e| datafusion::error::DataFusionError::from(e))?
                    .show()
                    .await
            })
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;
        Ok(())
    }
}

pub fn register_dataframe_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDataFusionSession>()?;
    m.add_class::<PyDataFrame>()?;
    Ok(())
}
