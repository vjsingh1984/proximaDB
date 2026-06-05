//! # Registry → DataFusion scalar-UDF adapter (F2)
//!
//! The engine-neutral [`ProximaFunctionRegistry`] defines each scalar function ONCE as a
//! row-wise `ProximaValue` kernel (the same definition the Volcano executor dispatches
//! through in F1b). This module is the **DataFusion adapter**: it wraps any registry scalar
//! kernel as a DataFusion [`ScalarUDF`] so the OLAP/MPP engine can serve the *same* function
//! without re-defining it — the Calcite/Velox "one definition, many physical bindings"
//! pattern.
//!
//! The wrapper is row-wise (Arrow row → `ProximaValue` → kernel → `ProximaValue` → Arrow) via
//! [`ScalarValue::try_from_array`] / [`ScalarValue::iter_to_array`]. DataFusion's *native*
//! vectorized builtins (UPPER/LOWER/ABS/…) remain the fast path in
//! [`super::logical_lowering::lower_scalar_function`]; this adapter is the binding for
//! registry functions DataFusion does NOT provide natively (custom functions, and — via
//! F4/F5 — vector distances and user `CREATE FUNCTION`s).

use std::sync::Arc;

use arrow_array::ArrayRef;
use arrow_schema::DataType;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::logical_expr::{
    Accumulator, AggregateUDF, ColumnarValue, ScalarUDF, Volatility as DFVolatility, create_udaf,
    create_udf,
};
use datafusion::prelude::SessionContext;
use datafusion::scalar::ScalarValue;
use proximadb_data_model::ProximaValue;
use proximadb_functions::{
    AggregateAccumulator, AggregateFunctionDef, ProximaFunctionRegistry, ScalarFn,
    ScalarFunctionDef, Volatility,
};

use super::logical_lowering::proxima_value_to_scalar;

/// DataFusion-native scalar names that `lower_scalar_function` already maps to vectorized
/// builtins. The adapter never registers over these (the native kernel is faster); a registry
/// entry under one of these names stays on the native fast path.
pub(crate) const DATAFUSION_NATIVE_SCALARS: &[&str] = &[
    "upper",
    "ucase",
    "lower",
    "lcase",
    "length",
    "char_length",
    "character_length",
    "abs",
    "ceil",
    "ceiling",
    "floor",
    "sqrt",
    "concat",
];

/// Convert a DataFusion [`ScalarValue`] (one Arrow cell) to a [`ProximaValue`] for kernel
/// input. Inverse of [`proxima_value_to_scalar`]; covers the common scalar types. Unhandled
/// types surface as a clear error rather than a silent wrong value.
pub(crate) fn scalar_value_to_proxima(v: &ScalarValue) -> DFResult<ProximaValue> {
    Ok(match v {
        ScalarValue::Null => ProximaValue::Null,
        ScalarValue::Boolean(None)
        | ScalarValue::Int8(None)
        | ScalarValue::Int16(None)
        | ScalarValue::Int32(None)
        | ScalarValue::Int64(None)
        | ScalarValue::UInt8(None)
        | ScalarValue::UInt16(None)
        | ScalarValue::UInt32(None)
        | ScalarValue::UInt64(None)
        | ScalarValue::Float32(None)
        | ScalarValue::Float64(None)
        | ScalarValue::Utf8(None)
        | ScalarValue::LargeUtf8(None) => ProximaValue::Null,
        ScalarValue::Boolean(Some(b)) => ProximaValue::Boolean(*b),
        ScalarValue::Int8(Some(x)) => ProximaValue::Int8(*x),
        ScalarValue::Int16(Some(x)) => ProximaValue::Int16(*x),
        ScalarValue::Int32(Some(x)) => ProximaValue::Int32(*x),
        ScalarValue::Int64(Some(x)) => ProximaValue::Int64(*x),
        ScalarValue::UInt8(Some(x)) => ProximaValue::UInt8(*x),
        ScalarValue::UInt16(Some(x)) => ProximaValue::UInt16(*x),
        ScalarValue::UInt32(Some(x)) => ProximaValue::UInt32(*x),
        ScalarValue::UInt64(Some(x)) => ProximaValue::UInt64(*x),
        ScalarValue::Float32(Some(x)) => ProximaValue::Float32(*x),
        ScalarValue::Float64(Some(x)) => ProximaValue::Float64(*x),
        ScalarValue::Utf8(Some(s)) | ScalarValue::LargeUtf8(Some(s)) => {
            ProximaValue::String(s.clone())
        }
        other => {
            return Err(DataFusionError::NotImplemented(format!(
                "registry UDF: unsupported Arrow scalar input {other:?}"
            )));
        }
    })
}

fn to_df_volatility(v: Volatility) -> DFVolatility {
    match v {
        Volatility::Immutable => DFVolatility::Immutable,
        Volatility::Stable => DFVolatility::Stable,
        Volatility::Volatile => DFVolatility::Volatile,
    }
}

/// Evaluate a registry scalar `kernel` over a DataFusion argument batch, row-wise. Extracted
/// from the UDF closure so it is unit-testable without DataFusion's `ScalarUDF` invocation
/// machinery. `name` is used only for error context.
fn eval_registry_scalar(
    name: &str,
    kernel: &Arc<ScalarFn>,
    return_type: &DataType,
    args: &[ColumnarValue],
) -> DFResult<ColumnarValue> {
    let num_rows = args
        .iter()
        .find_map(|a| match a {
            ColumnarValue::Array(arr) => Some(arr.len()),
            ColumnarValue::Scalar(_) => None,
        })
        .unwrap_or(1);
    let arrays: Vec<ArrayRef> = args
        .iter()
        .cloned()
        .map(|a| a.into_array(num_rows))
        .collect::<DFResult<Vec<_>>>()?;

    let mut out: Vec<ScalarValue> = Vec::with_capacity(num_rows);
    let mut row: Vec<ProximaValue> = Vec::with_capacity(arrays.len());
    for r in 0..num_rows {
        row.clear();
        for arr in &arrays {
            row.push(scalar_value_to_proxima(&ScalarValue::try_from_array(arr, r)?)?);
        }
        let result =
            (kernel)(&row).map_err(|e| DataFusionError::Execution(format!("{name}: {e}")))?;
        // A NULL result must carry the function's *declared* return type — `ScalarValue::Null`
        // is untyped and would make `iter_to_array` reject the otherwise-uniform batch. (Non-null
        // results are assumed to match `return_type`; the type-narrowing builtins like ABS are
        // DataFusion-native and never routed through this adapter.)
        let sv = match result {
            ProximaValue::Null => ScalarValue::try_from(return_type).map_err(|e| {
                DataFusionError::Execution(format!("{name}: cannot build typed null: {e}"))
            })?,
            v => proxima_value_to_scalar(&v)?,
        };
        out.push(sv);
    }
    Ok(ColumnarValue::Array(ScalarValue::iter_to_array(out)?))
}

/// Wrap a registry scalar definition as a DataFusion [`ScalarUDF`] (fixed arity). Each batch is
/// evaluated row-wise through the engine-neutral kernel. Variadic functions are NOT adapted
/// here (DataFusion's native variadic builtins cover the current set); callers should skip
/// them.
pub fn proxima_scalar_udf(def: Arc<ScalarFunctionDef>) -> ScalarUDF {
    let arg_types = def
        .signature
        .arg_types
        .iter()
        .map(|t| t.to_arrow_type())
        .collect::<Vec<_>>();
    let return_type = def.signature.return_ty.to_arrow_type();
    let volatility = to_df_volatility(def.signature.volatility);
    let name = def.signature.name.clone();
    let kernel = def.kernel.clone();
    let ret_for_closure = return_type.clone();

    create_udf(
        &def.signature.name,
        arg_types,
        return_type,
        volatility,
        Arc::new(move |args: &[ColumnarValue]| {
            eval_registry_scalar(&name, &kernel, &ret_for_closure, args)
        }),
    )
}

/// Register every registry scalar that DataFusion does NOT provide natively as a [`ScalarUDF`]
/// on `ctx`, so the OLAP path can serve registry/custom functions. Native builtins and variadic
/// functions are skipped (native fast path / no fixed-arity wrapper).
pub fn register_proxima_scalars(ctx: &SessionContext, reg: &ProximaFunctionRegistry) {
    for def in reg.scalar_defs() {
        let name = def.signature.name.to_ascii_lowercase();
        if def.signature.variadic || DATAFUSION_NATIVE_SCALARS.contains(&name.as_str()) {
            continue;
        }
        ctx.register_udf(proxima_scalar_udf(def));
    }
}

/// Build a typed [`ScalarValue`] for a result/state cell: NULL carries the declared arrow type
/// (untyped `ScalarValue::Null` would mismatch the UDAF's return/state type), other values map
/// directly.
fn typed_scalar(v: &ProximaValue, ty: &DataType) -> DFResult<ScalarValue> {
    match v {
        ProximaValue::Null => ScalarValue::try_from(ty)
            .map_err(|e| DataFusionError::Execution(format!("typed null: {e}"))),
        other => proxima_value_to_scalar(other),
    }
}

/// DataFusion [`Accumulator`] that delegates to an engine-neutral
/// [`AggregateAccumulator`]. Arrow batches are folded row-wise into the registry kernel;
/// `state`/`merge_batch` carry partial state across partitions for distributed aggregation.
struct ProximaDfAccumulator {
    inner: Box<dyn AggregateAccumulator>,
    name: String,
    return_type: DataType,
    state_types: Vec<DataType>,
}

impl std::fmt::Debug for ProximaDfAccumulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProximaDfAccumulator")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Accumulator for ProximaDfAccumulator {
    fn update_batch(&mut self, values: &[ArrayRef]) -> DFResult<()> {
        if values.is_empty() {
            return Ok(());
        }
        let num_rows = values[0].len();
        let mut row = Vec::with_capacity(values.len());
        for r in 0..num_rows {
            row.clear();
            for arr in values {
                row.push(scalar_value_to_proxima(&ScalarValue::try_from_array(arr, r)?)?);
            }
            self.inner
                .update(&row)
                .map_err(|e| DataFusionError::Execution(format!("{}: {e}", self.name)))?;
        }
        Ok(())
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> DFResult<()> {
        if states.is_empty() {
            return Ok(());
        }
        let num_rows = states[0].len();
        let mut st = Vec::with_capacity(states.len());
        for r in 0..num_rows {
            st.clear();
            for arr in states {
                st.push(scalar_value_to_proxima(&ScalarValue::try_from_array(arr, r)?)?);
            }
            self.inner
                .merge(&st)
                .map_err(|e| DataFusionError::Execution(format!("{}: {e}", self.name)))?;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> DFResult<ScalarValue> {
        typed_scalar(&self.inner.finalize(), &self.return_type)
    }

    fn state(&mut self) -> DFResult<Vec<ScalarValue>> {
        self.inner
            .state()
            .iter()
            .enumerate()
            .map(|(i, v)| typed_scalar(v, self.state_types.get(i).unwrap_or(&DataType::Null)))
            .collect()
    }

    fn size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

/// Wrap a registry aggregate definition as a DataFusion [`AggregateUDF`]. Each partition runs a
/// [`ProximaDfAccumulator`] over the kernel; DataFusion drives partial aggregation via
/// `state`/`merge_batch`.
pub fn proxima_aggregate_udf(def: Arc<AggregateFunctionDef>) -> AggregateUDF {
    let input_types = def
        .signature
        .arg_types
        .iter()
        .map(|t| t.to_arrow_type())
        .collect::<Vec<_>>();
    let return_type = def.signature.return_ty.to_arrow_type();
    let state_types = def
        .kernel
        .state_types()
        .iter()
        .map(|t| t.to_arrow_type())
        .collect::<Vec<_>>();
    let volatility = to_df_volatility(def.signature.volatility);
    let name = def.signature.name.clone();
    let kernel = def.kernel.clone();
    let ret = return_type.clone();
    let st = state_types.clone();

    let factory = Arc::new(move |_args: datafusion::logical_expr::function::AccumulatorArgs| {
        Ok(Box::new(ProximaDfAccumulator {
            inner: kernel.new_accumulator(),
            name: name.clone(),
            return_type: ret.clone(),
            state_types: st.clone(),
        }) as Box<dyn Accumulator>)
    });

    create_udaf(
        &def.signature.name,
        input_types,
        Arc::new(return_type),
        volatility,
        factory,
        Arc::new(state_types),
    )
}

/// Register every registry aggregate as a DataFusion [`AggregateUDF`] on `ctx` (the registry
/// holds only non-native aggregates — COUNT/SUM/… are DataFusion builtins).
pub fn register_proxima_aggregates(ctx: &SessionContext, reg: &ProximaFunctionRegistry) {
    for def in reg.aggregate_defs() {
        ctx.register_udaf(proxima_aggregate_udf(def));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, Float64Array, StringArray};

    fn upper_def() -> Arc<ScalarFunctionDef> {
        proximadb_functions::builtins()
            .lookup_scalar("upper")
            .expect("upper builtin")
    }

    #[test]
    fn adapter_metadata_matches_registry() {
        let udf = proxima_scalar_udf(upper_def());
        assert_eq!(udf.name(), "upper");
    }

    #[test]
    fn adapter_executes_row_wise_over_arrow() {
        // Drive the extracted batch body directly (no ScalarUDF invocation machinery).
        let def = upper_def();
        let rt = def.signature.return_ty.to_arrow_type();
        let input = Arc::new(StringArray::from(vec![Some("ab"), None, Some("Cd")])) as ArrayRef;
        let out =
            eval_registry_scalar("upper", &def.kernel, &rt, &[ColumnarValue::Array(input)]).unwrap();
        let arr = out.into_array(3).unwrap();
        let s = arr.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(s.value(0), "AB");
        assert!(s.is_null(1)); // NULL propagates through the kernel
        assert_eq!(s.value(2), "CD");
    }

    #[test]
    fn native_builtins_are_excluded_from_registration() {
        // The adapter must never shadow DataFusion's faster vectorized builtins.
        for n in ["upper", "lower", "abs", "concat"] {
            assert!(DATAFUSION_NATIVE_SCALARS.contains(&n), "{n} must be excluded");
        }
    }

    fn product_def() -> Arc<AggregateFunctionDef> {
        proximadb_functions::builtins()
            .lookup_aggregate("product")
            .expect("product builtin")
    }

    fn product_accumulator() -> ProximaDfAccumulator {
        let def = product_def();
        ProximaDfAccumulator {
            inner: def.kernel.new_accumulator(),
            name: "product".into(),
            return_type: def.signature.return_ty.to_arrow_type(),
            state_types: def
                .kernel
                .state_types()
                .iter()
                .map(|t| t.to_arrow_type())
                .collect(),
        }
    }

    #[test]
    fn aggregate_udf_metadata_matches_registry() {
        let udf = proxima_aggregate_udf(product_def());
        assert_eq!(udf.name(), "product");
    }

    #[test]
    fn df_accumulator_update_and_evaluate() {
        // product over [2, NULL, 3, 4] = 24 through DataFusion's Accumulator interface.
        let mut acc = product_accumulator();
        let col = Arc::new(Float64Array::from(vec![Some(2.0), None, Some(3.0), Some(4.0)]))
            as ArrayRef;
        acc.update_batch(&[col]).unwrap();
        assert_eq!(acc.evaluate().unwrap(), ScalarValue::Float64(Some(24.0)));
    }

    #[test]
    fn df_accumulator_state_and_merge_partial() {
        // Partition A = 2*5 = 10; partition B = 3; coordinator merges B's state into A → 30.
        let mut a = product_accumulator();
        a.update_batch(&[Arc::new(Float64Array::from(vec![2.0, 5.0])) as ArrayRef])
            .unwrap();
        let mut b = product_accumulator();
        b.update_batch(&[Arc::new(Float64Array::from(vec![3.0])) as ArrayRef])
            .unwrap();
        // b.state() -> [Float64(3)]; build a state batch and merge into a.
        let b_state = b.state().unwrap();
        let state_col = b_state[0].to_array().unwrap();
        a.merge_batch(&[state_col]).unwrap();
        assert_eq!(a.evaluate().unwrap(), ScalarValue::Float64(Some(30.0)));
    }
}
