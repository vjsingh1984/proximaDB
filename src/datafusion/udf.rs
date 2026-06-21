//! # ProximaDB DataFusion Scalar UDFs
//!
//! Custom scalar functions registered into [`super::create_session_context`]. The first
//! is `mc_price`, a vectorized Monte Carlo European option pricer that wraps the
//! dependency-free kernel in [`crate::compute::montecarlo`]. It mirrors how Spark prices
//! options (a UDF over a DataFrame), giving a fair, like-for-like comparison while running
//! Rust-native, rayon-parallel, with zero JVM overhead.
//!
//! Signature: `mc_price(spot, strike, vol, rate, t, is_call, n_paths)`
//!   * `spot, strike, vol, rate, t` — `Float64`
//!   * `is_call` — `Boolean` (true = call, false = put)
//!   * `n_paths` — `Int64` literal (paths per contract; constant across the column)
//!   * returns `Float64` (discounted expected payoff)

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
use arrow_schema::DataType;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility, create_udf};

use crate::compute::montecarlo::mc_price_batch_seq;

// =========================================================================
// JSON extraction UDFs
//
// The pgwire translator rewrites PostgreSQL JSON operators to portable function
// calls — `col -> 'k'` → `JSON_EXTRACT(col, 'k')` and `col ->> 'k'` →
// `JSON_EXTRACT_TEXT(col, 'k')`. A query that is BOTH relational (GROUP BY /
// aggregate) AND JSON-extracting routes to the DataFusion OLAP engine, so these
// functions must be registered there too (the document modality over the
// analytical route). JSON columns materialize to Parquet as Utf8 text; both UDFs
// take (Utf8 json, Utf8 key) → Utf8.
//   * json_extract       — returns the sub-value as compact JSON text (chainable:
//                          `doc->'a'->>'b'` = JSON_EXTRACT_TEXT(JSON_EXTRACT(...))).
//   * json_extract_text  — returns the sub-value as plain text (strings unquoted).
// A missing key, NULL input, non-object container, or unparseable JSON yields NULL.
// =========================================================================

/// `JSON_EXTRACT(json_text, key)` → sub-value as compact JSON text.
pub fn json_extract_udf() -> ScalarUDF {
    create_udf(
        "json_extract",
        vec![DataType::Utf8, DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(|args| json_extract_impl(args, false)),
    )
}

/// `JSON_EXTRACT_TEXT(json_text, key)` → sub-value as plain text.
pub fn json_extract_text_udf() -> ScalarUDF {
    create_udf(
        "json_extract_text",
        vec![DataType::Utf8, DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(|args| json_extract_impl(args, true)),
    )
}

fn json_extract_impl(args: &[ColumnarValue], as_text: bool) -> DFResult<ColumnarValue> {
    if args.len() != 2 {
        return Err(DataFusionError::Execution(format!(
            "json_extract expects 2 arguments (json, key), got {}",
            args.len()
        )));
    }
    let num_rows = args
        .iter()
        .find_map(|a| match a {
            ColumnarValue::Array(arr) => Some(arr.len()),
            ColumnarValue::Scalar(_) => None,
        })
        .unwrap_or(1);
    let docs = args[0].clone().into_array(num_rows)?;
    let keys = args[1].clone().into_array(num_rows)?;
    let docs = docs
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| DataFusionError::Execution("json_extract: arg0 must be Utf8".into()))?;
    let keys = keys
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| DataFusionError::Execution("json_extract: arg1 must be Utf8".into()))?;

    let mut out: Vec<Option<String>> = Vec::with_capacity(num_rows);
    for i in 0..num_rows {
        if docs.is_null(i) || keys.is_null(i) {
            out.push(None);
            continue;
        }
        out.push(extract_one(docs.value(i), keys.value(i), as_text));
    }
    Ok(ColumnarValue::Array(Arc::new(StringArray::from(out))))
}

fn extract_one(doc: &str, key: &str, as_text: bool) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(doc).ok()?;
    // Object key, or numeric index into an array.
    let child = match &parsed {
        serde_json::Value::Object(map) => map.get(key)?,
        serde_json::Value::Array(arr) => arr.get(key.parse::<usize>().ok()?)?,
        _ => return None,
    };
    if as_text {
        match child {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        }
    } else {
        Some(child.to_string())
    }
}

/// Fixed base seed so the UDF is deterministic (same inputs → same prices), which keeps
/// query results reproducible and benchmark comparisons fair.
const MC_BASE_SEED: u64 = 0x5DEE_CE66_D5B0_1234;

/// Build the `mc_price` scalar UDF.
pub fn mc_price_udf() -> ScalarUDF {
    create_udf(
        "mc_price",
        vec![
            DataType::Float64, // spot
            DataType::Float64, // strike
            DataType::Float64, // vol
            DataType::Float64, // rate
            DataType::Float64, // t
            DataType::Boolean, // is_call
            DataType::Int64,   // n_paths
        ],
        DataType::Float64,
        // Deterministic given inputs (fixed seed) → safe to treat as immutable.
        Volatility::Immutable,
        Arc::new(mc_price_impl),
    )
}

fn mc_price_impl(args: &[ColumnarValue]) -> DFResult<ColumnarValue> {
    if args.len() != 7 {
        return Err(DataFusionError::Execution(format!(
            "mc_price expects 7 arguments (spot, strike, vol, rate, t, is_call, n_paths), got {}",
            args.len()
        )));
    }

    // Number of rows: from the first array argument (scalars broadcast to this length).
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

    let f64_col = |idx: usize, name: &str| -> DFResult<&Float64Array> {
        arrays[idx]
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                DataFusionError::Execution(format!(
                    "mc_price: argument {idx} ({name}) must be Float64, got {:?}",
                    arrays[idx].data_type()
                ))
            })
    };

    let spot = f64_col(0, "spot")?;
    let strike = f64_col(1, "strike")?;
    let vol = f64_col(2, "vol")?;
    let rate = f64_col(3, "rate")?;
    let t = f64_col(4, "t")?;

    let is_call_arr = arrays[5]
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| {
            DataFusionError::Execution(format!(
                "mc_price: argument 5 (is_call) must be Boolean, got {:?}",
                arrays[5].data_type()
            ))
        })?;

    let n_paths_arr = arrays[6]
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| {
            DataFusionError::Execution(format!(
                "mc_price: argument 6 (n_paths) must be Int64, got {:?}",
                arrays[6].data_type()
            ))
        })?;
    let n_paths = if n_paths_arr.is_empty() {
        0
    } else {
        n_paths_arr.value(0).max(0) as usize
    };

    // Materialize boolean flags (BooleanArray packs bits, so a scalar slice isn't available).
    let is_call: Vec<bool> = (0..num_rows).map(|i| is_call_arr.value(i)).collect();

    let spot_s: &[f64] = spot.values();
    let strike_s: &[f64] = strike.values();
    let vol_s: &[f64] = vol.values();
    let rate_s: &[f64] = rate.values();
    let t_s: &[f64] = t.values();

    // Sequential per batch on purpose: DataFusion parallelizes across partitions, so the UDF
    // must not nest rayon here (that would oversubscribe cores).
    let prices = mc_price_batch_seq(
        spot_s,
        strike_s,
        vol_s,
        rate_s,
        t_s,
        &is_call,
        n_paths,
        MC_BASE_SEED,
    );

    Ok(ColumnarValue::Array(Arc::new(Float64Array::from(prices))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::montecarlo::black_scholes;

    #[test]
    fn mc_price_udf_metadata() {
        let udf = mc_price_udf();
        assert_eq!(udf.name(), "mc_price");
    }

    #[test]
    fn mc_price_impl_matches_black_scholes() {
        // Drive the implementation directly with array arguments.
        let spot = Arc::new(Float64Array::from(vec![100.0, 100.0])) as ArrayRef;
        let strike = Arc::new(Float64Array::from(vec![100.0, 110.0])) as ArrayRef;
        let vol = Arc::new(Float64Array::from(vec![0.2, 0.2])) as ArrayRef;
        let rate = Arc::new(Float64Array::from(vec![0.03, 0.03])) as ArrayRef;
        let t = Arc::new(Float64Array::from(vec![1.0, 1.0])) as ArrayRef;
        let is_call = Arc::new(BooleanArray::from(vec![true, false])) as ArrayRef;
        let n_paths = Arc::new(Int64Array::from(vec![400_000_i64, 400_000])) as ArrayRef;

        let args = vec![
            ColumnarValue::Array(spot),
            ColumnarValue::Array(strike),
            ColumnarValue::Array(vol),
            ColumnarValue::Array(rate),
            ColumnarValue::Array(t),
            ColumnarValue::Array(is_call),
            ColumnarValue::Array(n_paths),
        ];

        let out = mc_price_impl(&args).unwrap();
        let arr = out.into_array(2).unwrap();
        let prices = arr.as_any().downcast_ref::<Float64Array>().unwrap();

        let bs0 = black_scholes(100.0, 100.0, 0.2, 0.03, 1.0, true);
        let bs1 = black_scholes(100.0, 110.0, 0.2, 0.03, 1.0, false);
        assert!((prices.value(0) - bs0).abs() / bs0 < 0.03);
        assert!((prices.value(1) - bs1).abs() / bs1 < 0.03);
    }

    #[test]
    fn mc_price_impl_rejects_wrong_arity() {
        let err = mc_price_impl(&[]).unwrap_err();
        assert!(err.to_string().contains("7 arguments"));
    }
}
