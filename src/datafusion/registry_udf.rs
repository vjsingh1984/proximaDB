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
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility as DFVolatility, create_udf};
use datafusion::prelude::SessionContext;
use datafusion::scalar::ScalarValue;
use proximadb_data_model::ProximaValue;
use proximadb_functions::{ProximaFunctionRegistry, ScalarFn, ScalarFunctionDef, Volatility};

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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, StringArray};

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
}
