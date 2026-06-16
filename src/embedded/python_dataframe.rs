//! # Python DataFrame API for ProximaDB (Proxima-Spark)
//!
//! Provides a Rust-native distributed execution engine based on DataFusion
//! with a Python front-end via PyO3.

use crate::datafusion::ProximaDataFusionTable;
use crate::embedded::EmbeddedProximaDB;
use arrow::pyarrow::ToPyArrow;
use datafusion::arrow::util::pretty;
use datafusion::logical_expr::{Expr, LogicalPlan, Operator, ExprSchemable};
use datafusion::prelude::*;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use std::sync::Arc;

#[pyclass]
#[derive(Clone)]
pub struct PyExpr {
    pub expr: Expr,
}

#[pymethods]
impl PyExpr {
    #[new]
    fn new(expr_str: String) -> PyResult<Self> {
        Ok(Self {
            expr: col(expr_str),
        })
    }

    /// Give this expression an alias
    fn alias(&self, name: String) -> Self {
        Self {
            expr: self.expr.clone().alias(name),
        }
    }

    /// Cast this expression to a different data type
    fn cast(&self, to_type: String) -> PyResult<Self> {
        let dt = match to_type.to_lowercase().as_str() {
            "int" | "int64" | "bigint" => datafusion::arrow::datatypes::DataType::Int64,
            "int32" => datafusion::arrow::datatypes::DataType::Int32,
            "float" | "float64" | "double" => datafusion::arrow::datatypes::DataType::Float64,
            "float32" => datafusion::arrow::datatypes::DataType::Float32,
            "string" | "utf8" | "text" => datafusion::arrow::datatypes::DataType::Utf8,
            "bool" | "boolean" => datafusion::arrow::datatypes::DataType::Boolean,
            _ => {
                return Err(PyValueError::new_err(format!(
                    "Unsupported cast type: {}",
                    to_type
                )));
            }
        };
        let empty_schema = datafusion::common::DFSchema::empty();
        Ok(Self {
            expr: self.expr.clone().cast_to(&dt, &empty_schema).map_err(|e| PyRuntimeError::new_err(format!("{}", e)))?,
        })
    }

    /// Check if this expression is null
    fn is_null(&self) -> Self {
        Self {
            expr: self.expr.clone().is_null(),
        }
    }

    // Operator Overloading

    fn __add__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone() + rhs.expr,
        }
    }

    fn __sub__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone() - rhs.expr,
        }
    }

    fn __mul__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone() * rhs.expr,
        }
    }

    fn __truediv__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone() / rhs.expr,
        }
    }

    fn __eq__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().eq(rhs.expr),
        }
    }

    fn __ne__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().not_eq(rhs.expr),
        }
    }

    fn __gt__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().gt(rhs.expr),
        }
    }

    fn __ge__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().gt_eq(rhs.expr),
        }
    }

    fn __lt__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().lt(rhs.expr),
        }
    }

    fn __le__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().lt_eq(rhs.expr),
        }
    }

    fn __and__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().and(rhs.expr),
        }
    }

    fn __or__(&self, rhs: PyExpr) -> Self {
        Self {
            expr: self.expr.clone().or(rhs.expr),
        }
    }

    fn __invert__(&self) -> Self {
        Self {
            expr: self.expr.clone().not(),
        }
    }

    fn __repr__(&self) -> String {
        format!("PyExpr({})", self.expr)
    }
}

/// Create a column expression
#[pyfunction]
pub fn py_col(name: String) -> PyExpr {
    PyExpr { expr: col(name) }
}

/// Create a literal expression
#[pyfunction]
pub fn py_lit(value: &Bound<'_, PyAny>) -> PyResult<PyExpr> {
    if value.is_none() {
        Ok(PyExpr {
            expr: Expr::Literal(datafusion::common::ScalarValue::Null, None),
        })
    } else if let Ok(b) = value.extract::<bool>() {
        Ok(PyExpr { expr: lit(b) })
    } else if let Ok(i) = value.extract::<i64>() {
        Ok(PyExpr { expr: lit(i) })
    } else if let Ok(f) = value.extract::<f64>() {
        Ok(PyExpr { expr: lit(f) })
    } else if let Ok(s) = value.extract::<String>() {
        Ok(PyExpr { expr: lit(s) })
    } else {
        Err(PyValueError::new_err("Unsupported literal type"))
    }
}

// Aggregate Functions (using datafusion::functions_aggregate)

#[pyfunction]
pub fn py_count(expr: PyExpr) -> PyExpr {
    PyExpr { expr: datafusion::functions_aggregate::expr_fn::count(expr.expr) }
}

#[pyfunction]
pub fn py_sum(expr: PyExpr) -> PyExpr {
    PyExpr { expr: datafusion::functions_aggregate::expr_fn::sum(expr.expr) }
}

#[pyfunction]
pub fn py_avg(expr: PyExpr) -> PyExpr {
    PyExpr { expr: datafusion::functions_aggregate::expr_fn::avg(expr.expr) }
}

#[pyfunction]
pub fn py_min(expr: PyExpr) -> PyExpr {
    PyExpr { expr: datafusion::functions_aggregate::expr_fn::min(expr.expr) }
}

#[pyfunction]
pub fn py_max(expr: PyExpr) -> PyExpr {
    PyExpr { expr: datafusion::functions_aggregate::expr_fn::max(expr.expr) }
}

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
    fn sql(&self, _py: Python<'_>, query: String) -> PyResult<PyDataFrame> {
        let df = self
            .db
            .runtime()
            .block_on(async { self.ctx.sql(&query).await })
            .map_err(|e| {
                PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;

        Ok(PyDataFrame {
            df: Arc::new(df),
            db: self.db.clone(),
        })
    }

    /// Register a ProximaDB collection as a table in the DataFusion session
    fn table(&self, _py: Python<'_>, name: String) -> PyResult<PyDataFrame> {
        let df = self
            .db
            .runtime()
            .block_on(async { self.ctx.table(&name).await })
            .map_err(|e| {
                PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;

        Ok(PyDataFrame {
            df: Arc::new(df),
            db: self.db.clone(),
        })
    }

    /// Register all ProximaDB collections as tables in the DataFusion session
    fn refresh_tables(&self, _py: Python<'_>) -> PyResult<()> {
        use crate::datafusion::CollectionInfo;
        use crate::datafusion::NullSplitReader;
        use crate::datafusion::ProximaDataFusionTable;
        use crate::datafusion::infer_schema_from_collection;
        use std::sync::Arc;

        let collections = self.db.list_collections().map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to list collections: {}", e))
        })?;

        for info in collections {
            let table_name = info.name.clone();

            // 1. Get the real schema from the database
            let proxima_schema = self
                .db
                .runtime()
                .block_on(async {
                    if let Some(registry) = crate::storage::schema::registry::global_schema_registry() {
                        registry.get_latest_schema(&table_name).await
                    } else {
                        Ok(None)
                    }
                })
                .map_err(|e| {
                    PyRuntimeError::new_err(format!(
                        "Failed to get schema for {}: {}",
                        table_name, e
                    ))
                })?;

            let schema = if let Some(ps) = proxima_schema {
                infer_schema_from_collection(&ps).map_err(|e| {
                    PyRuntimeError::new_err(format!(
                        "Schema inference failed: {}",
                        e
                    ))
                })?
            } else {
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
                    PyRuntimeError::new_err(format!(
                        "Failed to register table {}: {}",
                        table_name, e
                    ))
                })?;
        }

        Ok(())
    }

    /// Execute a query across the entire ProximaDB cluster (Distributed)
    fn execute_distributed(&self, py: Python<'_>, query: String) -> PyResult<PyDataFrame> {
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
    /// Select columns from the DataFrame
    #[pyo3(signature = (*exprs))]
    fn select(&self, exprs: Vec<PyExpr>) -> PyResult<Self> {
        let df_exprs = exprs.into_iter().map(|e| e.expr).collect::<Vec<_>>();
        let df = (*self.df).clone();
        let new_df = df.select(df_exprs).map_err(|e| {
            PyRuntimeError::new_err(format!("DataFusion error: {}", e))
        })?;

        Ok(Self {
            df: Arc::new(new_df),
            db: self.db.clone(),
        })
    }

    /// Filter the DataFrame
    fn filter(&self, predicate: PyExpr) -> PyResult<Self> {
        let df = (*self.df).clone();
        let new_df = df.filter(predicate.expr).map_err(|e| {
            PyRuntimeError::new_err(format!("DataFusion error: {}", e))
        })?;

        Ok(Self {
            df: Arc::new(new_df),
            db: self.db.clone(),
        })
    }

    /// Alias for filter
    #[pyo3(name = "where")]
    fn py_where(&self, predicate: PyExpr) -> PyResult<Self> {
        self.filter(predicate)
    }

    /// Limit the number of rows
    fn limit(&self, n: usize) -> PyResult<Self> {
        let df = (*self.df).clone();
        let new_df = df.limit(0, Some(n)).map_err(|e| {
            PyRuntimeError::new_err(format!("DataFusion error: {}", e))
        })?;

        Ok(Self {
            df: Arc::new(new_df),
            db: self.db.clone(),
        })
    }

    /// Sort the DataFrame
    #[pyo3(signature = (*exprs))]
    fn sort(&self, exprs: Vec<PyExpr>) -> PyResult<Self> {
        let sort_exprs = exprs.into_iter().map(|e| e.expr.sort(true, false)).collect::<Vec<_>>();
        let df = (*self.df).clone();
        
        let new_df = df.sort(sort_exprs).map_err(|e| {
            PyRuntimeError::new_err(format!("DataFusion error: {}", e))
        })?;

        Ok(Self {
            df: Arc::new(new_df),
            db: self.db.clone(),
        })
    }

    /// Aggregate the DataFrame
    fn aggregate(&self, group_exprs: Vec<PyExpr>, agg_exprs: Vec<PyExpr>) -> PyResult<Self> {
        let df_group = group_exprs.into_iter().map(|e| e.expr).collect::<Vec<_>>();
        let df_agg = agg_exprs.into_iter().map(|e| e.expr).collect::<Vec<_>>();
        let df = (*self.df).clone();
        
        let new_df = df.aggregate(df_group, df_agg).map_err(|e| {
            PyRuntimeError::new_err(format!("DataFusion error: {}", e))
        })?;

        Ok(Self {
            df: Arc::new(new_df),
            db: self.db.clone(),
        })
    }

    /// Join with another DataFrame
    #[pyo3(signature = (other, on, how="inner"))]
    fn join(&self, other: &PyDataFrame, on: Vec<String>, how: &str) -> PyResult<Self> {
        let join_type = match how.to_lowercase().as_str() {
            "inner" => datafusion::logical_expr::JoinType::Inner,
            "left" => datafusion::logical_expr::JoinType::Left,
            "right" => datafusion::logical_expr::JoinType::Right,
            "full" => datafusion::logical_expr::JoinType::Full,
            _ => return Err(PyValueError::new_err(format!("Unsupported join type: {}", how))),
        };

        let left_cols = on.iter().map(|s| s.as_str()).collect::<Vec<_>>();
        let right_cols = on.iter().map(|s| s.as_str()).collect::<Vec<_>>();
        let df = (*self.df).clone();

        let new_df = df.join(other.df.as_ref().clone(), join_type, &left_cols, &right_cols, None).map_err(|e| {
            PyRuntimeError::new_err(format!("DataFusion error: {}", e))
        })?;

        Ok(Self {
            df: Arc::new(new_df),
            db: self.db.clone(),
        })
    }

    /// Add a new column or replace an existing one
    fn with_column(&self, name: String, expr: PyExpr) -> PyResult<Self> {
        let df = (*self.df).clone();
        let new_df = df.with_column(&name, expr.expr).map_err(|e| {
            PyRuntimeError::new_err(format!("DataFusion error: {}", e))
        })?;

        Ok(Self {
            df: Arc::new(new_df),
            db: self.db.clone(),
        })
    }

    /// Convert results to a PyArrow Table (Zero-Copy)
    fn to_arrow(&self, py: Python<'_>) -> PyResult<PyObject> {
        let batches = self
            .db
            .runtime()
            .block_on(async { (*self.df).clone().collect().await })
            .map_err(|e| {
                PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;

        if batches.is_empty() {
            return Ok(py.None());
        }

        let pyarrow = py.import("pyarrow")?;
        let py_batches = PyList::empty(py);

        for batch in batches {
            let py_batch = batch.to_pyarrow(py)?;
            py_batches.append(py_batch)?;
        }

        let table = pyarrow.call_method1("Table", (py_batches,))?;
        Ok(table.into_any().unbind())
    }

    /// Collect results as a list of dictionaries
    fn collect(&self, py: Python<'_>) -> PyResult<PyObject> {
        let batches = self
            .db
            .runtime()
            .block_on(async { (*self.df).clone().collect().await })
            .map_err(|e| {
                PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;

        let list = PyList::empty(py);
        for batch in batches {
            let py_batch = batch.to_pyarrow(py)?;
            let pylist = py_batch.call_method0("to_pylist")?;
            for item in pylist.downcast::<PyList>()?.iter() {
                list.append(item)?;
            }
        }

        Ok(list.into_any().unbind())
    }

    /// Show the first N rows of the DataFrame
    fn show(&self, n: usize) -> PyResult<()> {
        self.db
            .runtime()
            .block_on(async {
                let df = (*self.df).clone();
                df.limit(0, Some(n))
                    .map_err(|e| datafusion::error::DataFusionError::from(e))?
                    .show()
                    .await
            })
            .map_err(|e| {
                PyRuntimeError::new_err(format!("DataFusion error: {}", e))
            })?;
        Ok(())
    }
}

pub fn register_dataframe_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDataFusionSession>()?;
    m.add_class::<PyDataFrame>()?;
    m.add_class::<PyExpr>()?;
    m.add_function(wrap_pyfunction!(py_col, m)?)?;
    m.add_function(wrap_pyfunction!(py_lit, m)?)?;
    m.add_function(wrap_pyfunction!(py_count, m)?)?;
    m.add_function(wrap_pyfunction!(py_sum, m)?)?;
    m.add_function(wrap_pyfunction!(py_avg, m)?)?;
    m.add_function(wrap_pyfunction!(py_min, m)?)?;
    m.add_function(wrap_pyfunction!(py_max, m)?)?;
    Ok(())
}
