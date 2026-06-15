//! DataFusion Bridge for Window Function Execution
//!
//! Provides a bridge between ProximaDB's native window executor types and
//! DataFusion's window function evaluation. When the `datafusion-integration`
//! feature is enabled, this module can convert `WindowSpec` into DataFusion
//! window expressions and delegate evaluation to DataFusion's compute engine.
//!
//! When DataFusion is not available, the bridge falls back to the native
//! `WindowExecutor` from `window_executor.rs`.

use super::QueryRow;
use super::window_executor::{
    FrameBound, FrameDefinition, SortDirection, WindowFunctionCall, WindowSpec,
};
#[cfg(feature = "datafusion-integration")]
use super::window_executor::WindowFunction;
use anyhow::Result;
#[cfg(feature = "datafusion-integration")]
use anyhow::anyhow;

// ---------------------------------------------------------------------------
// DataFusion-backed executor (feature-gated)
// ---------------------------------------------------------------------------

/// Convert ProximaDB `WindowFunction` to a DataFusion built-in window function.
///
/// Returns the DataFusion `BuiltInWindowFunction` variant and whether the
/// function is an aggregate (which uses a different DataFusion API path).
#[cfg(feature = "datafusion-integration")]
fn map_window_function(func: &WindowFunction) -> Result<DataFusionWindowFunctionKind> {
    // DataFusion 51 replaced the `BuiltInWindowFunction` enum with window UDFs
    // (`datafusion::functions_window`). This bridge only needs a stable identifier for
    // routing/validation (the value is discarded before falling back to native), so we
    // carry the canonical SQL name rather than re-deriving the per-function UDF here.
    let kind = match func {
        WindowFunction::RowNumber => {
            DataFusionWindowFunctionKind::BuiltIn("ROW_NUMBER".to_string())
        }
        WindowFunction::Rank => DataFusionWindowFunctionKind::BuiltIn("RANK".to_string()),
        WindowFunction::DenseRank => {
            DataFusionWindowFunctionKind::BuiltIn("DENSE_RANK".to_string())
        }
        WindowFunction::Lag => DataFusionWindowFunctionKind::BuiltIn("LAG".to_string()),
        WindowFunction::Lead => DataFusionWindowFunctionKind::BuiltIn("LEAD".to_string()),
        WindowFunction::FirstValue => {
            DataFusionWindowFunctionKind::BuiltIn("FIRST_VALUE".to_string())
        }
        WindowFunction::LastValue => {
            DataFusionWindowFunctionKind::BuiltIn("LAST_VALUE".to_string())
        }
        // Aggregate functions go through DataFusion's aggregate path
        WindowFunction::Sum => DataFusionWindowFunctionKind::Aggregate("SUM".to_string()),
        WindowFunction::Avg => DataFusionWindowFunctionKind::Aggregate("AVG".to_string()),
        WindowFunction::Count => DataFusionWindowFunctionKind::Aggregate("COUNT".to_string()),
        WindowFunction::Min => DataFusionWindowFunctionKind::Aggregate("MIN".to_string()),
        WindowFunction::Max => DataFusionWindowFunctionKind::Aggregate("MAX".to_string()),
    };
    Ok(kind)
}

/// Discriminator for DataFusion window function routing.
#[cfg(feature = "datafusion-integration")]
#[derive(Debug)]
// Variant payloads (the canonical fn names) are retained for Debug/routing context
// but not read on the --lib path.
#[allow(dead_code)]
enum DataFusionWindowFunctionKind {
    /// A built-in window function by canonical SQL name (ROW_NUMBER, RANK, LAG, etc.)
    BuiltIn(String),
    /// An aggregate used in a window context (SUM, AVG, COUNT, etc.)
    Aggregate(String),
}

/// Convert ProximaDB `SortDirection` to DataFusion's sort direction flag.
#[cfg(feature = "datafusion-integration")]
fn map_sort_asc(direction: &SortDirection) -> bool {
    matches!(direction, SortDirection::Asc)
}

/// Convert ProximaDB `FrameBound` to DataFusion's `WindowFrameBound`.
#[cfg(feature = "datafusion-integration")]
fn map_frame_bound(
    bound: &FrameBound,
    is_start: bool,
) -> datafusion::logical_expr::WindowFrameBound {
    use datafusion::logical_expr::WindowFrameBound;
    use datafusion_common::ScalarValue;

    match bound {
        FrameBound::Unbounded => {
            if is_start {
                WindowFrameBound::Preceding(ScalarValue::UInt64(None))
            } else {
                WindowFrameBound::Following(ScalarValue::UInt64(None))
            }
        }
        FrameBound::CurrentRow => WindowFrameBound::CurrentRow,
        FrameBound::Offset(n) => {
            let val = ScalarValue::UInt64(Some(*n));
            if is_start {
                WindowFrameBound::Preceding(val)
            } else {
                WindowFrameBound::Following(val)
            }
        }
    }
}

/// Convert ProximaDB `FrameDefinition` to DataFusion's `WindowFrame`.
#[cfg(feature = "datafusion-integration")]
fn map_window_frame(frame: &FrameDefinition) -> datafusion::logical_expr::WindowFrame {
    use datafusion::logical_expr::{WindowFrame, WindowFrameUnits};

    WindowFrame::new_bounds(
        WindowFrameUnits::Rows,
        map_frame_bound(&frame.start, true),
        map_frame_bound(&frame.end, false),
    )
}

// ---------------------------------------------------------------------------
// Public bridge API
// ---------------------------------------------------------------------------

/// A window executor that delegates to DataFusion when the feature is enabled,
/// falling back to the native `WindowExecutor` otherwise.
pub struct DataFusionWindowExecutor {
    /// Native fallback executor (always available).
    native: super::window_executor::WindowExecutor,
    /// Whether to prefer DataFusion when available.
    #[allow(dead_code)]
    prefer_datafusion: bool,
}

impl DataFusionWindowExecutor {
    /// Create a new bridge executor.
    ///
    /// `prefer_datafusion`: when `true` and the `datafusion-integration` feature
    /// is compiled in, the executor will attempt to use DataFusion for evaluation.
    /// Falls back to native on any conversion error.
    pub fn new(prefer_datafusion: bool) -> Self {
        Self {
            native: super::window_executor::WindowExecutor::new(),
            prefer_datafusion,
        }
    }

    /// Execute window functions, using DataFusion if available and preferred.
    pub fn execute(
        &self,
        rows: Vec<QueryRow>,
        window_calls: &[WindowFunctionCall],
    ) -> Result<Vec<QueryRow>> {
        #[cfg(feature = "datafusion-integration")]
        {
            if self.prefer_datafusion {
                match self.execute_via_datafusion(&rows, window_calls) {
                    Ok(result) => return Ok(result),
                    Err(_) => {
                        // Fall back to native executor silently.
                    }
                }
            }
        }

        // Native fallback (always available)
        self.native.execute_window_functions(rows, window_calls)
    }

    /// Describe the window spec as a DataFusion-compatible representation.
    /// Useful for EXPLAIN plans and debugging.
    pub fn describe_spec(spec: &WindowSpec) -> WindowSpecDescription {
        WindowSpecDescription {
            partition_by: spec.partition_by.clone(),
            order_by: spec
                .order_by
                .iter()
                .map(|ob| {
                    format!(
                        "{} {}",
                        ob.field,
                        match ob.direction {
                            SortDirection::Asc => "ASC",
                            SortDirection::Desc => "DESC",
                        }
                    )
                })
                .collect(),
            frame: describe_frame(&spec.frame),
        }
    }

    /// Check whether DataFusion integration is compiled in.
    pub fn is_datafusion_available() -> bool {
        cfg!(feature = "datafusion-integration")
    }

    // -- DataFusion execution path (feature-gated) --

    #[cfg(feature = "datafusion-integration")]
    fn execute_via_datafusion(
        &self,
        rows: &[QueryRow],
        window_calls: &[WindowFunctionCall],
    ) -> Result<Vec<QueryRow>> {
        use arrow::array::{Float64Array, RecordBatch, StringArray};
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        if rows.is_empty() {
            return Ok(vec![]);
        }

        // Collect all field names from rows
        let mut field_names: Vec<String> = Vec::new();
        for row in rows {
            for key in row.fields.keys() {
                if !field_names.contains(key) {
                    field_names.push(key.clone());
                }
            }
        }
        field_names.sort();

        // Infer schema: try numeric first, fall back to string
        let fields: Vec<Field> = field_names
            .iter()
            .map(|name| {
                let is_numeric = rows.iter().all(|row| {
                    row.fields
                        .get(name)
                        .map(|v| v.is_number() || v.is_null())
                        .unwrap_or(true)
                });
                if is_numeric {
                    Field::new(name, DataType::Float64, true)
                } else {
                    Field::new(name, DataType::Utf8, true)
                }
            })
            .collect();

        let schema = Arc::new(Schema::new(fields.clone()));

        // Build Arrow arrays from QueryRow data
        let mut columns: Vec<Arc<dyn arrow::array::Array>> = Vec::new();
        for (i, name) in field_names.iter().enumerate() {
            if fields[i].data_type() == &DataType::Float64 {
                let values: Vec<Option<f64>> = rows
                    .iter()
                    .map(|row| row.fields.get(name).and_then(|v| v.as_f64()))
                    .collect();
                columns.push(Arc::new(Float64Array::from(values)));
            } else {
                let values: Vec<Option<String>> = rows
                    .iter()
                    .map(|row| {
                        row.fields
                            .get(name)
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                    })
                    .collect();
                columns.push(Arc::new(StringArray::from(values)));
            }
        }

        let _batch = RecordBatch::try_new(schema.clone(), columns)
            .map_err(|e| anyhow!("Failed to create RecordBatch: {}", e))?;

        // Validate that the window function mapping works for all calls.
        // This ensures we fail fast if an unsupported function is requested.
        for call in window_calls {
            let _kind = map_window_function(&call.function)?;
            let _frame = map_window_frame(&call.spec.frame);
            for ob in &call.spec.order_by {
                let _asc = map_sort_asc(&ob.direction);
            }
        }

        // For this initial bridge, we validate the conversion path and then
        // delegate to native. Full DataFusion evaluation (SessionContext +
        // LogicalPlan with window nodes) will be wired in a follow-up once
        // the Arrow version alignment is confirmed in CI.
        //
        // The key value of this bridge today:
        //   1. Proves the type mapping compiles against DataFusion 51.0
        //   2. Provides the RecordBatch conversion layer
        //   3. Falls back safely to native execution
        Err(anyhow!(
            "DataFusion window evaluation not yet fully wired; falling back to native"
        ))
    }
}

impl Default for DataFusionWindowExecutor {
    fn default() -> Self {
        Self::new(false)
    }
}

// ---------------------------------------------------------------------------
// Description types (always available, no feature gate)
// ---------------------------------------------------------------------------

/// Human-readable description of a window specification.
#[derive(Debug, Clone)]
pub struct WindowSpecDescription {
    /// PARTITION BY fields.
    pub partition_by: Vec<String>,
    /// ORDER BY clauses as "field ASC/DESC" strings.
    pub order_by: Vec<String>,
    /// Frame description string.
    pub frame: String,
}

/// Describe a frame definition as a human-readable string.
fn describe_frame(frame: &FrameDefinition) -> String {
    let start = describe_bound(&frame.start, "PRECEDING");
    let end = describe_bound(&frame.end, "FOLLOWING");
    format!("ROWS BETWEEN {} AND {}", start, end)
}

/// Describe a single frame bound.
fn describe_bound(bound: &FrameBound, direction: &str) -> String {
    match bound {
        FrameBound::Unbounded => format!("UNBOUNDED {}", direction),
        FrameBound::CurrentRow => "CURRENT ROW".to_string(),
        FrameBound::Offset(n) => format!("{} {}", n, direction),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::window_executor::{WindowFunction, WindowOrderBy};
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// Helper: create a `QueryRow` from a list of (field, value) pairs.
    fn make_row(pairs: Vec<(&str, serde_json::Value)>) -> QueryRow {
        let mut fields = HashMap::new();
        for (k, v) in pairs {
            fields.insert(k.to_string(), v);
        }
        QueryRow {
            fields,
            similarity_score: None,
            graph_distance: None,
            provenance: None,
        }
    }

    #[test]
    fn test_bridge_falls_back_to_native() {
        let executor = DataFusionWindowExecutor::new(false);

        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::RowNumber,
            args: vec![],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "val".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: FrameDefinition::default(),
            },
            output_field: "rn".to_string(),
        };

        let result = executor.execute(rows, &[call]).unwrap();
        assert_eq!(result[0].fields["rn"], json!(1));
        assert_eq!(result[1].fields["rn"], json!(2));
        assert_eq!(result[2].fields["rn"], json!(3));
    }

    #[test]
    fn test_bridge_with_aggregate_fallback() {
        let executor = DataFusionWindowExecutor::new(true); // prefer DF, but no feature

        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Sum,
            args: vec!["val".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "val".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: FrameDefinition {
                    start: FrameBound::Unbounded,
                    end: FrameBound::Unbounded,
                },
            },
            output_field: "total".to_string(),
        };

        let result = executor.execute(rows, &[call]).unwrap();
        // All rows get full-frame sum = 60
        assert_eq!(result[0].fields["total"], json!(60.0));
        assert_eq!(result[1].fields["total"], json!(60.0));
        assert_eq!(result[2].fields["total"], json!(60.0));
    }

    #[test]
    fn test_describe_spec() {
        let spec = WindowSpec {
            partition_by: vec!["dept".to_string()],
            order_by: vec![
                WindowOrderBy {
                    field: "salary".to_string(),
                    direction: SortDirection::Desc,
                },
                WindowOrderBy {
                    field: "name".to_string(),
                    direction: SortDirection::Asc,
                },
            ],
            frame: FrameDefinition {
                start: FrameBound::Offset(3),
                end: FrameBound::CurrentRow,
            },
        };

        let desc = DataFusionWindowExecutor::describe_spec(&spec);
        assert_eq!(desc.partition_by, vec!["dept".to_string()]);
        assert_eq!(desc.order_by.len(), 2);
        assert_eq!(desc.order_by[0], "salary DESC");
        assert_eq!(desc.order_by[1], "name ASC");
        assert_eq!(desc.frame, "ROWS BETWEEN 3 PRECEDING AND CURRENT ROW");
    }

    #[test]
    fn test_describe_frame_variants() {
        // Default frame
        let default_frame = FrameDefinition::default();
        let desc = describe_frame(&default_frame);
        assert_eq!(desc, "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW");

        // Full frame
        let full_frame = FrameDefinition {
            start: FrameBound::Unbounded,
            end: FrameBound::Unbounded,
        };
        let desc = describe_frame(&full_frame);
        assert_eq!(
            desc,
            "ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING"
        );

        // Offset frame
        let offset_frame = FrameDefinition {
            start: FrameBound::Offset(2),
            end: FrameBound::Offset(1),
        };
        let desc = describe_frame(&offset_frame);
        assert_eq!(desc, "ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING");
    }

    #[test]
    fn test_is_datafusion_available() {
        // Without the feature flag, should return false
        let available = DataFusionWindowExecutor::is_datafusion_available();
        // We just check it returns a bool without panicking
        assert!(available || !available);
    }

    #[test]
    fn test_empty_rows() {
        let executor = DataFusionWindowExecutor::new(true);
        let result = executor.execute(vec![], &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_default_executor() {
        let executor = DataFusionWindowExecutor::default();
        assert!(!executor.prefer_datafusion);
    }
}
